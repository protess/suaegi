# Plan — worktree-submodule-removal + ephemeral-setup-terminal-worktree-id (`suaegi-misc` 모듈 2개, 단일 PR)

조사: `2026-07-27-git-ref-normalizers.md`와 **같은 Explore 정찰**(5모듈 일괄, 소스·오라클 통독 + 소비자 전수 grep).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 41L / 오라클 72L. import 0, 외부 의존 0.
5모듈 배치의 **PR 2/2**(PR 1은 #109로 머지 완료). 두 모듈은 **서로 무관**하다 —
각자 별개의 "추가 핀" 서사를 갖고 크기가 작아 한 diff로 묶는다(`43b1bac`와 같은 모양).

## 0. 배치 — 둘 다 `suaegi-misc`
[[suaegi-misc-placement-rule]]: 런타임 import 0, 외부 의존 0.
`worktree_submodule_removal`은 기존 **`remote_runtime_error`의 구조적 쌍둥이**(`…ErrorLike` 입력 구조체를 받는
에러 형태 분류기)이고 `/i`→ASCII 리터럴 처리는 **`codex_auth_errors` 선례**다 — 다른 데 두면 관용구가 쪼개진다.
⚠ `ephemeral_…_worktree_id`는 **git이 아니다**. 소스 주석(`:1-3`)이 "인라인 setup/온보딩 터미널은
백킹 worktree가 없다"고 밝히고 소비자도 `OnboardingInlineCommandTerminal.tsx`/`runtime-worktree-selector.ts`다.
`stable_pane_id`(브랜디드 id validate/construct 쌍)의 쌍둥이. **git 크레이트에 넣으면 적극적으로 오해를 부른다.**
`regex` 금지(문구 1개, 대안 없음 → 리터럴 하나로 충분).

## 1. 계약 결정 — `worktree_submodule_removal`

- **H1 — ⚠⚠ 헤드라인: `/i` 플래그가 **한 번도 검증되지 않는다**, 그리고 그게 켈빈 기호 함정이 걸린 바로 그 플래그다.**
  정규식은 `/working trees containing submodules cannot be moved or removed/i` — **비앵커(부분문자열), `u` 없음, 문구 1개**.
  양성 픽스처 2개가 **둘 다 전부 소문자**라 **`/i`를 통째로 지워도 오라클이 통과**한다.
  → [[js-lowercase-two-mechanisms]]의 **살아 있는 사례**다:
  JS `/i`는 `/u` 없이 **ASCII만** 접고, Rust `str::to_lowercase`는 **유니코드 전체**를 접는다.
  문구에 **`k`(`wor`**k**`ing`)가 들어 있다** → 입력 `"wor\u{212A}ing trees containing submodules cannot be moved or removed"`
  (U+212A KELVIN SIGN)는 **JS `false` / `to_lowercase` 포트 `true`**.
  false positive면 호출자가 **다른 이유로 거부된 worktree에 `git worktree remove --force`를 재시도**한다.
  → **`to_ascii_lowercase` 필수**, `to_lowercase` 금지. **U+212A 음성 핀 필수**(오라클에 없음).
  ⚠ 방향 확인: `ſ`(U+017F)는 이미 소문자라 양쪽 안전, `İ`(U+0130)는 `to_lowercase`가 결합문자를 끼워 넣어
  오히려 매치를 **깨므로** 안전. **살아 있는 발산은 U+212A 하나뿐**이고 하필 문구에 있는 글자다.
- **H2 — `getErrorText`의 `unknown` 입력 모델**(`remote_runtime_error` 선례를 따른다):
  ```rust
  pub struct GitErrorFields<'a> { pub message: Option<&'a str>, pub stderr: Option<&'a str>, pub stdout: Option<&'a str> }
  pub enum GitErrorLike<'a> { ObjectLike(GitErrorFields<'a>), Primitive(&'a str) }
  ```
  ⚠ `String(error)`(`:12`)는 **이식 대상이 아니다** — `String(null)→"null"`, `String(1e21)→"1e+21"`(ECMAScript 부동소수 포맷!),
  `String(fn)`→소스 텍스트, `String(Symbol('x'))→"Symbol(x)"`. **호출자가 만들어 넘기는 `Primitive(&str)`**로 모델링하고
  "JS `ToString`은 범위 밖"을 헤더에 명시한다(`suaegi-project-runtime`의 "파싱은 상류에서 끝났다" 논거와 동형).
  ⚠ JS에선 **배열도 `typeof === 'object'`**라 `ObjectLike`로 간다(필드가 없어 `""`). 주석으로 기록.
- **H3 — 필드는 `["message","stderr","stdout"]` **정확히 그 순서**이고 `'\n'`으로 join**(`:4`,`:10`).
  ⚠ 최종 매치가 **비앵커 부분문자열**이라 **공개 술어만으로는 순서도 구분자도 관측 불가**하다 —
  `join("")`, `join(" ")`, "첫 매치 필드만 반환" 전부 통과한다.
  → **`get_error_text`를 `pub`으로 내서** 순서와 `\n` 구분자를 **직접 핀**한다. 이게 이 모듈의 유일한 방어책이다.
- **H4 — `typeof value === 'string' && value`의 **뒤쪽 `&& value`는 truthiness**라 `''`를 **버린다**(`:6`).
  빈 `message`가 선행 빈 줄로 join되지 않는다. Rust: `.filter(|v| !v.is_empty())`. 커버리지 0.
- **H5 — `stdout`은 **픽스처가 아예 없다**.** 필드 배열에서 지워도 green → 핀.
- **H6 — 소비 필드는 셋뿐**이다: `stack`·`cause`·`code`·`output`은 **읽지 않는다**.

## 2. 계약 결정 — `ephemeral_setup_terminal_worktree_id`

- **H7 — ⚠⚠ 헤드라인: 접두사 상수의 **리터럴 값이 리포 전체 어디에서도 단언되지 않는다**.**
  오라클 6단언이 전부 `${PREFIX}` 또는 `brand()` 경유의 **심볼 참조**다.
  음성 2개(`'repo-1::/work/orca/wt'`, `'global-floating-terminal'`)와 `'::'` 미포함 검사만이 리터럴 제약인데
  거의 아무 문자열이나 만족한다 → **상수를 `'x:'`로 바꿔도 전 테스트 green.**
  → **`assert_eq!(EPHEMERAL_SETUP_TERMINAL_WORKTREE_ID_PREFIX, "ephemeral-setup-terminal:")`** 직접 핀.
  ⚠ **후행 콜론이 상수의 일부**다.
- **H8 — `is(brand(x))` 라운드트립은 **공유 상수의 항진명제**라 정보량이 0이다.**
  `brand`의 두 갈래가 모두 `is`로 구동되므로(`:11`) 상수가 틀려도 항상 참이다.
  → 이 리포에서 **다섯 번째** 중복 메커니즘 사례. 라운드트립 테스트를 **커버리지로 세지 말 것**.
- **H9 — `is`는 **맨 `startsWith`**다**(`:18`). 접미사 검증이 **없다**:
  `is("ephemeral-setup-terminal:")`(빈 접미사) → **`true`**;
  `is("ephemeral-setup-terminal:repo-1::/x")` → **`true`**(오라클이 신경 쓰는 `::`가 있어도).
  → 둘 다 핀. **접미사 검증을 "보강"하지 말 것.**
- **H10 — `brand`는 **단사(injective)가 아니다**.** `brand("")` → `is("")`가 false → **맨 접두사**를 반환하고,
  그 값은 `is`를 만족한다. 즉 `brand("")`와 `brand("ephemeral-setup-terminal:")`가 **같은 값**을 낸다. 커버리지 0 → 핀.
- **H11 — 경계 3종이 미검증**: 콜론 없는 **진부분접두사**(`"ephemeral-setup-terminal"` → `false`),
  **중간 등장**(`"x-ephemeral-setup-terminal:y"` → `false`), **대소문자**(`"EPHEMERAL-SETUP-TERMINAL:x"` → `false`).
- **H12 — ⚠ 상류 위험을 **그대로 이식하고 기록**한다.** 실제 `panelId`가 우연히 접두사로 시작하면
  **브랜딩되지 않은 채** 하류(`runtime-worktree-selector.ts:21`)에서 브랜딩된 것으로 취급돼
  floating-terminal 스코프로 라우팅된다. 가드도 주석도 테스트도 **없다**.
  → `escape_cmd_set_value` 선례대로 **동작을 바꾸지 말고** 문서화된 상류 위험으로 남긴다.
- **H13 — trim이 **없다****(`:18`). 호출자가 대신 트림한다(`runtime-worktree-selector.ts:21`). **추가하지 말 것.**

## 3. 오라클 & 핀
**오라클 전량**: `worktree-submodule-removal.test.ts` 3케이스/4단언, `ephemeral-….test.ts` 4케이스/6단언.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**H1 대문자 문구 → `true`**(`/i` 삭제를 죽인다) **+ U+212A 음성**(`to_lowercase` 포트를 죽인다);
H3 `get_error_text`의 **필드 순서와 `\n` 구분자**(세 필드 동시 존재); H4 빈 `message` 제외;
**H5 `stdout` 단독 매치**; H2 `Primitive` 갈래 + 필드 없는 `ObjectLike` → `""`;
**H7 접두사 리터럴**; **H9 `is(PREFIX)` → `true`** + `is(PREFIX + "a::b")` → `true`;
**H10 `brand("")` → 맨 접두사** + 비단사성; H11 경계 3종; H12 충돌 사례를 **동작 그대로** 고정.

*mutation:* H1 `/i` 제거(대소문자 구분 비교)·`to_lowercase`로, H3 join을 `""`/`" "`로·필드 순서 교환·첫 매치만 반환,
H4 truthiness 가드 제거, H5 `stdout` 제거, H6 `stack` 추가, H7 상수 값 변경·후행 콜론 제거,
H9 접미사 비어있음 검사 추가·`contains`로, H10 `brand("")` 특수 처리 추가, H11 `eq_ignore_ascii_case`로,
H13 trim 추가.
**H8 라운드트립은 mutation 대상 아님**(항진명제 — §2 증명 참조).

## 4. 순서
단일 PR. 두 모듈은 무관하지만 각 20L 남짓이고 각자 핀 서사가 뚜렷하다.
크레이트 헤더 모듈 수(현재 twenty-four)·목록·`Cargo.toml` 설명을 같이 고친다(신규 2개는 **v1.4.146-rc.0**).
불변식: `suaegi-misc`(§0), **`to_ascii_lowercase` + U+212A 핀**(H1), 호출자 제공 `Primitive`(H2),
**`get_error_text`를 `pub`으로 내어 순서/구분자 고정**(H3), 빈 문자열 제외(H4), 세 필드(H5/H6),
**접두사 리터럴 핀**(H7), 라운드트립은 무정보(H8), **맨 `startsWith`**(H9), 비단사 `brand`(H10),
경계 3종(H11), **충돌 위험 기록만**(H12), trim 없음(H13), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[js-lowercase-two-mechanisms]],
[[suaegi-misc-placement-rule]], [[orca-source-location]], [[suaegi-impl-model-sonnet]]
