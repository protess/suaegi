# text-search 조사: repo content-search 백엔드 (rg + git-grep)

> 2026-07-24. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> 구현하지 않는다 — 이 문서가 포팅 계약의 증거 기반이다. 서브에이전트가 verbatim 포팅한다.
> 인용 경로 표기: 별도 명시 없으면 전부 `src/shared/text-search.ts`.
> 다른 파일은 파일명 명시(`types.ts:3543`, `search-match-count.ts:9`, `string-utils.ts:10`,
> 드라이버 `filesystem.ts` / `filesystem-search-git.ts`).
>
> **⚠️ Codex 교차검증 판정 NEEDS-REWORK (2026-07-24).** 인용은 전부 정확하나 **결정-임계 정정 2건**:
> (1) 오라클 케이스 24(`buildSubmatchRegex('(foo'/'[abc')===null`)는 **git-grep이 수용하는데 JS가 거부**함을
> 증명하지 **않는다** — 테스트는 git을 실행하지 않고, 실 Git 2.50.1도 두 패턴을 exit 128로 **거부**한다.
> `null`의 정확한 계약 = "JS regex 컴파일 실패"(git 수용 여부와 무관). (2) byte-offset은 실제 latent 버그:
> Orca가 rg byte offset을 **UTF-16 string index로 오용**(§4·§9). Rust는 byte offset을 canonical 소스 좌표로
> **보존하되 `&str` 슬라이싱은 char-boundary 방어**해야 한다(panic 금지). **최종 계약은 플랜
> `docs/superpowers/plans/2026-07-24-text-search.md`가 Codex 정정을 반영해 supersede한다.**
>
> **가장 중요한 발견 세 줄:**
> 1. **모듈은 순수하다 — 프로세스 실행/전송은 호출자 소유.** `text-search.ts`는 Electron·child_process·fs를
>    import하지 않는다(`:5-7` 주석, import는 `node:path`·`search-match-count`·`string-utils`·타입뿐 `:13-16`).
>    argv 빌더(`buildRgArgs`/`buildGitGrepArgs`)와 스트림 파서(`ingest*`)만 있고, 실제 spawn/kill/timeout은
>    드라이버(`filesystem.ts:939-1022`, `filesystem-search-git.ts:19-97`)가 한다.
> 2. **argv-injection 가드 = `--` 종결자 배치.** rg는 flags → `--` → query → target 순(`:213`),
>    git-grep은 `-e query --` → pathspecs 순(`:322`). `-`로 시작하는 쿼리가 절대 플래그로 안 읽힌다.
>    오라클(`text-search.test.ts:36`)이 `slice(-3) === ['--','needle','/root']`로 이걸 핀한다.
> 3. **대죄 표면(truncated 불변식).** `pushMatch`가 `acc.truncated = true`와 `return 'stop'`을 **같은 tick의
>    연속 두 문장**으로 실행한다(`:129-132`). 캡 도달 = 즉시 truncated. 드라이버 타임아웃도 kill 직전
>    `acc.truncated = true`(`filesystem.ts:1018`, `filesystem-search-git.ts:92`) — silent truncation 불가.

---

## 0. 요약 — 이 조사가 확정한 사실

1. **두 백엔드, 하나의 accumulator.** rg(`--json`) 경로와 git-grep(`--null`) 경로가 같은 `SearchAccumulator`
   (`:18-22`)와 같은 `pushMatch`/`clampLineContext`/`finalize`를 공유한다. git-grep은 rg 미설치(Linux desktop
   entry의 최소 PATH) 폴백(`filesystem-search-git.ts:12-18`, `filesystem.ts:934-937`).
2. **per-file 캡은 rg만 강제한다.** `MAX_MATCHES_PER_FILE = 100`(`:54`)은 rg `--max-count 100`(`:189-190`)으로만
   걸린다. **`buildGitGrepArgs`에는 per-file 캡 플래그가 없다**(`:299-341`) — git-grep 경로는 파일당 100 초과 가능,
   오직 `maxResults` 총량 캡만 받는다. **재현해야 할 비대칭**(§7).
3. **캡·클램프 3종:** per-file(rg만), 총량(`maxResults` ≤ `DEFAULT_SEARCH_MAX_RESULTS = 2000` `:55`),
   라인 길이(`MAX_LINE_CONTENT_LENGTH = 500` `:62`). 셋 다 §7의 대죄 표면.
4. **byte-offset vs UTF-16 index 잠복 불일치.** rg `--json` submatch `start`/`end`는 **UTF-8 byte offset**인데
   `clampLineContext`는 이를 **JS 문자열(UTF-16 code-unit) index**로 그대로 쓴다(`:276`, `:87-88`).
   ASCII는 동일, 멀티바이트는 어긋남 — 오라클은 ASCII만 써서 노출 안 됨(§9 개방질문).
5. **git-grep은 submatch range를 안 준다 → JS RegExp 재구성.** `buildSubmatchRegex`(`:351-364`)로 라인 안의
   모든 occurrence 위치를 다시 찾는다. RegExp 컴파일 실패 시 `null` → whole-line 하이라이트 폴백(`:417-421`).
   git이 확인했으나 JS regex가 0건이어도 whole-line 폴백(`:438-445`) — **git-confirmed hit 드롭 금지**.
6. **오라클 1블록만 impure.** `ingestGitGrepLine` describe의 첫 `it`(`text-search.test.ts:268-306`)이 실제
   `git init`/`git grep`을 shell-out. 나머지 37개는 순수 리터럴. 플랜은 이 1블록을 **녹화 픽스처**로 대체.

---

## 1. 공개 표면 (exported surface)

별도 명시 없으면 전부 `src/shared/text-search.ts`. 모듈 전체 **순수**(clock/IO 의존 0). 부수효과는 인자 `acc`
mutation뿐(`ingest*`, `pushMatch`).

| export | 시그니처(요약) | 반환 | 인용 |
|---|---|---|---|
| `MAX_MATCHES_PER_FILE` | const | `100` | `:54` |
| `DEFAULT_SEARCH_MAX_RESULTS` | const | `2000` | `:55` |
| `SEARCH_TIMEOUT_MS` | const | `15_000` (=15s) | `:56` |
| `MAX_LINE_CONTENT_LENGTH` | const | `500` | `:62` |
| `SearchAccumulator` | type | `{fileMap: Map<string,SearchFileResult>; totalMatches: number; truncated: boolean}` | `:18-22` |
| `SearchOptionsLike` | type | `Pick<SearchOptions, 'caseSensitive'|'wholeWord'|'useRegex'|'includePattern'|'excludePattern'>` | `:138-141` |
| `createAccumulator()` | 팩토리 | `{fileMap:new Map(), totalMatches:0, truncated:false}` | `:24-26` |
| `normalizeRelativePath(path)` | 경로 정규화 | `string` | `:33-35` |
| `splitSearchGlobPatterns(patterns)` | escaped-comma 분할 | `string[]` | `:143-175` |
| `buildRgArgs(query, target, opts)` | rg argv | `string[]` | `:183-215` |
| `ingestRgJsonLine(line, rootPath, acc, maxResults, transformAbsPath?)` | rg json 파싱 | `'continue'|'stop'` | `:225-282` |
| `toGitGlobPathspec(glob, exclude?)` | glob→pathspec | `string` | `:291-295` |
| `buildGitGrepArgs(query, opts)` | git grep argv | `string[]` | `:297-342` |
| `buildSubmatchRegex(query, opts)` | 라인 재스캔 regex | `RegExp | null` | `:351-364` |
| `ingestGitGrepLine(line, rootPath, submatchRegex, acc, maxResults)` | git grep 라인 파싱 | `'continue'|'stop'` | `:366-447` |
| `finalize(acc)` | accumulator→결과 | `SearchResult` | `:451-457` |

**비-export 내부 (반드시 함께 포팅):**
- `SEARCH_MAX_FILE_SIZE = 5 * 1024 * 1024`(`:59`) — rg `--max-filesize`용, `Math.floor(/1024/1024)=5`→`"5M"`.
- `TRUNCATION_MARKER = '…'`(`:63`) — U+2026, JS length 1.
- `acceptMatch(fileResult)`(`:28-30`): `matchCount = (matchCount ?? 0) + 1`.
- `pathFlavor(rootPath)`(`:37-42`): win32/posix 선택(§8).
- `relativeToSearchRoot`(`:44-46`) / `joinSearchRoot`(`:48-50`): `pathFlavor(...).relative|.join`.
- `clampLineContext(text, matchStart, matchLength)`(`:65-103`): 라인 클램프 + display 좌표(§7).
- `pushMatch(fileResult, acc, clamped, lineNumber, maxResults)`(`:106-134`): 매치 push + 캡 판정(§7).

**타입(`types.ts`):**
- `SearchMatch`(`:3543-3550`): `{line, column, matchLength, lineContent, displayColumn?, displayMatchLength?}` — 전부 number 외 `lineContent:string`.
- `SearchFileResult`(`:3552-3557`): `{filePath, relativePath, matches: SearchMatch[], matchCount?}`.
- `SearchResult`(`:3559-3563`): `{files: SearchFileResult[], totalMatches, truncated}`.
- `SearchOptions`(`:3565-3574`): `{query, rootPath, caseSensitive?, wholeWord?, useRegex?, includePattern?, excludePattern?, maxResults?}`.

**의존 export:**
- `escapeRegex(str)`(`string-utils.ts:10-12`): `str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')` — regex 메타 12종 리터럴 이스케이프.
- `normalizeSearchResult`(`search-match-count.ts:23-30`) — §6.

**크럭스: 값의 대소.** `column`/`matchLength`는 **원본 라인 기준 참값**이고, `displayColumn`/`displayMatchLength`는
클램프된 스니펫 기준(§7). 라인이 500 이하면 display 필드는 **아예 없다**(`:76-78`).

---

## 2. rg argv 구성 — `buildRgArgs` (`:183-215`)

고정 배열 리터럴(`:184-193`, **이 순서 그대로**):

| # | arg | 값 | 인용 |
|---|---|---|---|
| 0 | `--json` | | `:185` |
| 1 | `--hidden` | (dotfile 포함) | `:186` |
| 2-3 | `--glob` `!.git` | `.git` 항상 제외 | `:187-188` |
| 4-5 | `--max-count` `"100"` | `String(MAX_MATCHES_PER_FILE)` = per-file 캡 | `:189-190` |
| 6-7 | `--max-filesize` `"5M"` | `${Math.floor(5*1024*1024/1024/1024)}M` | `:191-192` |

조건부(순서 그대로 append):
- `!opts.caseSensitive` → `--ignore-case`(`:194-196`). **기본이 case-insensitive**(smart-case 아님, `-S` 안 씀).
- `opts.wholeWord` → `--word-regexp`(`:197-199`).
- `!opts.useRegex` → `--fixed-strings`(`:200-202`). **기본이 fixed-strings**(리터럴 검색).
- `opts.includePattern` → `splitSearchGlobPatterns`로 쪼갠 각 pat마다 `--glob`, `pat`(`:203-207`).
- `opts.excludePattern` → 각 pat마다 `--glob`, `!${pat}`(`:208-212`).

**종결자(대죄 가드):** `args.push('--', query, target)`(`:213`). `--` 뒤에 query 그다음 target.
`query`가 `-e`나 `--foo`처럼 시작해도 rg는 이를 패턴으로 읽는다. `target`은 `rootPath` **그대로**(WSL 번역 금지,
`:180-182` 주석 — 번역은 spawn 라우팅과 출력 파싱에서만).

**주의:** smart-case/hidden-file/line-number 플래그. rg `--json`은 line_number를 자동 포함하므로 `-n` 불요.
context 플래그(`-A/-B/-C`) **없음** — 매치 라인만.

---

## 3. git-grep argv — `buildGitGrepArgs` / `toGitGlobPathspec` / `buildSubmatchRegex`

### 3.1 `buildGitGrepArgs(query, opts)` (`:297-342`)

고정 프리앰블(`:299-309`, 이 순서):
`-c` `submodule.recurse=false` `grep` `-n` `-I` `--null` `--no-color` `--untracked` `--no-recurse-submodules`.
- `-c submodule.recurse=false` + `--no-recurse-submodules`: `submodule.recurse=true`가 `--untracked`와 충돌해
  실패하는 걸 회피(`:298` 주석).
- `-n`(라인번호), `-I`(바이너리 스킵 = rg `--max-filesize` 대응은 아님, 바이너리만), `--null`(콜론 포함 파일명
  모호성 제거 — filename\0lineno\0content), `--untracked`(미추적 파일 포함).

조건부(`:310-320`):
- `!caseSensitive` → `-i`(`:310-312`). git-grep은 wholeWord용 `-w`(`:313-315`).
- `!useRegex` → `--fixed-strings`, **else `--extended-regexp`**(`:316-320`). rg와 달리 정규식 모드에서 ERE 명시.

**종결자(대죄 가드):** `gitArgs.push('-e', query, '--')`(`:322`). `-e query`로 패턴을 명시 옵션에 바인딩하고
`--`로 pathspec 경계를 끊는다. `-`로 시작하는 쿼리 안전.

pathspec(`:324-340`):
- `includePattern` → 각 pat `toGitGlobPathspec(pat)` push, `hasPathspecs=true`(`:325-330`).
- `excludePattern` → 각 pat `toGitGlobPathspec(pat, true)` push(`:331-336`).
- **pathspec 하나도 없으면 `'.'` push**(`:338-340`) — git grep은 working tree 탐색에 pathspec 필수, `.`=cwd 전체.

**비대칭 재확인:** per-file `--max-count` 없음. 파일당 무제한(총 `maxResults`만).

### 3.2 `toGitGlobPathspec(glob, exclude?)` (`:291-295`)

```
needsRecursive = !glob.includes('/')
pattern = needsRecursive ? `**/${glob}` : glob
return exclude ? `:(exclude,glob)${pattern}` : `:(glob)${pattern}`
```
- `/` 없는 글롭(`*.ts`)은 **`**/` 프리픽스**로 재귀화(rg의 기본 재귀 글로빙 모사, `:288` 주석).
- `/` 있으면(`src/*.ts`) 그대로.
- 오라클: `*.ts`→`:(glob)**/*.ts`, `src/*.ts`→`:(glob)src/*.ts`, `*.ts` exclude→`:(exclude,glob)**/*.ts`
  (`test.ts:234-238`).

### 3.3 `buildSubmatchRegex(query, opts)` (`:351-364`)

git grep은 라인당 첫 hit만 알려주고 컬럼 range를 안 주므로, JS regex로 **한 라인 내 모든 위치를 재스캔**.
```
pattern = opts.useRegex ? query : escapeRegex(query)   (:355)
if opts.wholeWord: pattern = `\b${pattern}\b`           (:356-358)
try: return new RegExp(pattern, `g${caseSensitive ? '' : 'i'}`)  (:359-361)
catch: return null                                       (:361-363)
```
- 항상 `g` 플래그(전역), case-insensitive면 `i` 추가.
- **`null` 반환 = git-grep ERE는 받지만 JS RegExp는 거부하는 쿼리**(POSIX 클래스, back-ref 번호, `\<`/`\>`
  앵커 등 — `:348-350` 주석). 호출자는 whole-line 하이라이트 폴백(§4.2).
- 오라클: `a.b`→source `a\.b` flags `gi`(escapeRegex); `foo` wholeWord→`\bfoo\b`; `a|b` useRegex+caseSensitive→
  source `a|b` flags `g`; `(foo`/`[abc` useRegex→`null`(`test.ts:242-264`).

---

## 4. 스트림 파서

### 4.1 rg JSON 인제스트 — `ingestRgJsonLine` (`:225-282`)

rg `--json`은 라인당 JSON 이벤트 스트림: `begin`/`match`/`end`/`summary`. **`match`만 처리**.

절차:
1. `acc.totalMatches >= maxResults` → **`'stop'`**(`:232-234`) — 첫 관문.
2. `!line`(빈 줄) → `'continue'`(`:235-237`).
3. `JSON.parse(line)`, throw catch → `'continue'`(`:247-251`) — malformed JSON 무시.
4. `msg.type !== 'match' || !msg.data` → `'continue'`(`:252-254`) — begin/end/summary 스킵.
5. `rawPath = data.path?.text`; `typeof !== 'string'` → `'continue'`(`:256-259`).
6. `absPath = transformAbsPath ? transformAbsPath(rawPath) : rawPath`(`:260`) — WSL 번역 훅(로컬만, 릴레이 none).
7. `relPath = normalizeRelativePath(relativeToSearchRoot(rootPath, absPath))`(`:261`).
8. `lineContent = (data.lines?.text ?? '').replace(/\n$/, '')`(`:262`) — **후행 개행 1개만** 제거.
9. `lineNumber = data.line_number ?? 0`(`:263`).
10. `submatches = data.submatches ?? []`(`:264`).
11. **빈-submatch 폴백(`:265-268`):**
    ```
    if submatches.length === 0:
        submatches = [{ start: 0, end: lineContent.length > 0 ? 1 : 0 }]
    ```
    rg가 라인은 매치인데 submatch range를 안 준 경우, count-0 row 대신 navigable 라인-레벨 결과를 합성.
    빈 라인이면 `end: 0`(matchLength 0), 아니면 `end: 1`. **매치를 잃지 않기 위한 핵심 가드.**
12. 각 sub에 대해(`:270-280`): `absPath` 키로 fileResult get/create(`:271-275`) →
    `clamped = clampLineContext(lineContent, sub.start, sub.end - sub.start)`(`:276`) →
    `pushMatch(...)==='stop'` 이면 즉시 `'stop'` 반환(`:277-279`).
13. `'continue'`(`:281`).

**byte vs char(잠복 버그):** `sub.start`/`sub.end`는 rg의 UTF-8 **byte offset**. `clampLineContext`가 이를
문자열 index로 slice(`:87`) & `column = matchStart + 1`(`:98`). ASCII 동일, 멀티바이트 어긋남. §9.

### 4.2 git-grep 라인 인제스트 — `ingestGitGrepLine` (`:366-447`)

porcelain 라인 포맷: 현대 git `--null -n` = `filename\0lineno\0content`; 구버전 폴백 `filename\0lineno:content`.

절차:
1. `totalMatches >= maxResults` → `'stop'`(`:373-375`).
2. `!line` → `'continue'`(`:376-378`).
3. `nullIdx = line.indexOf('\0')`; `-1`이면 `'continue'`(`:381-384`) — NUL 최소 1개 필수.
4. `relPath = normalizeRelativePath(line.substring(0, nullIdx))`(`:385`).
5. `rest = line.substring(nullIdx+1)`; `secondNullIdx = rest.indexOf('\0')`(`:386-387`).
6. **2번째 NUL 있으면**(현대): `lineNumberText = rest[0..2ndNull]`, `lineContent = rest[2ndNull+1..].replace(/\n$/,'')`(`:390-392`).
   **없으면**(구버전 콜론): `colonIdx = rest.indexOf(':')`; `-1`이면 `'continue'`;
   `lineNumberText = rest[0..colon]`, `lineContent = rest[colon+1..].replace(/\n$/,'')`(`:393-400`).
7. `!/^\d+$/.test(lineNumberText)` → `'continue'`(`:401-403`) — 숫자 아닌 라인번호 거부.
8. `lineNum = Number(lineNumberText)`(`:404`); `absPath = joinSearchRoot(rootPath, relPath)`(`:406`); fileResult 클로저(`:407-414`).
9. **`submatchRegex === null`** → whole-line: `clampLineContext(lineContent, 0, lineContent.length)` push, 반환(`:417-421`).
10. **regex 있으면**(`:423-437`): `lastIndex=0`; `while (m = exec(lineContent))`:
    `clampLineContext(lineContent, m.index, m[0].length)` → `pushMatch`→`'stop'` 시 반환; `acceptedLineMatch=true`;
    **zero-length 매치면 `lastIndex++`**(무한루프 방지, `:433-436`).
11. **`!acceptedLineMatch`**(`:438-445`): git은 확인했는데 JS regex가 0건 → whole-line 폴백 push(`:439-444`).
    **git-confirmed hit 드롭 금지.**
12. `'continue'`(`:446`).

git-grep은 `m.index`/`m[0].length`가 **JS 문자열 index**라 `clampLineContext`와 일관(rg 경로의 byte/char 불일치
없음). 단 lineContent는 git이 준 바이트열을 UTF-8 디코드한 것.

---

## 5. glob 분할 — `splitSearchGlobPatterns` (`:143-175`)

escaped-comma 상태기계. 콤마로 분할하되 **이스케이프된 콤마는 리터럴로 유지**, 각 조각 `trim`.

```
out=[]; current=''; escaping=false
for ch of patterns:                       (:147)
    if escaping:                          (:148-152)
        current += `\\${ch}`               ← 백슬래시 + ch 둘 다 유지
        escaping = false; continue
    if ch === '\\':                        (:153-156)
        escaping = true; continue          ← 백슬래시 소비(아직 추가 안 함)
    if ch === ',':                         (:157-164)
        trimmed = current.trim()
        if trimmed: out.push(trimmed)      ← 빈 조각은 버림
        current = ''; continue
    current += ch                          (:165)
if escaping: current += '\\'              (:167-169)  ← 후행 단독 백슬래시 보존
trimmed = current.trim()                   (:170-173)
if trimmed: out.push(trimmed)
return out
```

**정확한 규칙:**
- `\` 다음 문자는 항상 `\` + 문자로 **원형 보존**(`\,`→`\,`, `\x`→`\x`). 이스케이프는 콤마 전용이 아니라 **모든 다음 문자**를 리터럴화.
- 이스케이프 안 된 `,`만 분할점. 이스케이프된 `,`(=`\,`)는 조각 안에 남는다.
- 각 조각 `.trim()`, 빈 문자열(trailing comma, `,,`, 공백-only)은 push 안 함.
- **후행 단독 백슬래시**(`escaping`이 true인 채 종료) → `current += '\\'` → 리터럴 `\` 유지(`:167-169`).

**엣지(오라클 핀):**
- `foo\,bar/**, *.ts, dist/**` → `['foo\,bar/**','*.ts','dist/**']`(`test.ts:61-67`). 이스케이프 콤마 유지 + 공백 트림.
- `src\` → `['src\']`(`test.ts:69-72`). 후행 백슬래시 보존.
- `buildRgArgs`/`buildGitGrepArgs`가 이 결과를 `--glob`/pathspec에 그대로 사용(§2·§3).

**주의(Rust):** `for (const ch of patterns)`는 **코드포인트** 순회(surrogate pair 안전). `current += `\\${ch}``의
백슬래시는 리터럴 `\`. Rust는 `chars()` 순회로 대응.

---

## 6. finalize + normalizeSearchResult

### `finalize(acc)` (`:451-457`)
```
return normalizeSearchResult({
  files: Array.from(acc.fileMap.values()).filter(f => f.matches.length > 0),  (:453)
  totalMatches: acc.totalMatches,
  truncated: acc.truncated
})
```
`fileMap` 삽입 순서로 배열화, `matches.length > 0`인 파일만(1차 필터). `totalMatches`/`truncated` 그대로 전달.

### `normalizeSearchResult(result)` (`search-match-count.ts:23-30`)
```
files: result.files
  .filter(f => f.matches.length > 0)           (:27)  ← 2차 필터(중복이지만 방어)
  .map(normalizeSearchFileResult)              (:28)
```
- `normalizeSearchFileResult`(`:16-21`): `{...fileResult, matchCount: normalizeSearchFileMatchCount(fileResult)}`.
- `normalizeSearchFileMatchCount`(`:9-14`): `matchCount = isValidMatchCount(matchCount) ? matchCount : 0`;
  `return Math.max(matchCount, matches.length)`. **matchCount는 절대 matches.length보다 작을 수 없다**(하한 보정).
- `isValidMatchCount`(`:3-7`): number && finite && integer && `>= 0`.

**크럭스:** matchCount는 (a) 유효하지 않으면 0으로, (b) matches.length 미만이면 length로 끌어올린다.
매치 있는데 matchCount 없거나 낮은 경우 방어. 오라클 `test.ts:451-473`, `:475-483`이 핀.

---

## 7. 캡·트렁케이션·클램프 (대죄 표면)

### 7.1 라인 콘텐츠 클램프 — `clampLineContext(text, matchStart, matchLength)` (`:65-103`)

- **`text.length <= MAX_LINE_CONTENT_LENGTH(500)`**: `{lineContent: text, column: matchStart+1, matchLength}`
  반환 — **display 필드 없음**(`:76-78`).
- **초과 시(윈도잉, `:79-102`):**
  ```
  clampedMatchLength = min(matchLength, 500)          (:80)  ← 멀티-MB regex hit 방어
  remaining = 500 - clampedMatchLength                 (:81)
  leftBudget = floor(remaining / 2)                    (:82)
  windowStart = max(0, matchStart - leftBudget)        (:83)
  windowEnd   = min(text.length, windowStart + 500)    (:84)
  windowStart = max(0, windowEnd - 500)                (:85)  ← 우측 잘릴 때 좌측 되당겨 500 채움
  snippet = text.slice(windowStart, windowEnd)         (:87)
  column  = matchStart - windowStart + 1               (:88)
  if windowStart > 0: snippet = '…'+snippet; column += 1   (:89-92)
  if windowEnd < text.length: snippet = snippet+'…'         (:93-95)
  return { lineContent: snippet, column: matchStart+1,       (:96-102)
           matchLength, displayColumn: column, displayMatchLength: clampedMatchLength }
  ```
- **참값 vs 표시값:** `column`/`matchLength`는 **원본 라인 참값**, `displayColumn`/`displayMatchLength`는
  스니펫 기준. `'…'`(길이 1) 마커가 좌측 붙으면 displayColumn += 1(`:91`).
- **크기 상한:** 스니펫 ≤ 500 + 마커 최대 2개 = 502. 오라클 `lineContent.length <= MAX_LINE_CONTENT_LENGTH + 2`
  (`test.ts:175`).

### 7.2 매치 push + 캡 판정 — `pushMatch` (`:106-134`)

```
match: SearchMatch = { line, column, matchLength, lineContent }   (:114-119)
if clamped.displayColumn !== undefined: match.displayColumn = ...  (:120-122)
if clamped.displayMatchLength !== undefined: match.displayMatchLength = ...  (:123-125)
fileResult.matches.push(match)          (:126)
acceptMatch(fileResult)                 (:127)  → fileResult.matchCount++  (:29)
acc.totalMatches++                      (:128)
if acc.totalMatches >= maxResults:      (:129-132)
    acc.truncated = true                (:130)  ┐ 같은 tick 연속 두 문장
    return 'stop'                        (:131)  ┘  = truncated 불변식
return 'continue'                        (:133)
```

**truncated 불변식(대죄 핵심):** `acc.truncated = true`(`:130`)와 `return 'stop'`(`:131`)은 분리 불가한 연속
문장. 호출자는 이 tick 이전에 truncated를 뒤집거나 resolve하면 안 됨(`:222-223` 주석). 캡 도달 = 즉시 truncated.

### 7.3 캡 3종 정리

| 캡 | 값 | 강제 위치 | 비고 |
|---|---|---|---|
| per-file | `MAX_MATCHES_PER_FILE=100`(`:54`) | rg `--max-count 100`(`:189-190`) | **git-grep은 강제 안 함**(§3.1) |
| 총량 | `maxResults`(드라이버 clamp ≤ `DEFAULT_SEARCH_MAX_RESULTS=2000`) | `ingest*` 진입 `>=` 체크(`:232`,`:373`) + `pushMatch`(`:129`) | 드라이버 `Math.max(1, Math.min(maxResults ?? 2000, 2000))`(`filesystem.ts:927-930`) |
| 라인 길이 | `MAX_LINE_CONTENT_LENGTH=500`(`:62`) | `clampLineContext`(`:76-102`) | 스니펫 ≤ 502 |
| 파일 크기 | `SEARCH_MAX_FILE_SIZE=5MB`(`:59`) | rg `--max-filesize 5M`(`:191-192`) | git-grep은 `-I`(바이너리만) |

**stop/continue 프로토콜:** `ingest*`는 `'stop'`을 반환하고 드라이버가 `child.kill()`(`filesystem.ts:976-978`,
`filesystem-search-git.ts:56-58`). 캡은 `>=` 경계(정확히 도달 시 다음 인제스트 진입에서 stop). 오라클
`test.ts:138-154`(rg), `:371-379`(git)이 `truncated===true` && `totalMatches===maxResults`를 동기 확인.

**드라이버 타임아웃도 truncated:** `SEARCH_TIMEOUT_MS(15s)` 만료 시 `acc.truncated = true` → `child.kill()` →
`finalize`(`filesystem.ts:1017-1021`, `filesystem-search-git.ts:91-95`). **타임아웃도 silent truncation 아님.**
(모듈 밖 impure지만 불변식 계승 — Rust 드라이버가 반드시 재현.)

---

## 8. 경로 정규화 — `normalizeRelativePath` (`:33-35`) + pathFlavor

```
normalizeRelativePath(path) =
  path.replace(/[\\/]+/g, '/').replace(/^\/+/, '')     (:34)
```
- `\` 또는 `/`의 **연속 런을 단일 `/`로** 접고, **선행 `/` 전부 제거**.
- 오라클: `a\b\c`→`a/b/c`, `/a/b`→`a/b`, `///a//b`→`a/b`(`test.ts:21-25`).
- 목적(`:32` 주석): 결과의 cross-platform 안정성 + 호출자의 `join(rootPath, relPath)` 보호.

**`pathFlavor(rootPath)`(`:37-42`):** `/^[a-zA-Z]:[\\/]/.test(rootPath)`(드라이브 문자) 또는
`rootPath.startsWith('\\\\')`(UNC) → `win32`, 아니면 `posix`. `relativeToSearchRoot`(`node:path` `.relative`,
`:45`)와 `joinSearchRoot`(`.join`, `:49`)가 이 flavor로 rootPath↔absPath 변환.
- rg 경로: `relativeToSearchRoot(rootPath, absPath)` → normalizeRelativePath(`:261`).
- git-grep 경로: relPath는 git이 준 것, `joinSearchRoot(rootPath, relPath)`로 absPath 복원(`:406`).

**Rust 필요사항:** `normalizeRelativePath`는 regex 2개 = 순수 문자열 연산, `std::path` 불요(수동 replace 권장 —
`std::path`는 플랫폼 종속이라 오라클과 어긋날 수 있음). `pathFlavor`/`.relative`/`.join`은 **Node `path.win32`/
`path.posix`의 정확한 시맨틱**(드라이브 상대·UNC 처리)을 재현해야 함 — Rust `std::path`는 실행 플랫폼 고정이라
직접 대응 불가, **수동 posix/win32 relative/join 구현 필요**(§9).

---

## 9. 오라클 (case-by-case) — `src/shared/text-search.test.ts`

38개 `it()`. 각 줄: 입력 → 기대 → 고정 크럭스. **impure 1블록**(케이스 25)은 플랜에서 픽스처 대체.

**normalizeRelativePath**
1. **(`:21`)** `a\b\c`/`\/a\/b`/`///a//b` → 세퍼레이터 접기 + 선행 슬래시 제거. §8.

**buildRgArgs**
2. **(`:29`) [injection 가드]** `('needle','/root',{})` → `--json`/`--hidden`/`--ignore-case`/`--fixed-strings` 포함;
   `!.git` index > `--glob` index; **`slice(-3)===['--','needle','/root']`**. `--` 앞배치 핀.
3. **(`:39`)** `{caseSensitive,wholeWord,useRegex:true}` → `--ignore-case` 없음, `--word-regexp` 있음, `--fixed-strings` 없음.
4. **(`:46`)** `includePattern:'*.ts, *.tsx'`, `excludePattern:'*.md'` → `*.ts`/`*.tsx`/`!*.md`. 콤마 분할.
5. **(`:53`) [escaped-comma]** `includePattern:'foo\,bar/**, *.ts'` → `foo\,bar/**`, `*.ts`. 이스케이프 콤마 유지.

**splitSearchGlobPatterns**
6. **(`:61`) [escaped-comma]** `foo\,bar/**, *.ts, dist/**` → 3조각, 이스케이프 콤마 보존.
7. **(`:69`) [trailing-backslash]** `src\` → `['src\']`. 후행 단독 백슬래시.

**ingestRgJsonLine**
8. **(`:91`)** match line2 sub[0,3] `abc` → relativePath `src/a.ts`, matchCount 1, `{line2,col1,matchLength3,lineContent'abc'}`. 기본 매핑.
9. **(`:107`)** `type:'begin'` → totalMatches 0. non-match 스킵.
10. **(`:113`)** `'not json'` → `'continue'`, 0. malformed JSON.
11. **(`:120`) [empty-submatch 폴백]** subs `[]` text `foobar` → matchCount 1, `{line4,col1,matchLength1,lineContent'foobar'}`. len>0→end 1.
12. **(`:130`) [empty-submatch 폴백]** subs `[]` text `''` → matchCount 1, `{line5,col1,matchLength0,lineContent''}`. len 0→end 0.
13. **(`:138`) [총량 캡 + 불변식]** 3 subs, maxResults 2 → `'stop'`, truncated true, totalMatches 2, matchCount 2. 동기 truncated.
14. **(`:156`) [라인 클램프]** 400k 라인, matchStart 200000, sub +6 → `lineContent.length <= 500+2`, `column===matchStart+1`(참값),
    matchLength 6, **displayColumn 정의됨**, displayMatchLength 6, `lineContent.slice(displayColumn-1, +6)==='NEEDLE'`. display 좌표 정합.
15. **(`:190`) [WSL transform]** root `\\wsl$\Ubuntu\home\u\repo`, transform이 prefix 치환 → filePath `\\wsl$\Ubuntu\home\u\repo/a.ts`.

**buildGitGrepArgs**
16. **(`:206`)** `{}` → `-i`, `--fixed-strings`, `--no-recurse-submodules` 포함, **`at(-1)==='.'`**(기본 pathspec).
17. **(`:214`)** `{useRegex}` → `--extended-regexp`, `--fixed-strings` 없음.
18. **(`:220`)** include `*.ts`/exclude `dist/**` → `:(glob)**/*.ts`, `:(exclude,glob)dist/**`.
19. **(`:226`) [escaped-comma]** `foo\,bar/**, *.ts` → `:(glob)foo\,bar/**`, `:(glob)**/*.ts`.

**toGitGlobPathspec**
20. **(`:234`)** `*.ts`→`:(glob)**/*.ts`; `src/*.ts`→`:(glob)src/*.ts`; `*.ts` exclude→`:(exclude,glob)**/*.ts`.

**buildSubmatchRegex**
21. **(`:242`)** `a.b`→source `a\.b`, flags `gi`. escapeRegex + 기본 gi.
22. **(`:248`)** `foo` wholeWord→source `\bfoo\b`.
23. **(`:253`)** `a|b` useRegex+caseSensitive→source `a|b`, flags `g`.
24. **(`:259`) [null 폴백 트리거]** `(foo`/`[abc` useRegex→`null`. JS RegExp 거부.

**ingestGitGrepLine**
25. **(`:268-306`) ⚠️ IMPURE — 실제 `git init`/`git grep` shell-out.** mkdtemp→`git init`→`src/a.ts` 3줄 write→
    `execFileSync('git', buildGitGrepArgs('reportError(',...))`→ 각 라인 ingest→finalize:
    totalMatches 3, 1 파일, `src/a.ts`, matchCount 3, matches `[[1,1],[2,1],[2,19]]`. **플랜: 이 stdout을 녹화 픽스처로 고정.**
26. **(`:308`)** `src/a.ts\05\0foo and foo again\n` re `foo` → matchCount 2, col 1 & 9. 라인 내 다중 위치 재스캔.
27. **(`:320`) [legacy 콜론]** `src/a.ts\05:foo` → matchCount 1, col 1. 콜론 폴백 파서.
28. **(`:329`)** `src/a.ts\010\0reportError(err, { action: 'save' })\n` re `reportError(` → matchCount 1, line 10 col 1 matchLength 12. 콘텐츠 콜론이 구분자 아님.
29. **(`:345`)** `weird:name.ts\01\0x` → relativePath `weird:name.ts`. 파일명 콜론 NUL로 처리.
30. **(`:353`)** `no-null-byte` / `a.ts\0no-colon` / `a.ts\0NaN:content` → totalMatches 0. malformed 3종 스킵.
31. **(`:362`) [zero-length 가드]** `new RegExp('','g')`, `a.ts\01\0abc` maxResults 5 → 0 < totalMatches ≤ 5. 무한루프 없음.
32. **(`:371`) [총량 캡 + 불변식]** re `a`, `f\01\0aaaa` maxResults 2 → `'stop'`, truncated true, totalMatches 2, matchCount 2.
33. **(`:381`) [null-regex 폴백]** `a.ts\03\0hello world` regex `null` → matchCount 1, matchLength 11, whole-line.
34. **(`:396`) [git-confirmed-무매치 폴백]** re `/nomatch/g`, `a.ts\03\0git reported this line` → matchCount 1, whole-line. hit 드롭 금지.

**finalize**
35. **(`:415`)** 표준 SearchResult shape, `truncated:true` 그대로 통과.
36. **(`:439`)** matches `[]` 파일 필터, `b.ts`만.
37. **(`:451`)** a.ts(matchCount 없음)→2, b.ts(matchCount 0)→1. `max(matchCount, matches.length)` 보정.
38. **(`:475`)** matchCount 2인데 matches `[]` → files `[]`. 빈 파일 필터가 이긴다.

---

## 10. Rust 생태계 노트 (사실만 — 결정 X)

- **rg는 서브프로세스로 호출한다(기존 선례).** suaegi는 Quick Open에서 이미 rg를 shell-out 한다 —
  `crates/suaegi-git/src/quick_open.rs`가 `tokio::process::Command`로 `rg --files ...`를 spawn하고
  `-z`/NUL-split 스트림 규율(`status.rs` 재사용)로 파싱, **transient≠empty**(실패/타임아웃을 빈 결과로 위장 금지)를
  명시 규율로 둔다(`quick_open.rs:5-21`, `rg_args`는 `:89-102`). text-search 드라이버도 이 spawn/스트림 패턴을 재사용 가능.
- **JSON 파싱 = serde_json.** 워크스페이스 루트 `Cargo.toml:13`에 `serde_json = "1"` 이미 선언, 여러 크레이트가
  `workspace = true`로 사용 중(`suaegi-core`/`suaegi-app`/`suaegi-keys`/`suaegi-forge`/`suaegi-tracker`).
  **단 `suaegi-git/Cargo.toml`은 현재 `serde`만 있고 `serde_json` 없음** — rg `--json` 파싱을 여기 두려면 의존 추가 필요.
- **정규식 크레이트 부재(리스크).** 워크스페이스에 `regex` 크레이트 의존이 **0건**(grep 결과 없음).
  `buildSubmatchRegex`는 **JS RegExp**로 라인을 재스캔한다 — Rust `regex` 크레이트는 문법·플래그(`g`/`i`,
  zero-length 매치 반복, `\b`, JS-특유 거부 규칙)가 JS와 **미묘히 다르다**. 특히 오라클 케이스 24(JS는 거부, ERE는
  수용)의 `null` 판정 재현이 크레이트마다 갈릴 수 있음.
- **호환성 리스크:**
  - rg `--json` 스키마 안정성: `type`/`data.path.text`/`data.line_number`/`data.lines.text`/`data.submatches[].start|end`
    필드 의존(`:238-268`). rg major 버전 간 스키마 안정하다는 일반 인식이나 **소스 대조 없이 단정 불가**.
  - git-grep porcelain: `--null -n` 출력이 `filename\0lineno\0content` 라는 가정(`:390-392`) + 구버전 콜론 폴백
    (`:393-400`). git 버전별 출력 차이가 오라클 케이스 25(실 git)와 26-30(리터럴) 사이 갭 요인.
  - byte vs char: rg submatch는 byte offset, Rust `&str` 슬라이싱은 char-boundary 아니면 panic — JS의 UTF-16
    index 시맨틱과 다른 방향으로 어긋난다(§9 개방질문 2).

---

## 11. Codex 교차검증용 개방 질문

1. **argv-injection 가드 충실도.** rg `--` before query/target(`:213`), git-grep `-e query --`(`:322`)를 Rust에서
   재현할 때 `Command::arg`가 자동으로 셸 해석 없이 넘기므로 안전하나, **오라클 케이스 2(`slice(-3)`)의 정확한
   순서/위치**를 argv 벡터로 핀할 것. `-`로 시작하는 쿼리·타깃이 실제 rg 15.x/git 2.x에서 플래그로 안 읽히는지 실측.
2. **byte-offset vs char-index.** rg submatch `start/end`는 UTF-8 byte offset인데 `clampLineContext`는 문자열 index로
   쓴다(`:276`,`:87-88`). 오라클은 ASCII만(케이스 14 `NEEDLE`)이라 미노출. Rust는 byte 슬라이싱이라 char-boundary
   panic 위험 — **byte offset을 그대로 쓸지(오라클과 일치, 단 boundary 방어), char index로 변환할지(오라클과 divergence)**
   결정 필요. git-grep 경로는 JS RegExp index(UTF-16)라 별개 시맨틱.
3. **glob-escape 상태기계.** `splitSearchGlobPatterns`(`:143-175`)의 이스케이프=모든-다음-문자-리터럴, 후행 단독
   백슬래시 보존(`:167-169`), 코드포인트 순회를 Rust `chars()`로 옮길 때 surrogate/멀티바이트에서 케이스 6·7과
   정확히 일치하는지. `current += `\\${ch}``의 백슬래시가 리터럴 1개인지.
4. **empty-submatch 폴백.** `submatches.length===0 → [{start:0, end: len>0?1:0}]`(`:265-268`). `lineContent.length`가
   JS UTF-16 length — Rust에서 byte len/char count 중 무엇을 쓰든 len>0 판정만 같으면 OK인지(케이스 11·12는 ASCII/빈문자).
5. **truncated 불변식.** `acc.truncated=true` + `return 'stop'` 동기성(`:129-132`)을 Rust 드라이버(async 스트림)에서
   보존할 것. **드라이버 타임아웃(15s)도 kill 직전 truncated 설정**(`filesystem.ts:1018`,
   `filesystem-search-git.ts:92`) — Rust async 취소/타임아웃 경로가 이 순서를 지키는지. quick_open의 transient≠empty
   규율과 동일 계열.
6. **rg `--json` 스키마 가정.** 의존 필드(`type`/`data.path.text`/`line_number`/`lines.text`/`submatches[].start|end`)의
   버전 안정성. serde 역직렬화 시 optional 필드/누락(`data.line_number ?? 0`, `path?.text` non-string→continue)을
   `:256-268`과 동일하게 관대 처리하는지.
7. **git-grep porcelain 포맷 & regex 크레이트.** (a) `--null -n` 출력이 `filename\0lineno\0content`인지, 구버전 콜론
   폴백(`:393-400`)이 여전히 필요한지 — 케이스 25(실 git)를 어떤 git 버전으로 녹화할지. (b) `buildSubmatchRegex`의
   JS RegExp를 Rust `regex` 크레이트로 옮길 때 케이스 24의 `null`(JS 거부/ERE 수용) 판정, zero-length 매치 반복
   (`:433-436`), `\b` wholeWord가 일치하는지. **regex 크레이트 워크스페이스 미도입 상태**라 의존 추가 판단 포함.
8. **shell-out vs 기존 러너 재사용.** rg는 `quick_open.rs`의 tokio Command 패턴 재사용이 자연스럽고,
   git-grep은 `suaegi-git/src/runner.rs`(git 러너)를 태울 수 있다. **suaegi-git에 serde_json 미도입** — rg JSON 파싱을
   suaegi-git에 둘지, 별도 search 크레이트에 둘지. per-file 캡 비대칭(rg만 `--max-count`, git-grep 무제한 §3.1)을
   Rust에서도 그대로 둘지(오라클 충실) 아니면 통일할지(divergence).
