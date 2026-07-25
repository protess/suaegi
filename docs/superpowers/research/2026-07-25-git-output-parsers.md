# git 출력 파서 5종 조사: cquoted-path · remote-identity · merge-tree-capability · status-limit · capability-cache

> 2026-07-25. Orca v1.4.150-rc.0 소스를 직접 읽고 `file:line`으로 인용. suaegi는 `crates/…`로 인용. 구현하지 않는다.
> Base: `…/scratchpad/orca-src/src/shared/`. (에이전트 디스패치 회복 후 Explore 서브에이전트 정찰 — read-only라
> 본문을 리드가 저장. 데이터로 취급, 크럭스는 impl 시 리드가 소스 재확인.)
>
> **가장 중요한 트랩:**
> 1. **C-quoted path는 BYTES.** `\NNN`은 8진수(base-8) 단일 바이트; 인접 `\NNN` 런이 하나의 UTF-8 멀티바이트를 이룸.
>    Orca는 모아서 `TextDecoder('utf-8')`로 디코드 → 손실 가능(String, invalid→U+FFFD). Rust는 `Vec<u8>`(정확) vs
>    `from_utf8_lossy`(JS 동형) 선택.
> 2. **remote-URL: host는 소문자화, path는 대소문자 보존.** scp(`git@host:owner/repo`)와 URL(`ssh://`,`https://`)을
>    `://` 유무로 분기, 포트 폐기, `.git`·앞뒤 슬래시 스트립.

## 0. 트랩 클래스 통합

| 트랩 | 위치 | 내용 |
|---|---|---|
| byte-vs-UTF16 | cquoted `:7,8,66` | 순회는 UTF-16 code unit이나 옥탈 런은 바이트→`Uint8Array`→`TextDecoder`. 출력 손실 가능. |
| octal radix=8 | cquoted `:57` | `parseInt(octal,8)` base 8. 최대 3자리(`:51`), 0–255. |
| regex `\s`(Unicode) | remote `:37,61`, merge-tree `:14,17,18,24` | JS `\s`는 유니코드; Rust `regex` 기본 ASCII 아님(유니코드) — `(?-u:\s)` 고려. |
| regex `/i` | merge-tree `:14,17,18,24` | Rust `(?i)`. ASCII 케이스는 `(?i-u)`. |
| host lower / path preserve | remote `:20` vs `:16` | `normalizeRemoteHost=trim().toLowerCase()`; `normalizeRemotePath` 대소문자 미변경. |
| `new URL` 의존 | remote `:45` | WHATWG URL(host 소문자·punycode, 포트 hostname 제외). Rust `url` 크레이트 미세 발산 검증. |
| `localeCompare` | remote `:93` | 로케일 정렬 tie-break → Rust `str::cmp`(코드포인트) 발산 가능(비-ASCII). |
| `Number.isInteger`/`>=0` | status-limit `:9` | float·NaN·음수 거부→DEFAULT. limit=0=무제한. |
| statefulness/동시성 | capability-cache 전체 | 순수 아님 — 가변 캐시 + async single-flight + 시간 retry. Rust 내부 가변성. |
| BOM 보존 | cquoted `:66` | `ignoreBOM:true` → 선두 EF BB BF를 U+FEFF로 보존. |

## 1. git-cquoted-path.ts (75L) — C-quoted 경로 디코딩
- Export: `decodeGitCQuotedPath(value: string): string` (`:1`). PURE.
- `:2` 가드: `length<2 || value[0]!=='"' || value.at(-1)!=='"'`면 원본 반환.
- `:7` 루프 `1..length-2`(감싼 따옴표 제외). `:9–12` 백슬래시 아니면 append.
- `:16` 단문자 escape: `a`→BEL(``), `b`,`f`,`n`,`r`,`t`,`v`, `\\`·`"`→리터럴.
- `:42` default 옥탈 분기: escaped가 `/[0-7]/`이면 옥탈 누적 루프(최대 3자리 `:51`, `parseInt(octal,8)` `:57`), 인접
  `\`+옥탈이면 계속(`:59`), `octalStart=index+2`(`:62`). `:66` `TextDecoder('utf-8',{ignoreBOM:true})`로 바이트
  전체 한 번에 디코드(invalid→U+FFFD, BOM 보존). `:67–68` 옥탈 아닌 미지 escape→escaped만 append(백슬래시 버림).
- 오라클(2): `"\357\273\277name"`→`﻿name`(BOM=EF BB BF, base-8); `"\357\273\277\343\201\202"`→`﻿あ`
  (EF BB BF + E3 81 82=U+3042, 인접 멀티바이트 옥탈 런).
- 중복: status.rs는 `-z`(비-quote NUL)라 무중복. suaegi-git에 cquoted 디코더 없음(grep=0). **NEW.**

## 2. git-remote-identity.ts (103L) — remote URL → identity
- Exports: type `GitRemoteIdentity{canonicalKey,remoteName,remoteUrl}`(`:1`), `normalizeGitRemoteUrl(url):string|null`
  (`:28`), `parseGitRemoteVerboseOutput(stdout):GitRemoteEntry[]`(`:54`), `deriveGitRemoteIdentity(stdout):
  GitRemoteIdentity|null`(`:84`). 전부 PURE.
- 헬퍼: `stripGitSuffix`(`:12` `.git` 끝이면 `slice(0,-4)`), `normalizeRemotePath`(`:16` `replace(/^\/+/,'')
  .replace(/\/+$/,'')` **후** stripGitSuffix — 슬래시 먼저, `.git` 나중, 대소문자 미변경), `normalizeRemoteHost`
  (`:20` `trim().toLowerCase()`), `isLocalFilesystemRemote`(`:24` `/^[A-Za-z]:[\\/]/` Windows), `primaryRemoteSortKey`
  (`:74` upstream→0, origin→1, else→2).
- `normalizeGitRemoteUrl`(`:29–51`): trim; 빈→null; Windows local FS→null; `:37` scp 감지(`://` 미포함 시
  `/^([^@\s:]+@)?([^:\s]+):(.+)$/`); scp면 host=group2 소문자+path=group3 정규화→`host/path`; else `new URL`(`:45`)
  host=hostname 소문자(포트 제외)+path=pathname 정규화. catch→null.
- `parseGitRemoteVerboseOutput`(`:56–69`): `/\r?\n/` split, trim, `(fetch)`로 끝나는 줄만(`:58`),
  `/^(\S+)\s+(.+?)\s+\(fetch\)$/`(`:61`), name·url 둘 다 truthy면 push.
- `deriveGitRemoteIdentity`(`:85–102`): 파스→canonicalKey 매핑→null 필터→`primaryRemoteSortKey` 정렬(동점
  `name.localeCompare` `:93`)→첫 번째.
- 오라클: https+.git/scp/ssh/`https://GitHub.com`→모두 `github.com/example/sample-app`; 다중 세그먼트 path 보존;
  포트 폐기; `git@Git.Company.Test:Team/Sample-App.git`→`git.company.test/Team/Sample-App`(host 소문자+path 보존);
  Windows `C:\`/`C:/`→null. derive: upstream>origin, 단일 origin/mirror는 그 이름.
- 중복: remote.rs `strip_credentials_from_message`(`:203`)는 URL 리댁션(다른 관심사). canonical-key 파서 없음(grep=0).
  **NEW** — `remote.rs`에 얹거나 `remote_identity.rs` 신설.

## 3. git-merge-tree-capability.ts (27L) — 에러 텍스트로 미지원 감지 (버전 파싱 아님)
- Exports: `isUnsupportedMergeTreeWriteTreeError(error:unknown):boolean`(`:11`), `isUnsupportedMergeTreeMergeBaseError`
  (`:22`). PURE. 내부 `getGitErrorText`(`:1`): object 아니면 Error→message else `String(error)`; object면
  message/stderr/stdout string 값들을 `\n` join(`:5–8`).
- writeTree(`:13–19`): 3 정규식 OR(`/i`): ① `/(?:unknown|invalid|unrecognized) option(?::|\s+)[`']?(?:--?)?write-tree
  [`']?(?:\s|$)/i` ② `/unknown rev [`']?--write-tree[`']?(?:\s|$)/i` ③ `/usage:\s*git merge-tree\s+<base-tree>\s+
  <branch1>\s+<branch2>/i`. mergeBase(`:24–26`): `merge-base` 옵션 정규식 1개.
- backtick과 홑따옴표 둘 다 `[`']?`. 앵커 `$`(non-multiline).
- 오라클: unknown-rev-write-tree(stderr)/usage(stdout)/unknown-option-write-tree(Error, 홑따옴표)→true; unrelated
  histories→false; `unknown option \`merge-base'`(backtick)→true, `merge-base failed`→false.
- 중복: 없음(grep=0). **NEW** — 모듈 5의 `isUnsupportedError` 콜백.

## 4. git-status-limit.ts (28L)
- Exports: const `DEFAULT_GIT_STATUS_LIMIT=1000`(`:6`), `resolveGitStatusLimit(value:unknown):number`(`:8`),
  `capGitStatusEntries<T>(entries,limit,previous={}):{entries,didHitLimit?,statusLength?}`(`:14`). PURE 제네릭.
- `resolveGitStatusLimit`(`:9`): `typeof number && Number.isInteger && >=0`면 value, else DEFAULT.
- `capGitStatusEntries`(`:19`): `exceeded=limit>0 && entries.length>limit`(limit=0=무제한); 미초과 AND
  `previous.didHitLimit!==true`면 `{entries}`만; else `{entries: exceeded?slice(0,limit):entries, didHitLimit:true,
  statusLength: max(previous.statusLength??0, entries.length)}`(끈적 보존).
- 오라클: resolve 0→0,25→25,1.5/NaN/-1→DEFAULT; cap `['a','b','c'],2`→`{['a','b'],didHitLimit:true,statusLength:3}`;
  `['a'],2,{didHitLimit:true,statusLength:3}`→유지(statusLength max); `['a','b'],0`→`{['a','b']}`.
- 중복: 없음(grep=0). **NEW.**

## 5. git-capability-cache.ts (115L) — **STATEFUL 캐시(순수 아님) — 이번 defer 후보**
- Exports: const `GIT_CAPABILITY_RETRY_INTERVAL_MS=30*60000`(`:3`), type `GitCapability`(5 리터럴 유니온 `:5`),
  class `GitCapabilityCache`(`:14`). 상태(`:15`): `retryAfterByCapability:Map`, `probesByCapability:Map<cap,Promise>`,
  `supportedCapabilities:Set`.
- `shouldTry(cap,now)`(`:19`): retryAfter 없으면 true; `now<retryAfter`면 false; 만료면 delete 후 true.
  `rememberUnsupported`(`:31`): supported 삭제+retryAfter=now+INTERVAL. `runWithFallback<T>`(`:38`): supported면
  직행; retry 창이면 fallback; 인플라이트 프로브 await; 새 프로브 single-flight. `runPreferredOrFallback`(`:88`):
  await preferred, outcome=`retryAfterByCapability.has?unsupported:supported`, catch시 `!isUnsupportedError`면 rethrow
  else rememberUnsupported+fallback.
- 오라클: retry 경계(`>=`); 동시 2 호출 첫 preferred reject→둘 다 fallback(secondPreferred 0회 = coalescing);
  supported 확정 후 동시 preferred 병렬(직렬화 없음); 나중 unsupported로 지원 드롭+retry.
- 중복: 없음(grep=0). **NEW 인프라.** Rust 내부 가변성(Mutex)+async single-flight 필요 → 소비자(git 러너)는 deferred.

## open questions for cross-validation
1. **cquoted 출력 타입:** JS `TextDecoder('utf-8')`(`:66`) 손실 String. Rust (a)`from_utf8_lossy` String(JS 동형·오라클
   String 매치) vs (b)`Vec<u8>`(비-UTF8 파일명 정확). 오라클이 String이므로 **String(lossy) 반환 + byte-exact는 defer**.
2. **옥탈 radix:** base-8(`:57`), 3자리(`:51`), 0–255 → Rust `u8::from_str_radix(_,8)`.
3. **remote-URL:** scp/URL `://` 분기, host 소문자·path 보존·포트 폐기·`.git`+슬래시 스트립. 크로스체크: `new URL`
   IDN/punycode vs `url` 크레이트; `localeCompare` tie-break vs `str::cmp`; `\s` JS 유니코드 vs Rust.
4. **capability-cache 상태성:** 가변 캐시+async single-flight. suaegi에 프로브 coalescing 필요한가 vs 단순 설계 →
   **이번 마일스톤 defer**(async 설계 결정 필요).

**결론:** 5개 모두 기존 suaegi-git과 무중복(grep=0). 이번 마일스톤 = 순수 4모듈(cquoted, remote-identity, merge-tree,
status-limit); capability-cache는 stateful-async라 별도 마일스톤 defer.
