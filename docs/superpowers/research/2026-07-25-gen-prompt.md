> **⚠️ Codex 교차검증 정정 (NEEDS-REWORK→정정 반영):** 핵심 CONFIRMED이나 계약 주장 4건 정정:
> (1) truncation 마커 **"항상 append" 틀림** — budget≤0→빈문자열(마커無), 0-budget 섹션 무마커 생략,
> positive<marker→마커 prefix만; **정확 계약**: 미절단→무마커/절단+budget≤0→빈/절단+소limit→마커prefix/그외→완전마커.
> (2) `parseGeneratedPullRequestFields` **배열은 throw 안 함**(`typeof []==='object'`→fallback); throw는 malformed
> JSON(SyntaxError)·null/number/string/bool(`Error("Expected a JSON object.")`)만. `[]` 핀은 fallback으로.
> (3) **4 모듈**(commit-message-agent-output 재export+오라클 커버, 제외 불가). 197L(198 아님). excerpt 테스트 14개.
> (4) `$` quirk: `$$`/`$&`/`` $` ``/`$'`만 특수(**`$n`은 캡처 없어 리터럴**). (5) UTF-16 정확재현은 `encode_utf16().count()`
> 만으론 불충분(JS slice가 surrogate 쪼갬, Rust String 불가). **budget 단위 결정=플랜 §C2(문서화된 divergence).**
> **최종 계약은 플랜 `docs/superpowers/plans/2026-07-25-gen-prompt.md` supersede.** 오라클 53(43+5+5).

# gen-prompt 조사: AI 커밋/PR 프롬프트+파싱 헬퍼 3(+1)개 → `suaegi-gen-prompt` 리프 크레이트

> 2026-07-25. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> 구현하지 않는다 — 이 문서가 포팅 계약(contract)이다. 서브에이전트는 여기서 **verbatim** 포팅하고,
> 각 `.test.ts`가 **오라클**이다. Orca 원본 경로 base: `…/scratchpad/orca-src/src/shared/`.
>
> **가장 중요한 발견 세 줄:**
> 1. **`STAGED_DIFF_BYTE_BUDGET = 200_000`은 "byte"가 아니다.** `truncateDiffForPrompt`는 전 구간에서
>    `String.length`/`.slice`/`.lastIndexOf`(전부 **UTF-16 code unit**)로 자른다. 마커 문자열도 `${omitted} bytes
>    omitted`라고 **거짓말**한다(실제 단위 = UTF-16 code unit). ASCII diff에서만 byte와 일치. 멀티바이트 diff에서
>    Rust `len()`(UTF-8 byte)나 `chars().count()`(scalar) 어느 쪽을 써도 **오라클과 발산** — 바이트-포-바이트
>    일치를 원하면 `encode_utf16().count()`+UTF-16 경계 슬라이스가 유일. 오라클은 **ASCII만 친다**(§1.5). 이게 최대 함정.
> 2. **`parseGeneratedPullRequestFields`는 garbage-in에서 `throw`한다** — 빈 값/None을 반환하지 **않는다**.
>    `JSON.parse(stripJsonFence(raw))`가 malformed에서 `SyntaxError`를 던지고, non-object에서 `throw new
>    Error('Expected a JSON object.')`(`pull-request-generation.ts:157`). **transient≠garbage**: LLM이 반쪽
>    출력을 주면 필드는 **fallback으로 채우되**, 아예 JSON이 깨지면 **예외로 재시도 신호**를 보낸다(§2.4).
> 3. **"no-regex" 스파이는 3개 함수의 2개 지점만 강제한다** — CRLF 정규화와 fence-body 추출만 손으로 짜라는
>    것. list-marker strip·preamble 판정·trailing-dot strip 정규식은 **스파이가 안 막는다**(§4.2, §3.3). 그러나
>    저장소 규칙(regex-DoS 회피, `regex` 크레이트 금지)상 **전부 손으로 짠다**. 어디가 오라클-강제이고 어디가
>    정책-강제인지 §트랩표에 분리.

---

## 0. 요약 — 이 배치가 확정한 결정

1. **크레이트 = `suaegi-gen-prompt` 하나, 4개 모듈.** 프롬프트가 명명한 3개 파일(`commit-message-prompt`,
   `pull-request-generation`, `commit-message-generation`)에 더해, **`commit-message-agent-output.ts`를 배치에
   포함해야 한다.** 이유: `commit-message-prompt.ts:20-23`이 `cleanGeneratedCommitMessage`/
   `excerptAgentFailureOutput`를 이 파일에서 **re-export**하고, `commit-message-generation.ts:1`이
   `cleanGeneratedCommitMessage`를 **직접 import**해 `splitGeneratedCommitMessage`가 그 위에 선다. 프롬프트는
   "이 파일은 테스트가 없어 제외"라 했으나 — **자체 `.test.ts`는 없지만 오라클은 존재한다**:
   `commit-message-prompt.test.ts:82-248`이 `cleanGeneratedCommitMessage`(:82-137)와
   `excerptAgentFailureOutput`(:139-248)를 re-export 경유로 **완전히 커버**한다. 즉 "오라클-검증 불가"라는 배제
   근거가 성립하지 않는다. → **`cleanGeneratedCommitMessage`는 포팅 필수·오라클 있음**(§4). `excerptAgentFailureOutput`도
   오라클 있으나 커밋/PR 생성 클러스터가 아니라 **에이전트 실패 출력 excerpt** — 이 배치의 스코프 밖이면 별도
   결정 필요(§4.4, §오픈퀘스천). `stripAnsiControlSequences`는 export되지만 excerpt 전용 헬퍼.
2. **전부 순수·클럭 비의존.** 프로덕션 import 그래프: `commit-message-prompt` → (`./commit-message-agent-output`
   re-export만); `pull-request-generation.ts:1` → `truncateDiffForPrompt` from `./commit-message-prompt`;
   `commit-message-generation.ts:1` → `cleanGeneratedCommitMessage`+`truncateDiffForPrompt` from
   `./commit-message-prompt`, `:2` → `type { TuiAgent } from './types'`(타입-온리). `commit-message-agent-output.ts`는
   **import 0**. **fs/child_process/Date/crypto 없음.** 전부 pure-in/pure-out.
3. **최대 발산 위험 = §1의 UTF-16 byte-budget 트렁케이션.** 그 다음이 `.trim()`/`\s`(JS whitespace 집합 ≠ Rust
   `char::is_whitespace`), 그 다음이 `parseGeneratedPullRequestFields`의 throw 시맨틱.
4. **`buildCommitPrompt`는 diff를 truncate하지 않는다**(`:28`, raw 임베드) — 반면 `buildCommitMessagePrompt`
   (`commit-message-generation.ts:41`)와 `buildPullRequestFieldsPrompt`(`pull-request-generation.ts:63`)는
   truncate한다. 이 비대칭이 계약. `buildCommitPrompt`는 구(舊) 프롬프트, `buildCommitMessagePrompt`가 신(新)
   구조화 프롬프트로 **둘 다 export·둘 다 살아있다**.
5. **`.replace('{{DIFF}}', diff)` 숨은 함정**(`commit-message-prompt.ts:28`): JS `String.replace(str, str)`는
   **첫 매치만** 치환하지만 **replacement 문자열에서 `$$`/`$&`/`` $` ``/`$'`/`$n` 특수 패턴을 해석**한다. diff에
   `$&`가 들어오면 `{{DIFF}}`가 주입된다. **오라클 미커버**(§1.2). Rust literal replace는 이 quirk를 안 만듦 →
   **의도적 발산이 안전** but Codex 확인 필요.

**트랩 클래스 히트맵 (프로덕션 코드):**

| 함수 (파일:line) | UTF-16 length/slice | trim/`\s`/case-fold | no-regex 손짜기 | throw/garbage | Math/index |
|---|---|---|---|---|---|
| `buildCommitPrompt` (cmp:27) | — | `.trim()` (:29) | `.replace('{{DIFF}}')` `$`-quirk (:28) | — | — |
| `truncateDiffForPrompt` (cmp:114) | **`.length`/`.slice`/`.lastIndexOf` 전부 UTF-16, "bytes" 마커 거짓말** (:60,67,76,78,80,118,126) | — | `indexOf` 루프 split (:41-55) | — | `Math.floor/min/max` (:77,80,92,99) |
| `tokenizeCustomCommandTemplate` (cmp:144) | `template.length`/`template[i]` UTF-16 인덱싱 (:151-199) | **`/\s/.test(ch)`** (:186) | 상태머신 손짜기(정규식 아님) | `ok:false` 반환(throw 아님) (:202) | — |
| `planCustomCommand` (cmp:223) | — | — | `token.split('{prompt}').join()` (:238) | `ok:false` 반환 (:229,233) | destructure |
| `cleanGeneratedCommitMessage` (cmao:4) | `.indexOf`/`.slice` (:12,17) | **`.trim()`×3** (:7,17,29), `/^(gen\|think)/i` (:15) | **CRLF·fence 손짜기(스파이 강제)** (:32-79) / list-marker `.replace` **정책만** (:27) | — | `charCodeAt` (:59,74) |
| `parseGeneratedPullRequestFields` (prg:151) | `.slice` (:97) | `.trim()`×여러 (:88,160,164), `.replace(/[.]+$/g)` `.replace(/\s+$/g)` (:163,166) | ASCII case-fold 손짜기 (:137-149), fence 손짜기(스파이) (:101-135) | **`JSON.parse` throw + `throw` non-object** (:155,157) | `indexOf`/`lastIndexOf` (:93,94) |
| `splitGeneratedCommitMessage` (cmg:72) | `.slice(0,72)` UTF-16 (:76) | `.trim()`/`.trimEnd()` (:76,77), `.replace(/[.]+$/g)` (:76) | `.indexOf('\n')`(스파이: split 금지) (:74) | — | — |
| `excerptAgentFailureOutput`* (cmao:125) | `.length`/`.slice` UTF-16 (:134,149,153,171) | `.trim()` (:179,190), `/\S/`(:129,130) | `.split(/\r\n\|\r\|\n/)`·`.replace` ANSI(스파이 **없음**) (:176,96) | `null` 반환 (:132) | `.at(-1)` (:144) |

*`excerptAgentFailureOutput`/`stripAnsiControlSequences`는 스코프-경계(§4.4).  cmp=commit-message-prompt, cmao=commit-message-agent-output, prg=pull-request-generation, cmg=commit-message-generation.

---

## 1. `commit-message-prompt.ts` (250L) — 핵심: 프롬프트 조립 + byte-budget 트렁케이션 + 커스텀 명령 토크나이저

### 1.1 공개 표면 (export 전수)
- `export { cleanGeneratedCommitMessage, excerptAgentFailureOutput } from './commit-message-agent-output'` (`:20-23`) — **re-export**. 이 두 함수의 정의는 §4·cmao.
- `buildCommitPrompt(diff: string, customSuffix: string): string` (`:27-34`) — 순수.
- `const STAGED_DIFF_BYTE_BUDGET = 200_000` (`:36`) — export const.
- `truncateDiffForPrompt(diff: string, budget: number = STAGED_DIFF_BYTE_BUDGET): string` (`:114-130`) — 순수.
- `const CUSTOM_PROMPT_PLACEHOLDER = '{prompt}'` (`:132`) — export const.
- `type TokenizeCustomCommandResult = { ok: true; tokens: string[] } | { ok: false; error: string }` (`:134-136`).
- `tokenizeCustomCommandTemplate(template: string): TokenizeCustomCommandResult` (`:144-208`) — 순수.
- `type CustomCommandPlan = { ok: true; binary: string; args: string[]; stdinPayload: string | null } | { ok: false; error: string }` (`:210-212`).
- `planCustomCommand(template: string, prompt: string): CustomCommandPlan` (`:223-250`) — 순수.
- **비-export(내부):** `COMMIT_MESSAGE_BASE_PROMPT`(const, `:5-18`), `splitDiffIntoFileSections`(`:41-55`),
  `clipSectionOnLineBoundary`(`:59-81`), `allocateBudgetFairly`(`:87-109`).

### 1.2 `buildCommitPrompt` — 정확 시맨틱
- `COMMIT_MESSAGE_BASE_PROMPT`(`:5-18`)는 **verbatim 재현 대상** template literal. 본문(줄바꿈 포함):
  ```
  You are generating a single git commit message.
  Read the staged diff below and produce the message.

  Rules:
  - First line: imperative mood, <= 72 chars, no trailing period.
  - Optional body: blank line, then wrapped at 72 chars explaining WHY.
  - Output ONLY the commit message - no preamble, no code fences, no quotes.
  - Do not include "Co-authored-by" trailers - Orca appends them after generation when configured.

  Staged diff:
  ```diff
  {{DIFF}}
  ```
  ```
  — **주의: 닫는 ```` ``` ```` 뒤에 `\n`이 하나 더 있다**(`:17` ```` ``` ````, `:18` 닫는 backtick) → 문자열은
  `"…```\n"`으로 끝난다. Rust raw string으로 그대로 박되 이 trailing newline 유지.
- `:28` `const base = COMMIT_MESSAGE_BASE_PROMPT.replace('{{DIFF}}', diff)`.
  - **함정 A (오라클 미커버):** JS `String.replace(searchString, replacement)`는 (a) **첫 `{{DIFF}}` 하나만**
    치환, (b) replacement=`diff`에서 **`$`-특수 패턴 해석**(`$$`→`$`, `$&`→매치된 `{{DIFF}}`, `` $` ``→prefix,
    `$'`→suffix). diff가 `$&`/`$'` 등을 포함하면 오염된다. Rust `str::replacen(.., .., 1)` 또는 `replace`는
    이 quirk가 없다. **테스트는 `$` 없는 ASCII diff만**(`commit-message-prompt.test.ts:18,25,31`) → 발산이
    숨는다. **포트 결정 필요**: JS quirk 재현(비추천) vs literal(안전). Codex 안건.
  - **함정 B:** BASE에 `{{DIFF}}`가 **딱 하나** 존재 → literal 첫-매치 replace로 충분.
- `:29` `const trimmedSuffix = customSuffix.trim()` — **JS `String.trim()`**. 제거 집합 = JS whitespace
  `[\t\n\v\f\r    -     　﻿]` + line terminators. **Rust
  `str::trim()`는 `char::is_whitespace`(Unicode White_Space) 기준 → `﻿`(BOM)를 안 깎고, 반대로 JS
  `\s`가 안 먹는 문자를 먹는 미세 차이**. MEMORY의 U+FEFF 함정과 동일 계열. 포트는 **JS `\s` 집합을 손으로 정의**.
- `:30-33` `if (!trimmedSuffix) return base;` else `return \`${base}\n\nAdditional user prompt:\n${trimmedSuffix}\``.
  base가 `\n`으로 끝나므로 결과는 `…```\n` + `\n\nAdditional user prompt:\n` + suffix. **suffix가 끝에 온다**(테스트 :27 `endsWith`).

### 1.3 `tokenizeCustomCommandTemplate` — 상태머신 (정확 재현)
POSIX-shell **grouping만**(단·이중 인용, 이중 인용 내 백슬래시 이스케이프). `$VAR`/커맨드치환/글롭/`~` **미확장**(주석 `:138-143`).
루프 `while (i < template.length)`(`:151`), `ch = template[i]`(`:152`) — **UTF-16 code unit 단위 인덱싱**. 상태: `current`(누적), `inToken`(bool), `quote`(`'"'`|`"'"`|null).

인용 안(`quote` truthy, `:153-170`):
- `:154-158` `ch==='\\' && quote==='"' && i+1<len` → `current += template[i+1]; i+=2` (이중인용 내 백슬래시 이스케이프, **다음 char 리터럴 추가**).
- `:159-166` `ch===quote` → `quote=null; i++; inToken=true`(인용 닫아도 **토큰은 열린 채** — `a"b"c`→`abc` 주석 :162).
- `:167-169` else → `current += ch; i++`.

인용 밖(`:172-198`):
- `:172-177` `ch==='"' || ch==="'"` → `quote=ch; inToken=true; i++`.
- `:179-184` `ch==='\\' && i+1<len` → `current += template[i+1]; inToken=true; i+=2`(인용 밖 백슬래시 이스케이프).
- `:186-194` **`/\s/.test(ch)`** → 토큰 경계: `inToken`이면 `tokens.push(current); current=''; inToken=false`; `i++`.
  - **함정 (whitespace 집합):** `/\s/`는 §1.2와 동일 JS `\s`. **Rust `char::is_whitespace()`와 발산**(특히
    U+FEFF·U+0085·U+00A0). BMP 밖 char는 `template[i]`가 **lone surrogate**를 주므로 `/\s/`는 항상 false →
    surrogate는 토큰 문자로 취급. Rust `chars()` 순회는 code point를 주므로 **astral char에서 인덱싱·경계 발산**.
    포트는 **UTF-16 code unit 순회 + JS `\s` 손정의**로 못박아야 오라클 일치.
- `:196-198` else(일반 char) → `current += ch; inToken=true; i++`.

종료(`:201-207`): `if (quote) return { ok:false, error:'Unclosed quote in command template.' }`; `if (inToken) tokens.push(current)`; `return { ok:true, tokens }`.
- **주의:** 빈 인용 `""`/`''`는 `inToken=true`로 만들지만 `current`는 `''` → **빈 문자열 토큰이 push된다**(예 `"" foo`→`['', 'foo']`). §1.4에서 `!binary` 트리거.
- 에러는 **throw 아님, `ok:false` union** 반환.

### 1.4 `planCustomCommand` — 정확 시맨틱
- `:224-227` `tokenized = tokenizeCustomCommandTemplate(template); if (!tokenized.ok) return { ok:false, error:tokenized.error }`(에러 전파).
- `:228-230` `if (tokens.length === 0) return { ok:false, error:'Custom command is empty.' }`.
- `:231` `const [binary, ...rest] = tokenized.tokens`.
- `:232-234` `if (!binary) return { ok:false, error:'Custom command must start with a binary name.' }`(빈-문자열 binary 방어, §1.3 빈 인용 케이스).
- `:236-239` `substitute = (token) => token.includes('{prompt}') ? token.split('{prompt}').join(prompt) : token`
  — **`split(literal).join()` = replaceAll 손짜기**(정규식 회피). `{prompt}` 여러 개면 전부 치환.
- `:240` `usesPlaceholder = tokenized.tokens.some(t => t.includes('{prompt}'))`.
- `:241-248` placeholder 있으면 `{ ok:true, binary:substitute(binary), args:rest.map(substitute), stdinPayload:null }`.
- `:249` 없으면 `{ ok:true, binary, args:rest, stdinPayload:prompt }` — **prompt를 stdin으로**(`claude -p` 미러, 주석 :216-221).
- `CUSTOM_PROMPT_PLACEHOLDER='{prompt}'`(`:132`)는 `.includes`/`.split` literal — case-sensitive.

### 1.5 `truncateDiffForPrompt` — **byte-budget 실체 해부 (silent-truncation 클래스, 최우선)**

**단위의 진실:** 상수명은 `STAGED_DIFF_BYTE_BUDGET`이고 마커도 `bytes omitted`라 하지만 **전 경로가 UTF-16
code unit(`String.length`)로 잰다.** byte 카운팅은 **어디에도 없다**. ASCII에서만 byte==code unit. 멀티바이트
diff(한글/이모지/CJK)에서:
- JS `'가'.length === 1`, UTF-8 3 bytes, scalar 1.
- 이모지 `'😀'.length === 2`(surrogate pair), UTF-8 4 bytes, scalar 1.
→ Rust `s.len()`(UTF-8 byte)·`s.chars().count()`(scalar) **어느 쪽도 오라클과 다른 컷 위치**를 낸다. 바이트-포-바이트
일치는 `s.encode_utf16().count()` + UTF-16 인덱스 슬라이스로만 가능. **오라클은 100% ASCII**(`test.ts:38,43,50,62,70`)라 이 발산을 **침묵**한다 → **추가 핀 필수**(비-ASCII diff 케이스).

`truncateDiffForPrompt`(`:114-130`):
- `:118` `if (diff.length <= budget) return diff`(UTF-16 length 비교). 마커 없이 원문 반환.
- `:121` `sections = splitDiffIntoFileSections(diff)`.
- `:122-124` `if (sections.length <= 1) return clipSectionOnLineBoundary(diff, budget)`(파일 경계 못 찾음 → 통짜 클립).
- `:125-129` else `allocations = allocateBudgetFairly(sections.map(s=>s.length), budget)`; 각 섹션을 `clipSectionOnLineBoundary(section, alloc[i])`로 클립 후 **`.join('')`**(빈 구분자, byte-for-byte 재조립).

`splitDiffIntoFileSections`(`:41-55`) — **정규식 아님, `indexOf` 루프**:
- `boundary = '\ndiff --git '`(`:42`). `start=0`, `next=diff.indexOf(boundary)`.
- 루프: `sections.push(diff.slice(start, next+1))` — **경계의 `\n`을 현재 섹션에 포함**(다음 섹션은 `diff --git`부터), `start=next+1`, `next=diff.indexOf(boundary, start)`.
- 종료 후 `sections.push(diff.slice(start))`. **첫 섹션은 첫 파일 앞 프리앰블 포함**(첫 헤더가 offset 0이면 첫 섹션은 `''` 가능? — `indexOf('\ndiff --git ')`는 문두 헤더를 안 잡음, 문두가 `diff --git`이면 boundary 없이 통짜 1섹션). concat = 원문(byte-for-byte 주석 :38-40).

`clipSectionOnLineBoundary(section, limit)`(`:59-81`) — 한 섹션을 line 경계에서 클립 + 마커:
- `:60-62` `if (section.length <= limit) return section`.
- `:63-65` `if (limit <= 0) return ''`.
- `:67` `markerFor = (omitted) => \`\n...(diff truncated, ${omitted} bytes omitted)\n\``.
- `:68` `let marker = markerFor(section.length)` — **omitted 자리에 전체 length를 넣어 마커 길이 상한 추정**(실제 수는 뒤에서 재계산).
- `:69-71` `if (marker.length >= limit) return marker.slice(0, limit)` — 마커조차 안 들어가면 **마커를 limit까지 잘라 반환**(diff 본문 0).
- `:75` `const target = limit - marker.length` — 마커 자리 확보 후 남는 예산.
- `:76` `const lineBreak = section.lastIndexOf('\n', target)` — target **이하** 마지막 `\n` 위치(UTF-16).
- `:77` `const cut = lineBreak > target / 2 ? lineBreak : target` — **한 줄이 너무 길면(newline이 예산 절반 이하 위치) 줄 경계 포기하고 target에서 하드 컷**(예산 절반 초과 위치의 newline만 채택).
- `:78` `const omitted = section.length - cut`.
- `:79` `marker = markerFor(omitted)` — **실제 omitted 수로 마커 재생성**(자릿수 변화로 마커 길이 변동 가능).
- `:80` `return \`${section.slice(0, Math.min(cut, Math.max(0, limit - marker.length)))}${marker}\`` — 재계산된
  마커 길이 반영해 **`min(cut, limit - marker.length)`로 본문 재-클립**(마커 자릿수가 늘면 본문을 더 깎아 총합 ≤ limit 유지).
  - **주의:** 마커는 `\n`으로 시작·끝. 본문 slice 뒤 마커가 붙으므로 최종 문자열은 `…본문\n...(diff truncated, N bytes omitted)\n`.

`allocateBudgetFairly(sizes, budget)`(`:87-109`) — **water-filling** 공정 분배:
- `:88` `alloc = [0,0,…]`(sizes.length). `:89` `active = 모든 인덱스`. `:90` `remaining = budget`.
- `:91-107` `while (active.length>0 && remaining>0)`: `share = Math.floor(remaining/active.length)`(`:92`);
  `if (share===0) break`(`:93-95`); 각 active `i`에 대해 `need = sizes[i]-alloc[i]`, `grant = min(need, share)`,
  `alloc[i]+=grant`, `remaining-=grant`, `grant<need`면 `stillActive.push(i)`(`:97-105`); `active=stillActive`.
- 결과: 딱 맞는 파일은 need만큼 받고 slack을 못 채운 파일에 재분배 → **거대 생성파일이 소형 human diff를
  굶기지 않음**(주석 :83-86, 테스트 :69-79). `Math.floor`·정수 산술 — Rust `usize`/정수 나눗셈으로 정확 재현.

**트렁케이션 계약 요약(포트 결정):** ① 컷은 **line 경계 우선, 초장문 라인은 하드 컷**. ② **마커를 항상 append**
(silent 아님 — flag은 마커 텍스트 자체). ③ 마커 문구 `\n...(diff truncated, N bytes omitted)\n` **verbatim**, N은
UTF-16 code unit 델타. ④ 다중 파일은 water-fill 분배 후 각자 클립·`''` join. ⑤ **"bytes"는 미스노머** — 단위
결정을 Codex에 올려야(§오픈퀘스천). ⑥ `budget` 파라미터 기본값 `STAGED_DIFF_BYTE_BUDGET`.

---

## 2. `pull-request-generation.ts` (175L) — PR 프롬프트 조립 + LLM JSON 파싱(garbage-in)

### 2.1 공개 표면
- `type PullRequestDraftContext = { branch: string|null; base: string; branchChangedByPreparation: boolean; currentTitle: string; currentBody: string; currentDraft: boolean; commitSummary: string; changeSummary: string; patch: string }` (`:3-13`).
- `type GeneratedPullRequestFields = { base: string; title: string; body: string; draft: boolean }` (`:15-20`).
- `buildPullRequestFieldsPrompt(context: PullRequestDraftContext, customPrompt: string): string` (`:30-85`) — 순수.
- `parseGeneratedPullRequestFields(raw: string, fallback: Pick<PullRequestDraftContext,'base'|'currentTitle'|'currentBody'|'currentDraft'>): GeneratedPullRequestFields` (`:151-175`) — 순수, **단 throw 가능**(§2.4).
- **비-export(내부):** `limitSection`(`:22-28`), `stripJsonFence`(`:87-99`), `getJsonFenceBody`(`:101-113`),
  `getLineBreakEnd`(`:115-124`), `getBodyEndBeforeClosingFence`(`:126-135`), `startsWithAsciiIgnoreCase`(`:137-149`).
- **import:** `truncateDiffForPrompt` from `./commit-message-prompt`(`:1`) — §1.5 재사용.

### 2.2 `limitSection` (`:22-28`) — char-budget 트렁케이션 (§1.5와 별개, 더 단순)
- `if (value.length <= maxChars) return value`(`:23`). else `omitted = value.length - maxChars`(`:26`);
  `return \`${value.slice(0, maxChars)}\n\n[truncated: ${omitted} characters omitted]\``(`:27`).
- **line 경계 무시·통짜 slice** (§1.5의 정교한 컷과 다름). **`.length`/`.slice` = UTF-16** → "characters"도
  미스노머(scalar 아님). ASCII에서만 정확. maxChars: 8000(commit/changed summary), 4000(custom prompt).

### 2.3 `buildPullRequestFieldsPrompt` — 정확 시맨틱
- `:34-65` `base` = 문자열 배열 `.join('\n')`. **verbatim 재현 대상** 룰 블록(`:36-47`), 그리고:
  - `:49` `Head branch: ${context.branch ?? '(detached)'}` — **`?? '(detached)'`**(null/undefined만, 빈 문자열은 유지).
  - `:50` `Current base: ${context.base}`.
  - `:51` `Current title: ${context.currentTitle || '(empty)'}` — **`||`**(빈 문자열도 `(empty)`).
  - `:52` `Current description: ${context.currentBody || '(empty)'}`.
  - `:53` `Current draft: ${context.currentDraft ? 'true' : 'false'}`.
  - `:56` `limitSection(context.commitSummary || '(none)', 8_000)`.
  - `:59` `limitSection(context.changeSummary || '(none)', 8_000)`.
  - `:62-64` `` '```diff', truncateDiffForPrompt(context.patch), '```' `` — **default budget으로 patch truncate**.
- `:67` `trimmedPrompt = customPrompt.trim()`(JS trim, §1.2 함정).
- `:68-75` 빈 suffix면 `[base, '', 'Final output requirement:', 'Return compact JSON only with keys base, title, body, and draft. No prose or code fences.'].join('\n')`.
- `:76-84` 비면 base + `'', 'Additional user prompt:', limitSection(trimmedPrompt, 4_000), '', 'Final output requirement:', '…'`.

### 2.4 `parseGeneratedPullRequestFields` — **garbage-in / transient≠garbage 계약 (최우선)**
- `:155` `const parsed = JSON.parse(stripJsonFence(raw)) as unknown` — **malformed JSON이면 `JSON.parse`가
  `SyntaxError` throw**. 잡지 않음 → **호출자에게 전파**. 즉 "완전히 깨진 LLM 출력"은 **빈 필드로 삼키지 않고
  예외**로 신호(retry). 이게 transient≠garbage: 정상 JSON이면 누락 필드를 fallback으로 메우되, JSON 자체가
  깨지면 throw.
- `:156-158` `if (!parsed || typeof parsed !== 'object') throw new Error('Expected a JSON object.')`.
  - JSON `null`→`parsed=null`→`!parsed` true→throw. `42`/`"str"`/`true`→`typeof!=='object'`→throw.
    (`[]`는 `typeof==='object'`라 통과 → record로 취급, 필드 없음 → 전부 fallback.)
- `:159` `const record = parsed as Record<string, unknown>`.
- `:160` `base = typeof record.base === 'string' ? record.base.trim() : fallback.base`.
- `:161-164` `title = (typeof record.title === 'string' && record.title.trim()) ? record.title.trim().replace(/[.]+$/g, '') : fallback.currentTitle.trim()`.
  - **정규식 `/[.]+$/g`**: trailing `.` 연쇄 제거. 스파이(§오라클)는 이 패턴을 **안 막음**(source가 `\r\n`도 `[\s\S]`도 아님) → 오라클상 허용. 정책상 손짜기(`trim_end_matches('.')`).
  - **주의:** `record.title.trim()`이 falsy(빈/공백)면 fallback로 감. fallback도 `.trim()`.
- `:165-166` `body = typeof record.body === 'string' ? record.body.replace(/\s+$/g, '') : fallback.currentBody`.
  - **`/\s+$/g` = trailing whitespace strip만**(leading 유지, `.trim()` 아님). **JS `\s` 집합**(§1.2) — Rust `trim_end` 발산 주의. fallback.currentBody는 **가공 없이 그대로**.
- `:167` `draft = typeof record.draft === 'boolean' ? record.draft : fallback.currentDraft`.
- `:169-174` `return { base: base || fallback.base, title: title || 'Update project files', body, draft }`.
  - **`base || fallback.base`**: base가 빈 문자열(예 `record.base==='   '`.trim()==='')이면 fallback.
  - **`title || 'Update project files'`**: title이 빈 문자열이면 하드코딩 default. (fallback.currentTitle도 빈 경우 여기서 잡힘.)

**`stripJsonFence`(`:87-99`) — fence·brace 추출:**
- `:88` `text = raw.trim()`.
- `:89-92` `fencedBody = getJsonFenceBody(text); if (fencedBody !== null) text = fencedBody.trim()`.
- `:93-97` `start = text.indexOf('{'); end = text.lastIndexOf('}'); if (start !== -1 && end > start) return text.slice(start, end+1)`.
- `:98` else `return text`(중괄호 못 찾으면 원문 → `JSON.parse`가 throw할 가능성).

**`getJsonFenceBody`(`:101-113`) — 손짜기 fence 검출(스파이 강제, 정규식 금지):**
- `:102` `bodyStart = getLineBreakEnd(text, 3)` — index 3(```` ``` ```` 직후)이 `\n`/`\r\n`인지.
- `:103-105` null이고 `startsWithAsciiIgnoreCase(text, '```json', 0)`이면 `bodyStart = getLineBreakEnd(text, 7)`(```` ```json ```` 직후).
- `:106-108` `if (bodyStart === null || !text.endsWith('```')) return null`.
- `:110-112` `closeStart = text.length - 3`; `bodyEnd = getBodyEndBeforeClosingFence(text, closeStart)`; `return bodyEnd === null ? null : text.slice(bodyStart, bodyEnd)`.

**`getLineBreakEnd`(`:115-124`)**: `code = text.charCodeAt(index)`; 10→`index+1`; 13→`text.charCodeAt(index+1)===10 ? index+2 : index+1`; else null. (LF/CRLF/CR 인식.)

**`getBodyEndBeforeClosingFence`(`:126-135`)**: `previousCode = charCodeAt(closeStart-1)`; 10→`charCodeAt(closeStart-2)===13 ? closeStart-2 : closeStart-1`; 13→`closeStart-1`; else null. (닫는 fence 앞 CRLF/LF/CR 벗김.)

**`startsWithAsciiIgnoreCase`(`:137-149`) — ASCII-only case-fold 손짜기:**
- `:138-140` 범위 가드. `:141-147` 각 char `charCodeAt`, `A`-`Z`(65-90)면 `+32`(소문자화) 후 비교. **ASCII 전용** —
  MEMORY의 `to_lowercase vs to_ascii_lowercase` 함정 그대로. Rust는 `eq_ignore_ascii_case` 계열, **유니코드 케이스폴드 금지**.

**garbage-in 표(계약):**

| 입력 | 결과 |
|---|---|
| malformed JSON (`{bad`, ``` ``` ```만) | **`JSON.parse` SyntaxError throw** |
| `null`/숫자/문자열/불린 top-level | **`throw 'Expected a JSON object.'`** |
| `{}` (빈 오브젝트) | 전부 fallback: `{base:fallback.base, title:fallback.currentTitle.trim()||'Update project files', body:fallback.currentBody, draft:fallback.currentDraft}` |
| `{"title":""}` | title 빈→`fallback.currentTitle.trim()`; base/body/draft fallback (테스트 :86-95) |
| `{"title":"x."}` | `title:'x'`(trailing dot strip) |
| `{"body":"a  \n"}` | `body:'a'`(trailing ws strip, leading 유지) |
| fenced `` ```json\n{…}\n``` `` | fence 벗기고 파싱 (테스트 :50-62) |
| `` ```JSON\r\n{…}\r\n``` `` | 대문자 태그·CRLF도 손짜기로 처리 (테스트 :64-84) |

---

## 3. `commit-message-generation.ts` (84L) — 신(新) 구조화 커밋 프롬프트 + subject/body 분리

### 3.1 공개 표면
- `type CommitMessageDraftAgent = TuiAgent | 'custom'` (`:4`) — `TuiAgent`는 `./types` 타입-온리 import(`:2`).
- `type CommitMessageDraftContext = { branch: string|null; stagedSummary: string; stagedPatch: string }` (`:6-10`).
- `type CommitMessageDraftOptions = { agentId: CommitMessageDraftAgent; model: string; thinkingLevel?: string; customPrompt?: string; customAgentCommand?: string }` (`:12-18`).
- `type GeneratedCommitMessage = { subject: string; body: string; message: string }` (`:20-24`).
- `buildCommitMessagePrompt(context: CommitMessageDraftContext, customPrompt: string): string` (`:34-70`) — 순수.
- `splitGeneratedCommitMessage(message: string): GeneratedCommitMessage` (`:72-84`) — 순수.
- **비-export:** `limitSection`(`:26-32`, §2.2와 동일 로직 복제본).
- **import:** `cleanGeneratedCommitMessage`, `truncateDiffForPrompt` from `./commit-message-prompt`(`:1`); `type TuiAgent` from `./types`(`:2`, 타입-온리).

### 3.2 `buildCommitMessagePrompt` (`:34-70`)
- `:39-42` `patch = context.stagedPatch.trim() ? truncateDiffForPrompt(context.stagedPatch) : '(diff omitted — too large to read; infer the change from the staged file list above)'`.
  - **`.trim()`으로 비어있음 판정하되 truncate는 원본 `stagedPatch`(trim 안 한 값)에 적용**(주의: 판정과 대상 불일치).
  - patch 없음→고정 문구(테스트 :41-53).
- `:43-63` `base` = 배열 `.join('\n')`. **verbatim 룰 블록**(`:44-52`, §1.2 BASE와 문구 다름). `:54` `Branch: ${context.branch ?? '(detached)'}`; `:57` `limitSection(context.stagedSummary, 6_000)`; `:60-62` `` '```diff', patch, '```' ``.
- `:65` `trimmedPrompt = customPrompt.trim()`. `:66-69` 빈→base; 비→`[base,'','Additional user prompt:', limitSection(trimmedPrompt, 4_000)].join('\n')`.

### 3.3 `splitGeneratedCommitMessage` (`:72-84`) — subject 정규화 + body 보존
- `:73` `normalized = cleanGeneratedCommitMessage(message)`(§4 의존).
- `:74` `firstNewline = normalized.indexOf('\n')` — **스파이 강제: `.split('\n')` 금지**(테스트 :69-82). indexOf/slice로 첫 줄만.
- `:75` `subjectLine = firstNewline === -1 ? normalized : normalized.slice(0, firstNewline)`.
- `:76` `subject = subjectLine.trim().replace(/[.]+$/g, '').slice(0, 72).trimEnd()`.
  - `.trim()`(JS ws) → `/[.]+$/g` trailing dot strip(스파이 미차단, 정책상 손짜기) → **`.slice(0,72)` = UTF-16 72 code unit**(astral char 2칸, 경계 쪼갤 위험) → `.trimEnd()`(72 컷 후 꼬리 공백 제거).
  - **함정:** `.slice(0,72)`가 surrogate pair 중간을 자르면 lone surrogate 발생 가능. Rust는 char/byte 경계라 다른 컷. **오라클 미커버**(테스트 subject 전부 ASCII·72 이하).
- `:77` `body = firstNewline === -1 ? '' : normalized.slice(firstNewline+1).trim()`.
- `:78` `safeSubject = subject.length > 0 ? subject : 'Update project files'`.
- `:79-83` `return { subject: safeSubject, body, message: body.length > 0 ? \`${safeSubject}\n\n${body}\` : safeSubject }`.

---

## 4. `commit-message-agent-output.ts` (198L) — **배치 포함 필수(오라클 있음), 프롬프트의 "제외" 근거 반박**

> 프롬프트는 "테스트 없어 제외"라 했으나, `cleanGeneratedCommitMessage`(:4)와 `excerptAgentFailureOutput`(:125)은
> `commit-message-prompt.ts:20-23` re-export를 통해 **`commit-message-prompt.test.ts:82-248`에서 완전 커버**된다.
> 자체 `.test.ts`가 없을 뿐 오라클은 있다. `splitGeneratedCommitMessage`(§3.3)가 `cleanGeneratedCommitMessage`에
> 의존하므로 **커밋 클러스터 포팅에 이 함수는 불가결**. → **`cleanGeneratedCommitMessage`는 포함·오라클-검증**.

### 4.1 공개 표면
- `cleanGeneratedCommitMessage(raw: string): string` (`:4-30`) — 순수. **오라클: cmp.test.ts:82-137**.
- `stripAnsiControlSequences(value: string): string` (`:91-103`) — 순수. (excerpt 헬퍼, export.)
- `excerptAgentFailureOutput(stdout: string, stderr: string): string | null` (`:125-159`) — 순수. **오라클: cmp.test.ts:139-248**.
- **비-export:** `normalizeGeneratedCommitMessageLineFeeds`(`:32-51`), `findEnclosingCommitMessageFenceBody`(`:53-79`),
  `isCommitFenceInfoCharacter`(`:81-89`), `stripAnsiIfPresent`(`:105-107`), 상수 `FAILURE_EXCERPT_*`(`:111-118`),
  `composeTwoEndExcerpt`(`:161-168`), `truncateExcerptPart`(`:170-172`), `collectExcerptLines`(`:174-185`),
  `collectExcerptLinesFromEnd`(`:187-197`).
- **import 0.**

### 4.2 `cleanGeneratedCommitMessage` — 정확 시맨틱 (no-regex 스파이가 여기를 겨눔)
- `:7` `text = normalizeGeneratedCommitMessageLineFeeds(raw).trim()` — **CRLF→LF 손짜기**(§4.3) 후 JS trim.
- `:12-18` 첫 줄 preamble 제거: `firstNewline = text.indexOf('\n')`; 있으면 `firstLine=text.slice(0,firstNewline)`;
  `if (/^(generating|thinking)\b/i.test(firstLine) || /^[.…]+$/.test(firstLine.trim())) text = text.slice(firstNewline+1).trim()`.
  - **정규식 `.test`×2**: `/^(generating|thinking)\b/i`(대소문자 무시, 단어경계), `/^[.…]+$/`(점·ellipsis 전용 줄).
    **스파이 미차단**(스파이는 `.replace` `\r\n`과 `.match` `[\s\S]`만 감시). 정책상 손짜기 권장: `\b`·`/i`·`…`(U+2026) 정확 재현 주의.
- `:20-23` `fenced = findEnclosingCommitMessageFenceBody(text); if (fenced !== null) text = fenced.trim()` — **fence 손짜기**(§4.3, 스파이 강제).
- `:27` `text = text.replace(/^(\s*)(?:[-*•●]\s+|\d+[.)]\s+)/, '$1').trim()` — **leading list-marker 하나 제거**.
  - **정규식 `.replace`이지만 스파이 미차단**(source가 `\r\n` 아님). `\s`=JS ws, 불릿 `[-*•●]`(hyphen/asterisk/U+2022/U+25CF), 또는 `\d+[.)]`(숫자+`.`/`)`) 뒤 `\s+`. 캡처 `$1`(선행 공백) 보존. 정책상 손짜기.
- `:29` `return text`.

### 4.3 `cleanGeneratedCommitMessage` 손짜기 헬퍼 (스파이 강제 지점)
**`normalizeGeneratedCommitMessageLineFeeds`(`:32-51`) — CRLF→LF, `.replace(/\r\n/g)` 금지:**
- `:33-36` `crlfStart = value.indexOf('\r\n'); if (crlfStart === -1) return value`(CRLF 없으면 원문).
- `:38-41` 첫 청크 `value.slice(0, crlfStart)` + `'\n'`, `chunkStart = crlfStart+2`, 다음 `indexOf('\r\n', chunkStart)`.
- `:43-48` while로 `slice(chunkStart, crlfStart)+'\n'` 누적.
- `:50` `return \`${normalized}${value.slice(chunkStart)}\``. **`\r\n`만 변환, 고립 `\r`은 유지**(§4.4 excerpt와 다름).

**`findEnclosingCommitMessageFenceBody`(`:53-79`) — `[\s\S]` match 금지:**
- `:54-55` `if (!text.startsWith('```')) return null`.
- `:58-64` `headerEnd=3`; ```` ``` ```` 뒤 info 문자열이 `isCommitFenceInfoCharacter`만 포함하는지 검사하며 `\n`(code 10)까지 전진, 아니면 null.
- `:66-68` `if (headerEnd >= text.length) return null`.
- `:70-73` `closingFenceStart = text.length-3`; `if (closingFenceStart <= headerEnd || !text.endsWith('```')) return null`.
- `:74-76` `if (text.charCodeAt(closingFenceStart-1) !== 10) return null`(닫는 fence 앞이 `\n`이어야).
- `:78` `return text.slice(headerEnd+1, closingFenceStart-1)`(fence body, 앞뒤 `\n` 제외).

**`isCommitFenceInfoCharacter`(`:81-89`)**: `[0-9A-Za-z]` + `-`(45) + `_`(95). info 태그 허용 문자셋.

### 4.4 `excerptAgentFailureOutput` + `stripAnsiControlSequences` — 스코프 경계 주의
이 두 함수는 **오라클이 있으나(cmp.test.ts:139-248, 15케이스)** "커밋/PR **생성**" 클러스터가 아니라 **에이전트
실패 stderr/stdout excerpt**(토스트/영속 표시용). 프롬프트 스코프 문언("AI commit/PR generation prompt+parse")의
경계에 걸친다. **결정 필요**(§오픈퀘스천): 같은 크레이트에 넣을지, 별도 배치로 뺄지. 알고리즘 요지(포팅 시):
- `:129` `source = /\S/.test(stderr) ? stderr : stdout`(stderr 우선, stdout은 prompt echo 위험이라 fallback만).
- `:130-132` source에 non-ws 없으면 `return null`.
- `:134-146` `source.length <= 8192`(`FAILURE_EXCERPT_SCAN_WINDOW`)면 전 라인 수집(`collectExcerptLines`),
  `<= HEAD+1`(3)줄이면 `join(' ')` 후 `truncateExcerptPart(_, 240)`, 아니면 head 2줄+마지막 1줄 `composeTwoEndExcerpt`.
- `:148-158` 초과면 head window(앞 8192)에서 2줄, tail window(뒤 8192)에서 1줄, 조합.
- `collectExcerptLines`(`:174-185`)/`FromEnd`(`:187-197`): **`text.split(/\r\n|\r|\n/)`**(고립 `\r`도 경계 — 진행바
  프레임), 각 줄 `stripAnsiIfPresent().trim()`, 빈 줄 스킵.
- `stripAnsiControlSequences`(`:91-103`): **정규식 `.replace`로 CSI/OSC 제거**. 이 함수는 스파이 감시 대상 아님
  (excerpt 테스트엔 스파이 없음). Rust 포팅 시 정책상 손짜기 or 신중한 정규식.
- `truncateExcerptPart`(`:170-172`): `value.length > budget ? \`${value.slice(0,budget).trimEnd()}…\` : value`.
  budgets: HEAD 100, TAIL 130, SINGLE 240(`:116-118`). **UTF-16 slice + `…`(U+2026) 마커**.

---

## 5. 오라클 케이스별 (3개 `.test.ts` 전수, input→expected→crux)

### 5.1 `commit-message-prompt.test.ts` (329L)
`buildCommitPrompt`:
- `:17-22` `('diff --git a/foo b/foo\n+hello','')` → diff·`+hello`·`First line: imperative mood` 포함 → **{{DIFF}} 치환 확인**.
- `:24-28` `('diff','Use Conventional Commits.')` → `Additional user prompt:` 포함 & `endsWith('Use Conventional Commits.')` → **suffix 말미 append**.
- `:30-33` `('diff','   \n  ')` → `Additional user prompt:` **불포함** → **공백-only suffix는 trim 후 append 안 함**(§1.2 `.trim()` 트랩 핀).

`truncateDiffForPrompt`:
- `:37-40` `'line\n'×10` → 그대로 반환 → **예산 내 무변경**(마커 없음).
- `:42-47` `'line\n'×(40000+100)` → 길이 감소 & `/diff truncated, \d+ bytes omitted/` 매치 → **초과 시 마커 append**. ⚠**byte-budget 케이스, ASCII만** — §1.5 UTF-16 발산 미커버.
- `:49-57` `'keep this line\n'×40, budget=95` → 마커 앞 body의 모든 줄이 `'keep this line'` → **line 경계 컷, mid-line 금지**.
- `:59-67` 20파일×`'+x\n'×200`, budget=120 → `result.length <= 120` → **tight budget 총량 상한**. ⚠**분배+마커 재-클립이 총합 ≤ budget 유지**(§1.5 `:80` min 로직 핀).
- `:69-79` hugeFile(`+x`×5000)+smallFile, budget=1000 → `a/src/app.ts`·`const meaningful = true` 포함 & huge는 `diff truncated` → **water-fill 공정분배로 소형 human diff 생존**(§1.5 `allocateBudgetFairly` 핀).

`cleanGeneratedCommitMessage`(re-export, cmao 정의):
- `:83-85` `'  feat: hello  \n'` → `'feat: hello'` → **JS trim**.
- `:87-90` ```` '```\nfeat: hello\n```' ```` → `'feat: hello'` → **fence(태그 없음) 벗김**.
- `:92-95` ```` '```text\nfix: bug\n```' ```` → `'fix: bug'` → **info 태그 fence 벗김**.
- `:97-100` `'Generating…\nfeat: hello world'` → `'feat: hello world'` → **preamble 줄 제거**(`/^generating\b/i`).
- `:102-104` `'feat: a\r\nbody line\r\n'` → `'feat: a\nbody line'` → **CRLF→LF 손짜기**.
- `:106-125` **no-regex 스파이 케이스**: 대형 CRLF+fence 입력 → `startsWith('feat: large output\nbody line')` & `endsWith('body line')` & `\r\n` 없음; **`replace`가 `/\r\n/` source로 안 불림** & **`match`가 `[\s\S]` source로 안 불림** → **CRLF·fence 반드시 손짜기**(§4.3). ⚠핵심 오라클.
- `:127-132` `'● Add Copilot entry…'`→`'Add Copilot entry…'`, `'1. Add numbered entry'`→`'Add numbered entry'` → **leading list-marker 하나 제거**(`:27` 정규식, 정책상 손짜기).
- `:134-136` `'   \n\t'` → `''` → **공백-only는 빈 문자열**.

`excerptAgentFailureOutput`(re-export, **스코프 경계** §4.4):
- `:156-160` Codex stderr(tail 에러) → head+`…`+tail 130 slice → **양단 excerpt, tail 우선 예산**.
- `:171-175` pi auth stderr → head+`…`+tail → **head-anchored도 tail 보임**.
- `:177-184` stdout에 prompt echo + stderr 짧음 → stderr만 → **stderr 우선, stdout echo 무시**.
- `:186-190` stdout only, stderr 공백 → stdout → **stderr 공백 시 stdout fallback**.
- `:192-194` 둘 다 공백 → `null` → **null 반환**.
- `:196-198` `'one\ntwo\nthree\n'` → `'one two three'` → **≤3줄 ellipsis 없이 join**.
- `:200-204` `'401: {"message":…}'` → 그대로 → **JSON 파싱/언랩 안 함**.
- `:206-215` ANSI CSI+OSC → `'Error: no payment method'` → **ANSI/OSC 제거**.
- `:217-221` `'Fetching 50%\rFetching 100%\rConnection error.'` → 공백 join → **고립 `\r` 경계**.
- `:223-225` `'one\r\ntwo\r\n'` → `'one two'` → **CRLF 처리**.
- `:227-231` `'Retrying request…\n'×10` → 2회만 → **반복 줄 축약**(`composeTwoEndExcerpt` head==tail 중복 제거).
- `:233-236` `'Error: '+'m'×300` → `'Error: '+'m'×233+'…'` → **단일 초장문 240 budget 컷**. ⚠UTF-16 slice.
- `:238-243` 대형 멀티라인 → `'first line filler line … last: operative error'` → **head/tail window**.
- `:245-247` `'x'×20000` → `'x'×100+'…'` → **거대 단일라인 100 budget**.

`tokenizeCustomCommandTemplate`:
- `:251-254` `'claude -p'` → `['claude','-p']` → **공백 split**.
- `:256-259` `'claude --msg "hello world"'` → `['claude','--msg','hello world']` → **이중인용 그룹**.
- `:261-264` `` `agent --json '{"k":"v"}'` `` → `['agent','--json','{"k":"v"}']` → **단일인용 verbatim**.
- `:266-269` `'claude --msg "she said \\"hi\\""'` → `[…,'she said "hi"']` → **이중인용 내 백슬래시 이스케이프**.
- `:271-274` `'foo a"b"c'` → `['foo','abc']` → **인접 인용/비인용 한 토큰**.
- `:276-282` `'claude --msg "no end'` → `ok:false`, error `/unclosed/i` → **미종료 인용 에러(throw 아님)**.
- `:284-287` `'   \t  '` → `{ok:true, tokens:[]}` → **공백-only 빈 토큰 리스트**. ⚠`/\s/` 트랩 핀(§1.3).

`planCustomCommand`:
- `:291-294` `('claude -p','COMMIT MSG')` → `{binary:'claude',args:['-p'],stdinPayload:'COMMIT MSG'}` → **placeholder 없으면 stdin**.
- `:296-299` `('codex exec {prompt}','PROMPT')` → `{binary:'codex',args:['exec','PROMPT'],stdinPayload:null}` → **argv 치환**.
- `:301-305` `'{prompt}'` vs `'"{prompt}"'` 동일 결과 → **인용 무관(no shell, 이중인용 안 함)**.
- `:307-315` `('agent --msg={prompt}','PROMPT')` → `args:['--msg=PROMPT']` → **토큰 내부 임베드 치환**.
- `:317-320` `'   '` → `ok:false` → **빈 템플릿 에러**.
- `:322-328` `'agent "unclosed'` → `ok:false`, `/unclosed/i` → **토크나이저 에러 전파**.

### 5.2 `pull-request-generation.test.ts` (96L)
`buildPullRequestFieldsPrompt`:
- `:25-33` `(context,'Use conventional PR titles.')` → `Return ONLY compact JSON`·`Head branch: feature/pr-details`·`Current base: main`·`Additional user prompt:`·suffix 포함 → **JSON 요청 + context + custom suffix**.
- `:35-46` currentBody가 템플릿(`## Summary…`)일 때 → `preserve its headings…`·`Leave genuinely unknown template items as TODO or unchecked` 포함 → **템플릿 보존 룰 verbatim**.

`parseGeneratedPullRequestFields`:
- `:50-62` `` ```json\n{…"title":"fix: add details."…}\n``` `` → `{base:'main',title:'fix: add details',body:'Summary',draft:true}` → **fence 벗김 + trailing dot strip**.
- `:64-84` **no-regex 스파이**: `` ```JSON\r\n{…}\r\n``` `` → `title:'fix: add details'`; `match`가 `^\`\`\``+`[\s\S]` source로 안 불림 & `replace`가 `\r\n` global source로 안 불림 → **fence·CRLF 손짜기**(§2.4 `getJsonFenceBody`, 대문자 태그 case-fold). ⚠핵심.
- `:86-95` `'{"title":""}'` → `{base:'main',title:'Feature pr details',body:'- Add form',draft:false}` → **누락/빈 값 fallback**(§2.4). ⚠garbage-in 핀. **주의: throw 케이스(malformed JSON, non-object)는 오라클 없음** → 추가 핀 필수.

### 5.3 `commit-message-generation.test.ts` (83L)
`buildCommitMessagePrompt`:
- `:9-25` staged context, suffix `''` → `Branch: feature/commit-drafts`·`Staged files:\nM\tsrc/…`·`Staged patch:\n```diff`·`+hello`·`Use only the staged changes below as context.` 포함 & `Additional user prompt:` 불포함 → **git 검사 대신 staged context 임베드**.
- `:27-39` `branch:null`, suffix `'Use Conventional Commits.'` → `Branch: (detached)`·`Additional user prompt:\nUse Conventional Commits.` → **detached fallback + bounded custom section**.
- `:41-53` `stagedPatch:''` → `Staged files:\nA\thuge.jsonl`·`diff omitted — too large to read` 포함 → **빈 patch면 고정 문구**(§3.2, `.trim()` 판정).

`splitGeneratedCommitMessage`:
- `:56-67` `'Fix source control generation.\n\n- Move planning into main'` → `{subject:'Fix source control generation', body:'- Move planning into main', message:'Fix source control generation\n\n- Move planning into main'}` → **subject trailing dot strip + body 보존 + message 재조립**.
- `:69-82` **no-split-array 스파이**: `'Add generated paste protection\n\n'+('- Explain…\n'×10000).trimEnd()` → `subject:'Add generated paste protection'`, body startsWith/endsWith 확인; **`split('\n')` 안 불림** → **`indexOf('\n')`+slice로 첫 줄만**(§3.3). ⚠핵심.

**오라클 침묵(추가 핀 필수) 종합:**
1. **비-ASCII byte-budget 트렁케이션**(§1.5) — 한글/이모지 diff에서 UTF-16 vs UTF-8-byte vs scalar 컷 위치.
2. **`.replace('{{DIFF}}', diff)`의 `$`-패턴 quirk**(§1.2) — `$&`/`$'` 포함 diff.
3. **`parseGeneratedPullRequestFields` throw 경로**(§2.4) — malformed JSON, non-object(`null`/`42`/`"str"`/`true`), `[]`.
4. **`splitGeneratedCommitMessage`의 `.slice(0,72)` 멀티바이트 subject**(§3.3) — 72 UTF-16 컷이 surrogate/한글 경계 쪼갬.
5. **whitespace/case-fold 발산**(§1.2,1.3,2.4) — `.trim()`/`/\s/`/`/\s+$/`가 U+FEFF·U+00A0·U+0085에서 Rust와 갈림.
6. **`clipSectionOnLineBoundary`의 `limit<=0`·`marker.length>=limit` 극단 경로**(§1.5 `:63,69`) — 오라클은 중간 budget만.

---

## 6. Codex 교차검증용 오픈 퀘스천 (통합)

1. **byte-budget 단위 결정 (최우선):** `STAGED_DIFF_BYTE_BUDGET`/마커가 "byte"라 부르지만 실제는 **UTF-16 code
   unit**. Rust 포트는 (a) `encode_utf16().count()`+UTF-16 슬라이스로 **오라클 바이트-포-바이트 재현**, (b) 진짜
   UTF-8 `len()` byte 예산으로 **의미 수정**(이름값 일치), (c) `chars().count()` scalar 중 무엇? 마커 문구
   `${N} bytes omitted`의 N 단위도 이에 종속. **비-ASCII diff 핀을 추가**하고 단위를 못박아야. (`§1.5`)
2. **마커/flag 계약:** 트렁케이션은 **silent 아님** — `\n...(diff truncated, N bytes omitted)\n` 마커를 항상
   append(단일·다중 파일 모두). line 경계 컷·초장문 하드 컷·water-fill 분배·재-클립 min 로직(`:80`)까지 verbatim
   재현 대상. `limit<=0`/`marker.length>=limit` 극단 경로 핀 추가 여부? (`§1.5`)
3. **no-regex 손짜기 범위:** 스파이가 **강제하는 지점**은 오직 ① CRLF 정규화(`normalizeGeneratedCommitMessageLineFeeds`,
   `getLineBreakEnd`), ② fence-body 추출(`findEnclosingCommitMessageFenceBody`, `getJsonFenceBody`),
   ③ `splitGeneratedCommitMessage`의 `split('\n')` 금지. **스파이 미차단이지만 정책상 손짜기**: list-marker
   strip(`cmao:27`), preamble `.test`(`cmao:15`), trailing-dot `/[.]+$/`(prg:163, cmg:76), trailing-ws
   `/\s+$/`(prg:166). 전부 `regex` 크레이트 없이 손짜기 확정? ANSI stripper(`cmao:96`)는? (`§4`)
4. **whitespace/case-fold 술어:** JS `.trim()`·`/\s/`·`/\s+$/` = JS whitespace 집합(U+FEFF 포함, U+0085 미포함
   등)으로 **손 정의**하고, `startsWithAsciiIgnoreCase`(prg:137)·fence info charset(cmao:81)는 **ASCII-only
   case-fold**(`eq_ignore_ascii_case`, `to_lowercase` 금지)로 못박기 — 합의? (`§1.2, §2.4`)
5. **parse-failure 시맨틱:** `parseGeneratedPullRequestFields`는 **malformed/non-object에서 throw**(Result::Err),
   정상 JSON의 누락 필드는 **fallback으로 메움**. Rust 시그니처를 `Result<GeneratedPullRequestFields, ParseError>`로
   내고 throw 케이스를 오라클에 **추가 핀**(malformed, `null`, `42`, `[]`)? transient(재시도) vs garbage(fallback)
   경계를 이 throw로 구현. (`§2.4`)
6. **`.replace('{{DIFF}}', diff)` `$`-quirk:** JS는 replacement의 `$&`/`$'`를 해석 — Rust literal replace는
   안 함. **의도적 발산(literal)**로 가고, quirk-노출 케이스를 문서화? (`§1.2`)
7. **크레이트/모듈 레이아웃:** `suaegi-gen-prompt` 리프 크레이트 하나에 **4개 모듈**
   (`commit_message_prompt`, `pull_request_generation`, `commit_message_generation`, `commit_message_agent_output`).
   `commit_message_agent_output`은 프롬프트가 "제외"라 했으나 **오라클 존재·의존 필수**(§0.1, §4)라 **포함**해야 —
   합의? 단 **`excerptAgentFailureOutput`+`stripAnsiControlSequences`는 커밋/PR 생성 스코프 밖**(에이전트 실패
   excerpt)이라 (a) 같은 크레이트 동거, (b) 별도 배치 분리 중 결정 필요. **`TuiAgent`(cmg:2)는 `types.ts:2442`의
   거대 string-literal union**(`'claude'|'codex'|'pi'|…` 십수 개)이고, 이를 참조하는 `CommitMessageDraftAgent`
   (cmg:4)·`CommitMessageDraftOptions`(cmg:12)는 **포팅 대상 두 순수 함수(`buildCommitMessagePrompt`,
   `splitGeneratedCommitMessage`)가 전혀 소비하지 않는다** — 시그니처에 안 들어감. 따라서 포트는 이 union을
   **끌어오지 말고 생략하거나 최소 스텁**으로 두면 된다(types.ts 전체 의존 회피). 확정? (`§0.1, §3.1, §4.4`)
