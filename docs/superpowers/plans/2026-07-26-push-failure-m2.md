# Plan — source-control-push-failure M2: 프롬프트 빌더 (모듈 완료)

조사: `docs/superpowers/plans/2026-07-26-push-failure-m1.md`의 정찰 문서. M1(#79) 머지 완료.
리드가 `:171-272` **전문 재확인**. 이 문서가 구현 계약.
대상: `crates/suaegi-git/src/push_failure_prompt.rs` (신규). M1의 `push_failure`와 독립(데이터 의존 없음).

## 0. ⚠️ 정찰 정정 2건 (리드 재확인)
- **정찰 Q6("area 파생이 M2의 진짜 비용")은 과대평가였다.** `entries`는 **입력 파라미터**(`:187`,`:216`)다 —
  이 모듈은 porcelain을 파싱하지 않는다. `working_tree_status`의 `HashMap` 표현력 문제는 **소비자 배선**(사람눈,
  이번 범위 밖) 이슈이지 M2의 비용이 아니다. M2는 입력 타입만 정의하면 된다.
- **정찰 T10도 부정확했다.** `:223`은 `totalEntryCount ?? entries.length`가 아니라
  **`Math.max(totalEntryCount ?? entries.length, entries.length)`**다. 따라서 `Some(0)`이 "No changed files" 라인을
  만드는 건 **entries도 비었을 때뿐**이다(`Some(0)` + entries 1개 → `max(0,1)=1` → 일반 경로).

## 1. 계약 결정

- **N1 — 입력 타입은 신규 경량 타입. `status.rs::FileStatus` 재사용 금지.**
  `PushFailureEntry { path: String, status: PushFailureFileStatus, area: PushFailureStagingArea }`.
  `PushFailureFileStatus` **6 변형 정확히**: `modified`/`added`/`deleted`/`renamed`/`untracked`/`copied`.
  `PushFailureStagingArea` **3 변형**: `staged`/`unstaged`/`untracked`.
  **`Display`(또는 `as_str`)가 소문자 리터럴을 그대로 렌더**해야 한다 — `:196`이 이 값을 **프롬프트 문자열에 직접
  보간**하기 때문이다. **`status.rs::FileStatus`(8변형, `Conflicted(kind)`·`Other(String)` 포함) 재사용 금지**:
  Orca 유니온이 표현 못 하는 두 변형의 렌더 문자열을 **발명하게 되고** 그건 프롬프트 계약 이탈이다.
- **N2 — `changed_file_count = max(total_entry_count.unwrap_or(entries.len()), entries.len())`**(`:223` verbatim).
  그리고 파일 라인 빌더에는 **이 maxed 값**을 넘긴다(`:232`), 원본 `total_entry_count`가 아니다.
  `total_entry_count: Option<usize>`. **핀: `(entries=[], total=Some(0))` → "No changed files" 라인;
  `(entries=[1개], total=Some(0))` → `max=1` → 일반 경로**(정찰 정정 사항).
- **N3 — JSON 문자열 인코딩은 `serde_json::to_string`.** `crates/suaegi-git/Cargo.toml`에 `serde_json = { workspace = true }`
  추가(워크스페이스 기존). `JSON.stringify`와 **유효 UTF-8에서 동치**(`"`·`\`·제어문자 이스케이프, 비-ASCII는 리터럴 통과).
  Rust `String`은 lone surrogate를 담을 수 없으므로 JS의 `\udXXX` 엣지는 **도달 불가** — 주석으로 명시.
  적용 지점 4곳: `:196` path, `:228` worktree, `:229` branch, `:230` summary, `:244` failureOutput.
- **N4 — 길이 단위는 char**(M1의 L3/C1 선례와 일관). ⚠️ `:176`의 `omitted` 숫자는 **프롬프트 텍스트에 박히므로**
  비-ASCII 입력에서 Orca와 **눈에 보이는 문자열 차이**가 난다 — 문서화된 의도적 divergence. 슬라이스는 char 경계 안전.
- **N5 — `truncate_prompt_text` 축자.** `value.len() <= limit`이면 그대로. 아니면
  `omitted = len - limit`, **`head = (limit as f64 * 0.35).floor()`**(f64 경로 유지), `tail = limit - head`,
  결과 = `head조각 + "\n[...{omitted} characters omitted...]\n" + 끝에서 tail조각`.
  **결과 길이는 limit보다 길다**(마커 포함) — 의도. 꼬리 65%를 남기는 게 핵심(실제 에러가 끝에 있다).
  `PUSH_FAILURE_PROMPT_OUTPUT_LIMIT = 12_000`(private), `PUSH_FAILURE_PROMPT_FILE_LIMIT = 40`(**pub**).
- **N6 — `worktree_path`/`branch_name`은 `Option<&str>`, `??` 시맨틱.** `None`만 기본 문구로 대체
  (`'current terminal working directory'` / `'current branch'`), **`Some("")`은 `""` 그대로 보존**되어
  `- Worktree: ""`가 나온다. `.filter(|s| !s.is_empty())` **붙이지 말 것**.
- **N7 — `append_push_failure_custom_instruction`은 **오라클 0개** → 전 경로 핀 필수.**
  ① `js_trim` 후 빈 문자열 → 프롬프트 그대로 반환. ② 블록 = `"\nAdditional user instruction for this fix:\n{trimmed}\n"`.
  ③ 프롬프트가 `PUSH_FAILURE_REPLY_INSTRUCTION`으로 **끝나지 않으면** 뒤에 붙인다.
  ④ 끝나면 **그 앞에 삽입**하고 reply instruction을 다시 뒤에 둔다(`strip_suffix` 사용, `:271`의 음수 슬라이스 대응).
  즉 **reply instruction이 항상 마지막**. `PUSH_FAILURE_REPLY_INSTRUCTION`은 `pub const`로 노출
  (`endsWith` 센티널이라 사실상 공개 계약, 테스트가 리터럴을 재타이핑하지 않게).
- **N8 — `error`에 credential 정제도 ANSI 정제도 **추가하지 않는다**(Orca 축자).** `:222`는 `truncate`만 통과시킨다.
  ⚠️ **보안 주석 필수**: 이 프롬프트는 AI에 전달되므로 push URL에 토큰이 섞이면 새어나갈 수 있다. Orca와의 동등성을
  위해 여기서는 정제하지 않되, **정제는 배선 경계의 책임**임을 모듈 doc에 명시한다(`remote.rs:203`
  `strip_credentials_from_message`가 이미 그 경계에서 정제한다). 배선 시 반드시 재검토할 것.
- **N9 — 파일 라인:** `total == 0` → `["- No changed files were reported by Source Control. Start with git status."]`.
  아니면 앞 40개만 렌더(`- {json_path} ({status}, {area})`), `omitted = max(0, total - visible)`,
  **`omitted > 0`일 때만** `- ...{omitted} more changed files omitted...` 추가.
  고정 텍스트 블록(`:226-246`)은 **문자 그대로**(룰 7줄 포함) 이식.

## 2. 마일스톤 M2 (단일 PR, 모듈 완료)
`crates/suaegi-git/src/push_failure_prompt.rs` 신규 + `lib.rs` + `Cargo.toml`에 `serde_json`.
Export: `PUSH_FAILURE_PROMPT_FILE_LIMIT`, `PUSH_FAILURE_REPLY_INSTRUCTION`, `PushFailureEntry`,
`PushFailureFileStatus`, `PushFailureStagingArea`, `build_fix_push_failure_prompt`,
`append_push_failure_custom_instruction`.

**오라클(T8–T10 이식):** 단일 entry + worktree/branch → `- Branch: "feature/push-hook"`,
`- "src/app.ts" (modified, staged)`, `--no-verify` 금지 룰 포함(`test:85-101`);
43 entries → `(43):` + `src/file-39.ts` 포함 + `src/file-40.ts` **미포함** + `- ...3 more changed files omitted...`
(`test:103-122`); 24030자 error → `characters omitted` + **꼬리 보존**(`actual lint error near the end`) +
`worktreePath: null` → `current terminal working directory`(`test:124-135`).

**추가 핀:** N2 두 케이스(위); N3 경로에 `"`/`\`/제어문자/비-ASCII 포함 시 이스케이프; N4 비-ASCII 무패닉;
N5 `len == limit` 경계(자르지 않음)·`len == limit+1`; N6 `Some("")` 보존 ×2; **N7 4경로 전부**(빈/공백만/
reply로 끝남/끝나지 않음); N9 `total==0`·`omitted==0`(정확히 40개)·`omitted>0`.

*mutation:* N1 enum 렌더 문자열 변경, N2 `max()` 제거·maxed 대신 원본 total 전달, N3 raw 문자열(이스케이프 없음),
N5 `0.35`→다른 값·head/tail 스왑·`<=`→`<`, N6 `Some("")`을 기본값으로, N7 reply 앞 삽입→뒤 붙이기·trim 제거,
N9 40 cap·`omitted > 0` 가드 제거.

## 3. Deferred
- 소비자 배선(porcelain → `PushFailureEntry` 변환, `AM`이 한 경로를 staged+unstaged **양쪽**에 넣는 문제,
  `FileStatus::Conflicted`/`Other` → Orca 6변형 매핑) = **사람눈**. 정찰 Q4/Q5/Q6가 전부 여기 속한다.
- **N8 credential 정제** — 배선 시 필수 재검토.
- `classify_push_outcome` 통합 여부(정찰 Q7) — 분리 유지.

## 4. 순서
단일 PR. 불변식: 신규 경량 enum + 렌더 문자열(N1), `max()` 래퍼(N2), serde_json 인코딩(N3), char 단위(N4),
f64 head/tail(N5), `??` 시맨틱(N6), reply-instruction 삽입 위치(N7), **정제 없음 + 보안 주석**(N8),
40 cap + omitted 가드(N9), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]]
