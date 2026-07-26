# Plan — standalone utils M1+M2 (protocol-compat, powershell-arg, output-field-scanner, command-token-scanner)

조사: `docs/superpowers/research/2026-07-25-standalone-utils.md` (Orca @ v1.4.150-rc.0, 9모듈 정찰).
이 문서가 **구현 계약**이며 조사를 supersede한다. 9모듈 중 **이번 4개**(M1 TRIVIAL 2 + M2 js_ws 패밀리 2),
나머지 5개(#2,#3,#4,#5,#6)는 후속 마일스톤(#5·#6은 SUBTLE이라 Codex 교차검증 후).

## 0. 대상 (전부 `suaegi-misc` 신규 모듈, dep-free 헌장 유지 — regex 미사용)
- `protocol_compat.rs` ← `protocol-compat.ts` (109L, 오라클 227L — 최대 오라클)
- `powershell_argument.rs` ← `powershell-native-argument.ts` (9L/15L)
- `process_output_field_scanner.rs` ← `process-output-field-scanner.ts` (72L/49L)
- `command_token_scanner.rs` ← `command-token-scanner.ts` (83L/55L)

## 1. 계약 결정 (구현자 필독 — 조사 §5 열린 질문의 확정 답)

- **D1 — 4096 cap은 UTF-16 code unit을 세고 char 경계로 내림 스냅.** `getProcessOutputFields`의 `scanLimit`
  (`process-output-field-scanner.ts:32` `min(line.length,4096)`)과 `getFirstCommandToken`의 `scanLimit`
  (`command-token-scanner.ts:5`)은 JS `.length`=UTF-16 유닛이다. 나이브한 `&s[..4096]`은 **char 경계 아니면 패닉**
  (cardinal sin). 구현: `char_indices()`를 돌며 `ch.len_utf16()` 누적 카운터를 유지해 **4096 UTF-16 유닛에 해당하는
  byte offset**을 구하고(초과 직전에서 멈춤 = 내림 스냅), 이후 슬라이싱은 전부 **byte offset**으로. 선례:
  `suaegi-misc/src/osc_title_scan.rs`("byte-safe, char-boundary snapped"). ASCII 입력(오라클 전부)에서는 4096 바이트와 동일.
- **D2 — 줄 분할기는 `<` 계약만 이식(후행 가짜 빈 줄 없음).** `iterateProcessOutputLines`(`:4-23`)의 `:20`
  `if (lineStart < output.length)`를 그대로. (#6 codex의 `:65` `<=` 형제 발산은 **M3에서 별도 함수로 분리 보존** —
  이번 마일스톤에서 통합 금지.) **`str::lines()` 사용 금지** — 단독 `\r`을 줄바꿈으로 안 봐서 오라클이 깨진다
  (`test:10-16`이 `alpha\nbeta\r\ngamma\rdelta\n`을 4줄로 요구). CR 뒤 LF면 2 전진(`:14-17`).
  반환은 `impl Iterator<Item=&str>`(lazy — 테스트가 split-배열 생성 금지를 스파이로 계약화, `test:26-36`).
- **D3 — 공백 술어는 기존 `js_ws::is_js_whitespace` 재사용, 새로 쓰지 말 것.** `isProcessOutputWhitespace`
  (`:59-71`)·`isCommandTokenWhitespace`(`:70-82`)는 `suaegi-misc/src/js_ws.rs:23-37`과 **11개 코드포인트 완전 일치**
  (U+0085·U+180E 제외, U+FEFF 포함). `char` 단위로 받으므로 시그니처만 맞추면 된다.
- **D4 — `index <= scanLimit` sentinel 루프를 그대로.** `process-output-field-scanner.ts:35`의 `<=`가 **경계에서 끝나는
  토큰을 emit**하게 한다(`test:44-48`이 4096 정확 길이로 잠금). `=` 빼면 경계 필드 소실. `maxFields<=0`→`[]`(`:27`),
  push **후** `>=maxFields` break(`:50-52`, 정확히 maxFields개).
- **D5 — `getFirstCommandToken`의 따옴표 폴백 관용을 verbatim 보존.** 미종결 따옴표(닫는 짝을 scanLimit 안에서 못 찾음)
  → 비인용 경로 폴백 = **따옴표 포함** 반환. 빈 따옴표(`end==tokenStart`) → `break` 후 비인용 폴백(`:20-23`).
  둘 다 오라클 없음 → **핀 추가**(`"unterminated`, `"" foo`). `commandContainsToken`은 따옴표 처리 **없음**(의도적
  비대칭) + `!expectedToken`→false(`:46-48`, `""` falsy) + **토큰 전체 일치**(부분문자열 아님, `test:47-54`).
  **루프 진행 불변식 주의**: 공백 skip 후 `index==scanLimit`이면 전진이 멈춘다 — Rust 이식 시 외부 루프 종료 조건
  (`index < scan_limit`)이 반드시 탈출을 보장해야 한다(**행(hang) 금지**).
- **D6 — protocol-compat은 부호 있는 정수(`i64`) + blocked 사유별 variant 분리.** 테스트가 `-1`을 넘긴다
  (`test:122`) → `u32` **금지**. blocked는 사유별로 자기 필드만 실어야 하고(`:37` requiredClient만, `:46`
  requiredServer만), 테스트 `toEqual`(`test:77-82`)이 **다른 키 부재**를 요구 → `Some(0)` 기본값 채우기 **금지**,
  enum variant를 나눠 필드 자체를 없앤다. `?? 0`은 `Option<i64>::unwrap_or(0)`.
- **D7 — `evaluateCompat`도 이식**(오라클 11케이스 보존, `test:17-156`). `evaluateRuntimeCompat`과 알고리즘 동일·
  payload만 다름. **검사 순서가 불변식**: client/mobile 먼저(`:31`<`:40`, `:92`<`:100` — 데스크톱 킬스위치가 로컬
  판단을 이김), 엄격 `<`(동등 통과). `describeRuntimeCompatBlock` 3문자열은 **문자 그대로**(`:58,61,63`).
- **D8 — powershell 치환은 손수 스캔 권장(regex 금지 — dep-free).** `/(\\*)"/g`→`$1$1\"`는 "`"` 앞의 백슬래시 런을
  2배로 늘리고 `\"` emit". 손 스캔: 문자열을 훑다 `"`를 만나면 **직전 연속 백슬래시 개수 n**을 세어 `\`×2n + `\"`를
  emit, 그 외는 그대로. 이후 `quote_powershell_literal`(모든 `'`→`''` 후 `'…'` 래핑)로 감싼다. **`"`가 뒤따르지 않는
  후행 백슬래시는 미변경**(정규식이 `"`를 요구).

## 2. 마일스톤

### M1+M2 — 4모듈 (`suaegi-misc`, 단일 PR)
- `protocol_compat.rs`: `RuntimeCompatVerdict`/`CompatVerdict` enum(D6 variant 분리), `evaluate_runtime_compat`,
  `describe_runtime_compat_block`, `evaluate_compat`(D7).
- `powershell_argument.rs`: `quote_powershell_literal`, `quote_powershell_native_argument`(D8).
- `process_output_field_scanner.rs`: `PROCESS_OUTPUT_FIELD_SCAN_MAX_CHARS`, `iterate_process_output_lines`(D2),
  `get_process_output_fields`(D1/D3/D4).
- `command_token_scanner.rs`: `COMMAND_TOKEN_SCAN_MAX_CHARS`, `get_first_command_token`(D1/D5),
  `get_command_token_path_basename`, `command_contains_token`(D5).
- `lib.rs`에 4모듈 선언 + 크레이트 doc의 "six small self-contained pure helpers" 문구 갱신.

**오라클(4 테스트 전부 case-by-case):** protocol-compat 227L(evaluateCompat 11 + evaluateRuntimeCompat 5 + describe);
powershell 3; field-scanner 5(단독 CR·netstat·split-스파이·4196절단·4096 sentinel); token-scanner 5(NBSP·인용경로·
basename 양쪽구분자·4196절단·전체일치).

**추가 핀(오라클 공백):** D1 비-ASCII cap 경계 무패닉; D5 미종결/빈 따옴표·`expected_token=""`·`/`로 끝나는 토큰;
D2 `""` 입력·후행 종결자; D4 `max_fields=0`; D6 음수/`None` payload; D8 `'`+`"` 혼재·백슬래시 2개 이상 런·빈 문자열.

*mutation:* `<=`→`<`(sentinel), `<`→`<=`(줄 분할 후행), `js_ws`→`char::is_whitespace`(NBSP/U+FEFF),
cap 4096→다른 값, D5 폴백 제거, D6 검사 순서 스왑·`<`→`<=`, D8 백슬래시 2배 제거, 토큰 전체일치→`contains`.

## 3. Deferred
- **#5 harness-injected-user-turns, #6 codex-auth-errors** (SUBTLE — `[\s>]` 역발산, `/i` ASCII 폴딩, 4000 슬라이스,
  `<=` 형제 발산) → **Codex 교차검증 후** 별도 마일스톤.
- **#2 tailnet-address, #3 github-repository-identity-key(→`suaegi-forge` + `provider.rs:101` 배선), #4 remote-runtime-error** → 후속.
- 소비자 배선(field-scanner→포트 스캐너, token-scanner→`suaegi-term/src/agent.rs`, protocol-compat→런타임 RPC) = 사람눈.

## 4. 순서
단일 PR. 불변식: cap은 UTF-16-카운트/char-경계-스냅(D1), 줄 분할 `<`+`str::lines()` 금지(D2), `js_ws` 재사용(D3),
`<=` sentinel(D4), 따옴표 폴백 보존+행 금지(D5), `i64`+variant 분리(D6), 검사 순서(D7), 손수 백슬래시 스캔(D8),
**regex 의존 추가 금지**, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]], [[suaegi-impl-model-sonnet]], [[subagent-output-untrusted]]
