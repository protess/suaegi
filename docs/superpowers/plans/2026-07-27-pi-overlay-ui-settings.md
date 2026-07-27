# Plan — pi-overlay-ui-settings (`suaegi-misc`에 모듈 1개, 단일 PR)

조사: Explore 정찰 **2회가 독립적으로** 이 모듈을 커버했고 **모든 항목에서 일치**했다
(`2026-07-27-ui-prefs-normalizers.md` 조사 + `2026-07-27-terminal-title-strippers.md` 조사).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 17L / 오라클 37L. import 0, 외부 의존 0.

## 0. 배치 — 이전 두 계획의 결론을 **뒤집는다**
`2026-07-27-ui-prefs-normalizers.md` §0은 "자체 leaf 크레이트"라고 썼다. **그걸 이걸로 대체한다.**
[[suaegi-misc-placement-rule]]을 적용하면: `JsValue`/`JsRecord`를 **로컬 private 사본**으로 두면
인트라 크레이트 import 0 · 외부 의존 0 → **`suaegi-misc` 자격을 만족**하고 `[dependencies]`는 계속 비어 있다.
**소스 17L·생산 소비자 0개짜리를 위해 크레이트를 세우는 게 더 나쁘다.**
⚠ 유일한 반론은 톤이다 — 크레이트 헤더가 멤버를 "순수 문자열/숫자 변환"이라 부르는데
~70L짜리 값 트리는 현 멤버 중 가장 무겁다. 그래도 크레이트 신설보다 낫다고 판단한다.
**`serde_json` 금지** — `Value::Object`는 `BTreeMap`이라 키를 **재정렬**하고,
`preserve_order`는 cargo feature 통합으로 **워크스페이스 전역에 전파**되므로 영구 금지다(P1).

## 1. `JsValue`/`JsRecord` — **3번째 사본**, 단 축소판
기존 2벌: `suaegi-filedrop/src/lib.rs:297-376`(~80L), `suaegi-quickcmd/src/lib.rs:294-392`(~83L).
모듈별 복제는 이 리포의 확립된 헌장이다. **크로스 크레이트 재사용 금지.**
필요한 부분집합만 옮긴다:
- `enum JsValue { Null, Undefined, Bool(bool), Number(f64), Str(String), Array(Vec<JsValue>), Object(JsRecord) }`
- `struct JsRecord(Vec<(String, JsValue)>)` + `get`, 그리고 **in-place-overwrite-else-append** `set`
  (`filedrop:364-371`의 `with`가 **정확히 이 의미론**이다 — 그 모양을 가져온다).
- **P2 — `isPlainRecord`는 `matches!(v, JsValue::Object(_))`로 접힌다.**
  JS의 `typeof === 'object' && !== null && !Array.isArray`는 `Date`/`Map`/클래스 인스턴스도 **통과**시키지만
  `JsValue` 트리엔 그런 variant가 **없다**. → **발산이 아니라 모델링 결정**이므로 주석에 명시.
  ⚠ `typeof fn === 'function'`이라 **함수는 원래도 탈락**한다 — 이건 접어도 무손실.

## 2. 계약 결정

- **P3 — ⚠⚠ 키 순서가 **Rust에선 진짜 선택**이다**(JS에선 공짜).
  `merged.terminal = …`(`:13`)과 `merged.hideThinkingBlock = …`(`:14`)은 JS `[[Set]]`이라
  **이미 있는 키는 제자리에서 덮어쓰고**, 없으면 **뒤에 붙는다**(소스 순서: `terminal` → `hideThinkingBlock`).
  → 비-레코드 입력의 정확한 순서는 **`["terminal", "hideThinkingBlock"]`**.
  → 오라클 1번 픽스처(`defaultProvider`, `hideThinkingBlock`, `packages`, `terminal` 순)에서는
  **`hideThinkingBlock`이 2번 슬롯에 그대로 남는다** — 맨 뒤로 가지 **않는다**.
  ⚠ 오라클이 전부 `toEqual`(순서 무시)이라 **어떤 순서든 통과**한다 → **직접 핀**.
- **P4 — ⚠ 깊이(shallow vs deep)와 입력 변경(mutate vs copy)이 **둘 다 오라클로 판별 불가**다.**
  `terminal`이 딱 한 겹만 여분 키를 갖고(`showImages`), 두 단언 모두 **반환값만** 본다 —
  호출 뒤 입력을 다시 읽는 테스트가 **0개**다. → 재귀 병합 포트도, 입력을 제자리 변경하는 포트도 통과.
  → 핀: **호출 후 입력 불변** + `terminal` 아래 2단 이상 값이 **모양 그대로 통과**.
  Rust에선 `&JsValue`를 받아 **소유 `JsRecord`를 반환**하면 변경 자체가 불가능해진다(구조적 방어).
- **P5 — 비-레코드 `terminal`은 **통째로 버려진다****(`:10`). `'compact'`는 핀이 있지만
  `null`·`[]`·`42`·부재는 **커버리지 0**(전부 같은 `{}` 가지).
- **P6 — ⚠ `null`이 **유일한** 비-레코드 최상위 픽스처다.** `undefined`·문자열·수·불리언·**배열**·함수가 전부 미검증.
  **배열이 위험하다**: `Array.isArray` 검사를 빠뜨린 포트는 `['a','b']`를 `{"0":"a","1":"b"}`로 펼쳐
  **완전히 다른 출력**을 내면서 오라클을 통과한다.
- **P7 — 두 상수가 **둘 다 `true`**라 **서로 바꿔도 통과**한다.**
  값으로는 구별 불가이고 **어느 키에 얹히는지만** 고정돼 있다.
  → Rust에서 상수 하나로 합쳐도 아무 테스트가 안 잡는다. **합치지 말 것** — 별개의 제품 결정이다.
- **P8 — 세 대입은 **무조건**이다**(`:12`,`:13`,`:14`). 입력에 `hideThinkingBlock: false`가 있어도 `true`로 덮인다
  (핀 있음). `terminal.clearOnShrink`도 마찬가지.
- **P9 — 반환은 **항상 새 최상위 객체**다.** 비-레코드 입력이면 `{}`에서 시작한다.
- **P10 — 모델링 한계 2가지를 **주석으로 명시****(구현하지 않는다):
  ① JS 객체는 **정수형 키를 먼저, 오름차순으로** 정렬한다(`{"7":a,"0":b}` → `0`,`7`) — `Vec` 사본은 재현 안 함;
  ② spread는 **Symbol 키도 복사**하고 **getter를 호출**한다 — `JsValue` 트리에 대응물 없음.
  실제 JSON 유래 입력으론 **도달 불가**하므로 구현이 아니라 문서로 처리한다.
- **P11 — ⚠ 이 모듈은 **생산 소비자가 0개**다.** 리포 전수 grep이 자기 테스트 하나만 찾는다.
  실제 오버레이 경로는 오히려 `settings.json`을 **무수정 복사**한다고 단언한다
  (`relay/plugin-overlay.test.ts:146-161`, `main/pi/titlebar-extension-service.test.ts:82-117`).
  → **클론 완성도를 위해 이식하되**, "상류가 이미 이 정책에서 후퇴했다"를 doc 주석에 남기고
  **어디에도 배선하지 않는다**.

## 3. 오라클 & 핀
**오라클 3단언 전량**(`pi-overlay-ui-settings.test.ts:5-25`, `:27-31`, `:32-35`).

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**P3 키 순서**(비-레코드 입력 → `["terminal","hideThinkingBlock"]` 정확히; 1번 픽스처에서
`hideThinkingBlock`이 **2번 슬롯 유지**); **P4 호출 후 입력 불변** + 2단 중첩 통과;
**P6 배열 입력**(`["a","b"]` → 펼쳐지지 **않음**) + `undefined`·문자열·수·불리언;
P5 `terminal`이 `null`·`[]`·`42`·부재; P7 두 키가 **각각** 올바른 상수를 받음(교환 감지);
P8 `true` 입력도 `true`로 유지 + `clearOnShrink` 없는 `terminal` 레코드;
P9 빈 레코드 입력 → 두 키만; P2 `Object`만 레코드로 인정.

*mutation:* P2 `Array`도 레코드로 인정, P3 순서를 `hideThinkingBlock` 먼저·기존 키를 뒤로 이동·정렬 추가,
P4 입력 제자리 변경·재귀 병합, P5 비-레코드 `terminal` 보존, P6 배열 가드 제거,
P7 두 상수 교환·하나로 합침, P8 대입을 조건부로(`if absent`), P9 입력 객체 재사용.

## 4. 순서
단일 PR. export가 함수 하나뿐이라 쪼갤 seam이 없다.
크레이트 헤더의 모듈 수(현재 twenty)·목록·`Cargo.toml` 설명을 같이 고친다(신규 모듈은 **v1.4.146-rc.0**).
불변식: **`suaegi-misc` + 로컬 3번째 사본**(§0·§1), `serde_json` 금지(§0),
`Object`만 레코드(P2), **키 순서 명시**(P3), **불변 입력 + shallow 2단**(P4), 비-레코드 `terminal` 파기(P5),
**배열 가드**(P6), 상수 2개 분리 유지(P7), 무조건 대입(P8), 새 객체 반환(P9),
모델링 한계 문서화(P10), **미배선 + 상류 후퇴 기록**(P11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-misc-placement-rule]],
[[orca-source-location]], [[suaegi-impl-model-sonnet]]
