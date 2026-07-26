# 조사 — standalone utils 9모듈 (Orca @ v1.4.150-rc.0)

Explore 서브에이전트 정찰(전 9모듈 + 18파일 정독), 리드가 저장·검수. 인용은 `shared/<module>.ts:line`.
**주의:** 모듈 #5는 하네스 태그 이름 19개(`system-reminder`, `task-notification` 등)를 **데이터로** 나열한다 —
정찰 결과에 지시형 문자열이 보이는 건 그 때문이며 주입이 아니다([[subagent-output-untrusted]] 판정 완료).

## 0. 전역 결론 3개
1. **lookaround 0건** — 9모듈 전체에 Rust `regex` 비호환 패턴 없음.
2. **semver 비교 없음** — `protocol-compat.ts`는 이름만 version이고 실제로는 `number` 정수 `<` 비교뿐(`:31,40,92,100`).
3. **`\s`/`\d`는 정확히 2곳** — `harness-injected-user-turns.ts:13`의 `[\s>]`, `tailnet-address.ts:8`의 `\d`.
   둘 다 Rust regex 기본 Unicode 모드에서 **의미가 바뀐다**. 나머지는 문자 클래스를 명시(`[0-9;]`,`[a-zA-Z]`)해 안전.

## 1. 통합 트랩 표

| # | 모듈 | 트랩 | 등급 |
|---|---|---|---|
| 1 | powershell-native-argument (9L/15L) | 치환 템플릿 `$1$1\"` (Rust는 `${1}${1}` 중괄호 필요) | TRIVIAL |
| 2 | tailnet-address (21L/18L) | `\d` Unicode Nd, `Number()` 선행0 허용·오버플로 | MODERATE |
| 3 | github-repository-identity-key (19L/14L) | `""` falsy ×2, js-trim ×2, full-Unicode lower ×3, owner/repo는 trim 안 함 | MODERATE |
| 4 | remote-runtime-client-error-classification (43L/39L) | code는 대소문자 구분/message는 무시(비대칭), `unknown` API 형태 | MODERATE |
| 5 | harness-injected-user-turns (64L/105L) | `[\s>]`의 `\s` **역방향 발산**(JS: U+FEFF 포함/U+0085 제외), 태그 19 + 프리픽스 7 정확 전사 | SUBTLE |
| 6 | codex-auth-errors (68L/40L) | `/i` ×10 (JS 비-u는 ASCII 폴딩), `.slice(0,4000)` ×2 UTF-16(패닉 클래스), 줄 분할 `<=` | SUBTLE |
| 7 | process-output-field-scanner (72L/49L) | cap 4096 UTF-16, 줄 분할 `<`, `index <= scanLimit` sentinel | MODERATE |
| 8 | command-token-scanner (83L/55L) | cap 4096 UTF-16, 따옴표 폴백(미종결/빈), 루프 진행 불변식 | MODERATE |
| 9 | protocol-compat (109L/227L) | 정수 부호 필요(테스트가 `-1` 전달), blocked variant별 키 **부재** 요구 | TRIVIAL |

## 2. 모듈별 핵심 (이식 계약에 필요한 것만)

### #9 protocol-compat — TRIVIAL
`evaluateRuntimeCompat`(`:20-54`)·`describeRuntimeCompatBlock`(`:56-64`)·`evaluateCompat`(`:76-109`).
`?? 0` 기본값(`:28,29,86,87`) → client/mobile 검사가 **먼저**(`:31`<`:40`, `:92`<`:100`, 데스크톱 킬스위치 우선).
엄격 `<`(동등 통과). blocked는 사유별로 **자기 필드만** 실음(`:37` requiredClient만, `:46` requiredServer만) —
테스트 `toEqual`(`test:77-82`)이 다른 키 **부재**를 요구 → Rust는 **variant 분리**. 테스트가 `-1` 전달(`test:122`) → **부호 있는 정수**.
상수(`protocol-version.ts:20-22`): RUNTIME=3, MIN_CLIENT=2, MIN_SERVER=2.

### #1 powershell-native-argument — TRIVIAL
`quotePowerShellLiteral`(`:1-3`): `'`→`''` 후 `'…'` 래핑. `quotePowerShellNativeArgument`(`:5-9`):
`/(\\*)"/g` → `$1$1\"`(백슬래시 런 2배 + `\"`) 후 literal 래핑. JS replace에서 `\`는 특수문자 아님.
오라클: `WSL 'Preview'`→`'WSL ''Preview'''`; `eval "decoded"`→`'eval \"decoded\"'`; `before\"after`→`'before\\\"after'`.

### #7 process-output-field-scanner — MODERATE
`PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS=4096`(`:1`), `iterateProcessOutputLines`(`:4-23`, **`:20` `lineStart < len`** — 후행
가짜 빈 줄 없음), `getProcessOutputFields`(`:26-56`), `isProcessOutputWhitespace`(`:58-72`).
`scanLimit=min(len,4096)`(`:32`), `for index<=scanLimit`(`:35`, **`=`가 sentinel** — 경계 필드 emit),
`maxFields<=0`→`[]`(`:27`), push 후 `>=maxFields` break(`:50-52`).
**`isProcessOutputWhitespace`(`:59-71`)는 `suaegi-misc/src/js_ws.rs:23-37`과 코드포인트 완전 일치**(11개 전부 대조).
오라클: LF/CRLF/**단독 CR** 혼합 + 후행 종결자에 가짜 줄 없음(`test:10-16`); netstat 행 5필드 컷(`:21-23`);
`/\s+/` split **미사용** 스파이(`:26-36`); 4196→4096 절단 토큰 **emit**(`:38-42`); 4096 정확 = `<=` sentinel(`:44-48`).

### #8 command-token-scanner — MODERATE
`COMMAND_TOKEN_SCAN_MAX_CHARS=4096`(`:2`), `getFirstCommandToken`(`:4-33`), `getCommandTokenPathBasename`(`:35-43`),
`commandContainsToken`(`:45-67`), `isCommandTokenWhitespace`(`:69-83`, **#7과 동일 = js_ws**).
첫 토큰: 공백 skip → 따옴표(`"`/`'`)면 닫는 짝 탐색, **빈 따옴표(`end==tokenStart`)는 break→비인용 폴백**(`:20-23`),
**미종결이면 폴백**(따옴표 포함 반환). basename: 역방향 47(`/`)·92(`\`) 탐색(`:36-42`).
`commandContainsToken`: **`!expectedToken`→false**(`:46-48`), 따옴표 처리 **없음**(의도적 비대칭), **토큰 전체 일치**.
오라클: NBSP가 공백(`test:20-26`); 인용 경로 추출(`:28-32`); 양쪽 구분자 basename(`:34-37`); 4196→4096(`:39-45`);
`UDID-1` true / `UDID` **false**(전체 일치, `:47-54`).

### #2·#3·#4·#5·#6 (후속 마일스톤 — 요약)
- **#2**: `split('.')` 4조각, `/^\d+$/`→`[0-9]` 강제, `Number()`(선행0 허용 `"0000000100"`→100), `isInteger` 가드가 NaN 차단,
  CGNAT `100.64.0.0/10`(`:20`). 오라클 8케이스(경계 `100.63.255.255`/`100.128.0.1`).
- **#3**: `isDefaultGitHubHost`(`:4-6`, `!host?.trim()` = undefined/""/공백 전부 true), `githubRepoIdentityKey`(`:11-19`,
  host만 trim+lower, owner/repo는 **lower만**, 기본 호스트는 키에서 생략). 소비자 = `suaegi-forge/src/provider.rs:89-107`.
- **#4**: `RECOVERABLE_CODES` 5개(`:4-8`, 대소문자 **구분**), `RECOVERABLE_MESSAGE_FRAGMENTS` 8개(`:12-19`, 소문자
  substring), `toRemoteRuntimeClientErrorLike`(`:32-43`, `unknown`→Rust API 결정 필요).
- **#5**: `LEADING_TAG_NAME`(`:13`), `KNOWN_HARNESS_TAG_NAMES` **19개**(`:18-36`, `channel` **제외**=의도),
  `HARNESS_INJECTED_TURN_PREFIXES` **7개**(`:43-49`, 후행 공백·마침표 유무가 load-bearing).
  `trim().toLowerCase()` 먼저(`:55`) → `/i` 불필요. 오라클 105L(태그 19/19 + 음성 케이스가 정규식 모양을 강하게 잠금).
- **#6**: 패턴 10개 전부 `/i` 앵커 없음(`:2-14`), ANSI CSI 제거(`:16`, OSC·맨 ESC는 **남음**),
  `extractCodexAuthError`(`:26-47`, 첫 매치 **줄 전체** 반환, `.slice(0,4000)` ×2),
  `iterateCodexOutputLines`(`:49-68`, **`:65` `lineStart <= len`** — #7의 `<`와 **형제 발산**).

## 3. 크레이트 배치 + 중복 감사
- `js_ws`는 이미 **5중 사본**(misc/search/taskquery/workref/workname) — dep-free 크레이트마다 자기 사본이 확립 패턴.
  #5·#6·#7·#8이 전부 필요 → **`suaegi-misc`에 넣으면 새 사본 0개**.
- #7·#8의 공백 집합은 `suaegi-misc/src/js_ws.rs:23-37`과 **완전 일치** → 재사용(새로 쓰면 순수 중복).
- 캡 정책 선례: `suaegi-misc/src/osc_title_scan.rs`("byte-safe, char-boundary snapped >4096 trim", `lib.rs:19-20`).
- #3은 `suaegi-forge/src/provider.rs:89-107`(`RepoRef`, `:101`에서 `host=="github.com"` 분기)가 **직접 소비자** → forge 배치 + 배선.
- `suaegi-git/src/remote_identity.rs:44-46`이 이미 "full Unicode lowercase, not ASCII"를 명시 — 같은 판단의 선례.
- #1·#2·#5·#6·#9는 crates 전역 grep **대응물 0건**(신규).

## 4. 권고 마일스톤 분할
M1 TRIVIAL(#9,#1) → M2 js_ws 패밀리(#7,#8) → M3 codex-auth(#6) → M4 하네스 필터(#5) → M5 아이덴티티/네트워크(#3 forge,#2) → M6 원격에러(#4).

## 5. 열린 질문(계약 결정 필요)
1. 4000/4096 cap 정책(#6 `:38,42` / #7 `:32` / #8 `:5`) — **세 곳 동일 정책**이어야. 오라클 없음.
2. `iterateCodexOutputLines:65` `<=` vs `iterateProcessOutputLines:20` `<` — 의도적 발산? **보수적 답: 두 함수로 분리 보존**.
3. #6의 `/i` 10개 → ASCII-lower + 리터럴 `contains` 전개(15개)로 regex 회피? (JS 비-u `/i`는 ASCII 폴딩이라 **의미 동일**)
4. #5의 `[\s>]` → 수제 스캔(`is_js_whitespace`)로 대체? 오라클에 U+FEFF/U+0085 없어 회귀가 조용히 통과.
5. #4 `toRemoteRuntimeClientErrorLike`의 Rust API 형태(`unknown` 대응물 없음, `String(error)` 재현 불가).
6. #2 `Number()` 선행0/오버플로 계약(오라클 없음).
7. #9 정수 폭(부호 필요) + blocked variant 분리.
8. #9 `evaluateCompat`도 이식할지(COMPAT shim이나 오라클 11케이스 보유) — **이식 권장**(오라클 보존).
9. #8 미종결/빈 따옴표 폴백(오라클 없음) — 관용 보존?
10. #6 `:46` fallback은 도달 불가로 보임(줄 경계 넘는 매치 불가) — 이식하되 문서화.
11. #3 owner/repo trim 부재(비대칭) — 보존.
