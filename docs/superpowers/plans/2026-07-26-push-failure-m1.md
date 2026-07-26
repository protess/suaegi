# Plan — source-control-push-failure M1: 분류·정규화

조사: Explore 정찰(272L 소스 + 136L 오라클 전문 정독), 리드 검수. 인용은 `shared/source-control-push-failure.ts:line`.
대상: `crates/suaegi-git/src/push_failure.rs` (신규). **새 외부 의존 없음**(`regex` 기존, `suaegi-misc` 추가).

## 0. 범위 (2-PR 분할 중 M1)
포팅: `:3-6`(상수), `:13-26`(regex 8개), `:28-66`(정규화·라인), `:68-135`(분류·요약·확장판정), `:137-169`(WS fold).
Export: `PUSH_FAILURE_SUMMARY_SCAN_CODE_UNITS`, `is_push_hook_failure`, `sanitize_push_failure_details`,
`summarize_push_failure`, `has_expanded_push_failure_details`.
**M2(별도 PR)로 연기:** 프롬프트 빌더(`:8-11, 171-272`) — `GitStatusEntry`/`GitStagingArea` 신규 타입 설계,
JSON 이스케이프, area 파생(porcelain `AM`은 한 경로가 staged+unstaged **동시** 존재 → 현재 `working_tree_status`의
`HashMap<String,_>`로는 표현 불가)이 얽혀 **결정 비용이 M1과 다른 축**이다. 두 반쪽은 데이터 의존이 없다
(`build...Prompt`가 `summary`/`error`를 이미 `String`으로 받음, `:214-215`) — 깨끗한 절단면.

## 1. 계약 결정

- **L1 (T2, 최우선) — 6개 regex의 `/i`는 `(?i-u:…)`로. `to_lowercase()` 절대 금지.**
  이 파일에 `.toLowerCase()`는 **0회**이고, 케이스 무시는 전부 **정규식 `/i`(non-`u`)** 다 = ECMAScript
  Canonicalize = **비-ASCII를 ASCII로 폴딩하지 않는다**(U+212A KELVIN ↛ `k`, U+017F ſ ↛ `s`).
  Rust `(?i)`는 Unicode simple folding이라 **폴딩해버린다** → 반드시 **`(?i-u:…)`**(ASCII-only).
  **⚠️ 인접 코드가 오답을 유도한다:** `suaegi-forge/src/classify.rs:32`는 `stderr.to_lowercase()` + `contains()`를
  쓴다 — 그건 **다른 메커니즘**(full Unicode)이고 이 모듈에 복제하면 조용히 틀린다([[js-lowercase-two-mechanisms]]).
  **핀: U+212A를 `k` 자리에 넣은 입력이 매치되지 않을 것**(예: `pre-pus` + U+212A는 `pre-push` 불일치).
- **L2 (T1/T3/T4) — 문자 클래스는 패턴별로 다르게 처리한다. `(?-u)` 일괄 적용 금지.**
  - `\d`(ANSI_PATTERN `:15`, 3곳) → **`[0-9]`**. Rust `\d`는 Unicode `Nd`(아랍-인도 숫자 매치).
  - `\b`(`:20,21,22,23,24`) → **`(?-u:\b)`**. JS `\b`는 ASCII word `[A-Za-z0-9_]` 기준.
  - `\s`(`:20`, `npm\s+(?:warn|warning)`) → **명시적 ECMAScript WS 클래스**.
    `(?-u:\s)`(ASCII-only)도 `\s`(Unicode White_Space)도 **둘 다 틀리다**: JS `\s`는 **U+FEFF 포함 / U+0085 제외**,
    Rust `\s`는 **U+0085 포함 / U+FEFF 제외**. →
    `[\t\n\x0B\x0C\r \u{a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]`
  - `.`(`:20`, `.*`) → JS `.`는 LF/CR/**U+2028/U+2029** 제외, Rust `.`는 `\n`만 제외 →
    **`[^\n\r\u{2028}\u{2029}]`** 로 명시(라인 안에 U+2028이 살아남을 수 있다 — `:52-66`이 **LF만**으로 라인을 쪼개므로).
- **L3 (T6/T17) — 길이 단위는 **char**(C1 선례), 슬라이스는 char 경계 안전.**
  `commit_message_prompt.rs:112-114`가 이미 "Orca는 UTF-16 code unit, 여기선 chars, per C1"로 결정해 둔 선례를
  **그대로 따른다**(일관성). 적용 지점: `:30` 64KiB 스캔, `:127` `raw.length > LIMIT` 비교.
  **바이트 슬라이스 금지**(non-char-boundary = 패닉, cardinal sin). `char_indices()`로 경계 확보.
  오라클(`test:74-81`)은 순-ASCII라 어느 쪽이든 통과 — **문서화된 좁은 divergence**로 주석.
  **`:123` 빈-체크가 `:127` 길이-체크보다 먼저**라는 순서를 보존(T17: 65537 units 전부 ANSI인 입력은 `false`).
- **L4 (T8) — WS 술어/trim은 `suaegi_misc::{is_js_whitespace, js_trim}` 재사용.**
  `:155-169`의 수동 코드 테이블은 `suaegi-misc/src/js_ws.rs:23-37`과 **코드포인트 집합 완전 일치**(정찰 검증).
  `suaegi-git`에 `suaegi-misc = { path = "../suaegi-misc" }` 추가(**의존 0개 순수 leaf, 사이클 없음** — M3에서
  `suaegi-forge`에 같은 판단을 적용한 선례). 7번째 사본을 만들지 말 것.
- **L5 (T20) — ANSI/CONTROL 패턴은 verbatim, 특히 **좁은 최종 바이트 클래스**를 넓히지 말 것.**
  `:13-15` ANSI: 도입부 `[\u{1b}\u{9b}]`(**C1 CSI U+009B도 처리**), 프리픽스 `[[\]()#;?]*`, OSC/BEL 대안,
  CSI 대안의 최종 클래스 **`[0-9A-PR-TZcf-nq-uy=><~]`** — 표준 `[@-~]`보다 **의도적으로 좁다**(`Q`,`U`-`Y`,`a`,`b`,
  `d`,`e`,`o`,`p`,`v`-`x`,`z`,`@`,`[`-`^`,`` ` `` 제외). 넓히면 동작이 바뀐다.
  `:16-18` CONTROL: `[\u{0}-\u{8}\u{b}\u{c}\u{e}-\u{1f}\u{7f}-\u{9f}]` — **TAB/LF 보존**, U+00A0 **보존**.
  **⚠️ `suaegi-gen-prompt::strip_ansi_control_sequences` 재사용 금지** — U+009B 미처리, 최종 클래스가 `[@-~]`로
  더 넓고, `ESC (`/`#`/`;`/`?` 프리픽스 미지원 = **다른 함수**다. 별도 구현.
- **L6 (T20 순서) — `normalize`의 5단계 순서가 load-bearing:**
  ① char 스캔 상한 → ② ANSI 제거 → ③ `\r\n?`→`\n` → ④ CONTROL 제거 → ⑤ `js_trim`.
  ②가 ③보다 먼저(ANSI 내부 CR 선제거), ③이 ④보다 먼저(CONTROL 범위가 `\u{d}`를 **제외**하므로 ③ 없으면 CR 잔존).
- **L7 — 라인 분할은 **LF만**.** `:52-66`이 `charCodeAt !== 10` 수동 스캔이고 테스트가 **`String.prototype.split`
  호출 0회**를 스파이로 강제한다(`test:75,80`). Rust는 `split('\n')`로 이식(LF는 UTF-8 멀티바이트 내부에 못 들어감).
  **`.lines()` 금지**(trailing `\r` 처리 계약이 다름). 각 조각 `js_trim` 후 빈 것 제거.
- **L8 (`:68-95`) — 7분기 순서가 전부. 분기2 exclusion이 **무조건 이긴다**.**
  ① 빈 → false ② **EXCLUSION 매치 → false(무조건)** ③ `hook declined to push`(인라인, **`\b` 없음**) → true
  ④ PUSH_HOOK → true ⑤ RUNNER **∧** CONTEXT → true ⑥ LINT **∧** CONTEXT → true ⑦ false.
  **blob 전체 대상 매칭**(라인 단위 아님) — 1행 `husky`와 9행 `git push`가 결합되는 게 의도.
  `PUSH_CONTEXT_PATTERN`의 공백은 **리터럴 단일 스페이스**(`\s+` 아님) → `"failed  to  push"`는 불일치.
- **L9 — EXCLUSION 21개 verbatim(`\b` 하나도 없음 = 순수 substring).** `submodule`(#12)이 `failed to push all
  needed submodules`(#13)·`unable to push submodule`(#14)를 **완전 포섭하는 dead alternative** — **verbatim 유지 +
  주석**(upstream diff 대조성). 목록에 **bare `failed to push`는 없다**(그래서 T1 positive가 성립).
- **L10 (`:101-117`) — 요약 4분기, **lint > hook**.** ① 라인 0개 → `Push failed.` ② LINT → `Lint failed during push.`
  ③ HOOK∨RUNNER → `Pre-push hook failed.` ④ `lines[0]`.
  **exclusion을 보지 않는다**(auth 실패를 넘기면 첫 줄이 그대로 요약) — 게이팅은 호출자 책임, 주석 명시.
- **L11 (`:37-50`) — 저신호 필터는 **신호 라인이 하나라도 있을 때만** 작동.** `hasSignalLine` false면 필터 skip(npm
  노이즈가 요약이 됨). 필터 결과가 **전멸하면 원본 라인으로 되돌린다**(`filtered.len() > 0 ? filtered : lines`).
  **오라클 0개** → 핀 필수.

## 2. 마일스톤 M1 (단일 PR)
`crates/suaegi-git/src/push_failure.rs` 신규 + `lib.rs` 선언/re-export + `Cargo.toml`에 `suaegi-misc`.

**오라클(`source-control-push-failure.test.ts` T1–T7 이식):** bare `failed to push`+pre-push→true/hook 요약;
lint∧context→true/lint 요약; exclusion 6종→false(**T4a: exclusion이 LINT∧CONTEXT를 이김**);
ANSI+BEL 이중 스트립+lint>hook; hasExpanded 2케이스; **64KiB 경계 + split 미사용**.

**추가 핀(오라클 침묵 — 정찰이 열거):** L1 U+212A/U+017F 불일치; L2 아랍-인도 숫자가 ANSI `\d` 불일치·`\b`
경계(`pre-pushed`✗/`lint-staged`✓)·`\s`의 U+FEFF✓/U+0085✗·`.`의 U+2028; L3 non-ASCII 경계 무패닉;
L6 `\r\n`/lone-CR 정규화; L7 U+2028은 라인 구분자 **아님**; L9 exclusion #11 `stale info`·#19·#20·#21;
L10 `lines[0]` 폴백; **L11 저신호 필터 3케이스**(신호 있음→필터·신호 없음→skip·전멸→되돌림);
whitespace-fold 동일내용/공백만-다름.

*mutation:* L1 `(?i-u)`→`(?i)`, L2 `[0-9]`→`\d`·`(?-u:\b)`→`\b`·WS 클래스→`\s`·`.`, L3 byte 슬라이스·순서 스왑,
L5 최종 클래스 `[@-~]`로 확장·U+009B 제거, L6 정규화 단계 순서, L7 `.lines()`, L8 분기 순서(exclusion을 뒤로),
L9 exclusion 항목 누락, L10 lint/hook 우선순위 스왑, L11 게이트 제거·전멸 폴백 제거.

## 3. Deferred
- **M2** 프롬프트 빌더 — 정찰 열린질문 Q4(비-UTF8 경로), Q5(`Conflicted`/`Other` 렌더), **Q6(area 파생 =
  진짜 비용)**, Q2(credential 정제 divergence), Q3(error의 ANSI 정제) 결정 후.
- `classify_push_outcome`(`remote.rs:275`) 통합 여부(Q7) — **분리 유지 권장**(어휘·대소문자 규칙이 다름).
- 소비자 배선("Push blocked" 패널·Details 디스클로저) = 사람눈.

## 4. 순서
단일 PR. 불변식: `(?i-u)` ASCII 폴딩(L1), 클래스별 처리(L2), char 단위+경계 안전(L3), js_ws 재사용(L4),
ANSI verbatim·좁은 클래스 유지(L5), 정규화 5단계 순서(L6), LF-only 분할(L7), 7분기 순서·exclusion 무조건(L8),
21개 verbatim(L9), lint>hook(L10), 저신호 게이트(L11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[js-lowercase-two-mechanisms]], [[suaegi-impl-model-sonnet]]
