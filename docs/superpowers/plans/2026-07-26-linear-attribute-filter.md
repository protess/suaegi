# Plan — linear-issue-attribute-filter (단일 PR)

조사: Explore 정찰(244L 소스 + 137L 오라클 전문 정독). 대상: `crates/suaegi-tracker/src/linear/attribute_filter.rs` (신규).
**외부 의존 0**(소스에 `import` 한 줄도 없음). `serde_json`은 tracker에 이미 있음.

## 0. 열린 질문 확정 (리드 결정)

- **Q1 (T2/T8) — signature는 suaegi 내부 캐시 키다. JS 피어가 존재하지 않는다.**
  Orca는 Electron이라 renderer↔main이 같은 JS 값을 비교하지만, **suaegi는 포트 전체가 Rust**라 signature 문자열이
  프로세스 경계를 넘어 JS와 **바이트 비교될 일이 없다**.
  → **정렬 비교자는 `str::cmp`(code-point 순)** 로 두고 **문서화된 divergence**로 남긴다. 오라클이 실제로 잠그는
  성질은 "동치 필터 → 동일 signature"(`test.ts:37-39`)이고 그건 **임의의 total order로 만족**된다. astral(≥U+10000)과
  U+E000..U+FFFD의 상대 순서만 JS와 다르다(JS는 UTF-16 code unit 비교라 astral이 앞). 선례:
  `suaegi-git/src/remote_identity.rs`가 `localeCompare`를 포기하고 문서화한 것과 동일한 판단.
- **Q1b (T8) — 그래도 JSON 필드 순서는 Orca와 일치시킨다(비용이 없으므로).**
  `serde_json::Map`은 BTreeMap이라 **알파벳 정렬**(`assignee,labelIds,priorities,stateIds`)로 나가 Orca의 삽입 순서
  (`stateIds,priorities,assignee,labelIds`)와 다르다. → **`#[derive(Serialize)]` struct**를 쓴다(serde struct는
  **선언 순서 유지**). assignee도 `kind` → `id` 순 struct/enum. `Map`/`json!` 사용 금지.
- **Q2 (T3) — id 길이 캡은 `encode_utf16().count()`로 정확히.** 이건 **untrusted 입력에 대한 보안 캡**이므로
  근사(`chars().count()`)가 아니라 JS와 정확히 같은 단위여야 한다. 선례: `suaegi-path/src/cross_platform_path.rs:354`.
  **`str::len()`(바이트) 금지** — `"あ"×256`이 JS 통과/바이트 거부로 갈린다.
- **Q3 (T15) — `{kind:'user', id:'   '}` → `null` 강등을 **축자 유지**.** 놀랍지만 실제 동작이다("assignee 필터 해제",
  `unassigned` 아님). parse 경로는 이미 throw라 wire 입력에선 도달 불가 — canonicalize 직접 호출 경로에서만 발생.
  `Err`로 승격하지 말 것(오라클이 안 잡는 동작을 임의 변경 = 조용한 semantic 변경). **주석 필수.**
- **Q4 (T5) — canonicalize의 lenient drop을 유지.** `priorities: Vec<u8>`로 두고 `> 4` drop을 미러한다.
  newtype으로 out-of-range를 구성 불가하게 만들면 **실제 존재하는 분기를 삭제**하는 것이라 금지.
  ⚠️ 단 `u8` 선택으로 **음수·비정수는 타입상 표현 불가** → JS의 `p < 0`/`!Number.isInteger(p)` drop 분기는
  canonicalize 경로에서 **도달 불가**가 된다(= sanctioned divergence, 주석). parse 경로는 JSON에서 오므로
  그 검증이 **살아 있어야** 한다(아래 P5).
- **Q5 — 에러는 구조화 enum**(필드명 + 인덱스 + 한도 수치 보유). 오라클이 정규식으로 메시지를 검사하므로
  Rust 테스트는 **enum variant + 필드**로 대응한다. 하우스 선례(`linear/write.rs:46-48` `InvalidWriteId`)는
  "raw 값을 담지 않는다"이지 "필드명을 담지 않는다"가 아니다 → 필드명/인덱스는 담되 **raw id 값은 담지 않는다**.
- **Q6 — `optional_parsed_*`는 오라클 0개** → 전 경로 신규 핀.
- **Q7 — `EMPTY_*` 싱글턴 identity 의존 없음** → Rust는 `Default` 하나로 흡수(freeze 개념 소멸).

## 1. 계약 결정

- **P1 (T1) — `js_trim` 사용. `str::trim()` 금지.** JS trim은 **U+FEFF 포함 / U+0085 제외**, Rust는 반대.
  `"\u{FEFF}"` 하나짜리 id가 JS에선 빈 문자열로 **drop/reject**되는데 Rust `trim()`은 유효 id로 통과시킨다.
  **`suaegi-misc = { path = "../suaegi-misc" }` 의존 추가**(의존 0개 순수 leaf) — 정찰은 "9번째 로컬 사본"을
  권했으나 **최근 두 마일스톤에서 이미 `suaegi-forge`(M3)·`suaegi-git`(push-failure M1)에 misc 의존 방식을
  채택**했으므로 그 선례와 일관되게 간다.
- **P2 (T12/T13/T11) — 캡 강제 순서·단위 축자.**
  ① 캡은 **원시 배열 길이**에 건다(trim/dedup **전**). ② **truncate 아니라 reject**. ③ 정확히 한계는 **통과**
  (`> N` 비교): 100개 통과/101개 거부, 256자 통과/257자 거부, 5개 통과/6개 거부.
  ④ **배열 캡 검사가 per-entry 검사보다 먼저**(`Array(101)` 중 불량이 섞여도 `exceeds 100`).
  ⑤ **id 길이 캡은 trim 후**(`"    "+'x'×256` → trim 후 256 → 통과).
- **P3 (T10) — parse 검사 순서: null/undefined → **unknown key** → required key → 필드별 assert.**
  `serde(deny_unknown_fields)` **금지**(에러 우선순위를 못 맞춘다). `&serde_json::Value`에서 손으로 검사.
  필드 assert 순서는 **stateIds → priorities → assignee → labelIds**(여러 필드가 동시에 불량이면 stateIds가 먼저).
- **P4 (T9/T17) — `null`/absent 규칙이 함수마다 다르다. 뭉개지 말 것.**
  `parse(value)`: `undefined`와 `null` **둘 다 Err**. `optional_parsed(value: Option<&Value>)`:
  `None`(absent) → `Ok(None)`, `Some(Value::Null)` → **Err**. 반면 `is_empty(filter: Option<&Filter>)`와
  `signature(Option<&Filter>)`는 **None을 empty로 취급**(`!filter` → true / `''`).
  `assignee` 키: **키 없음 → required Err**, **키 있고 null → OK**, `{assignee: undefined}`는 JSON에 없으므로 무관.
- **P5 — priority 검증은 두 정책 공존.** canonicalize: `> 4` **조용히 drop**(Q4).
  parse: `Value::as_u64()`가 None(=비정수/음수/문자열) → **`/integer/` 계열 Err**, 값 `> 4` → **`/0 to 4/` 계열 Err**.
  **두 에러를 구분**해야 오라클(`test.ts:104-119`)이 산다. `1.5`와 `5`가 다른 에러다.
- **P6 — canonicalize는 정렬·dedup 후 출력이 canonical.** 문자열은 `str::cmp`(Q1), 숫자는 오름차순.
  dedup은 **값 기준**(첫 등장 유지지만 정렬이 뒤따르므로 **안정성 관측 불가** → `sort_unstable` 허용, 주석).
  `canonicalize_ids`에는 **캡이 없다**(T22 — 캡은 parse 경로 전용). 넣지 말 것.
- **P7 — "empty"는 **정규화 후** 판정.** `is_empty`는 canonicalize를 거친 뒤 4조건 AND
  (stateIds 비고 ∧ priorities 비고 ∧ assignee None ∧ labelIds 비고).
  ⚠️ **`priorities:[0]`과 `assignee:Unassigned`는 empty가 아니다**(falsy-0 트랩, 오라클 `test.ts:59-69`가 잠금).
  omission이 실제로 일어나는 곳은 **`optional_parsed`(→None)와 `signature`(→`""`)** 두 곳뿐.

## 2. 마일스톤 (단일 PR, 커밋 3개로 분할해 diff 가독성 확보)
`crates/suaegi-tracker/src/linear/attribute_filter.rs` + `linear/mod.rs`·`lib.rs` re-export + `Cargo.toml`에 `suaegi-misc`.
- 커밋1: 타입(`LinearIssueAttributeFilter`, `LinearIssueAttributeAssignee`) + 4 상수 + `Default`.
- 커밋2: `canonicalize_*`, `is_empty_*`, `signature_*` + 오라클 1–4(`test.ts:20-69`).
- 커밋3: `parse_*`, `optional_parsed_*` + 오라클 5(`test.ts:71-136`) + 경계 핀.

**오라클(5케이스 전량):** empty 두 철자 + `signature(None)=""`; trim+dedup+sort 압축 케이스 + **입력 불변** +
signature 멱등 + `"priorities":[0,1,3]` 형태; 4 facet 각각이 signature를 바꿈; **falsy-0**(`[0]`/unassigned는
non-empty); parse 7종 거부(required / unknown key / 빈 id / 비정수 / 범위 / assignee.id 누락 / 101개 캡).

**추가 핀(오라클 침묵):** P2 정확히 100·256·5 **통과** 및 101·257·6 **거부**; `labelIds` 캡·`priorities` 6개 캡
(오라클엔 stateIds만); P4 `parse(Null)` vs `parse(absent)` vs `optional_parsed` 3경로; **`optional_parsed` 전체**;
`assignee: null` vs 키 누락; Q3 `{user, id:"   "}` → None 강등; P1 U+FEFF-only id가 drop/reject(**`str::trim` 대비**);
Q2 비-ASCII 길이(`"あ"×256` 통과, `"あ"×257` 거부; 이모지 128자=UTF-16 256 통과); P6 비-ASCII 정렬 결정성;
최상위가 배열인 경우 → `expected an object`.

*mutation:* P1 `js_trim`→`trim`, Q2 utf16→bytes/chars, P2 캡을 dedup 후로·`>`→`>=`·truncate로·검사 순서 스왑,
P3 unknown/required 순서 스왑·필드 assert 순서, P4 null과 absent 통합, P5 두 에러 통합, P6 정렬 제거·dedup 제거·
canonicalize에 캡 추가, P7 empty에 `[0]` 포함·canonicalize 생략, Q1b 필드 순서(BTreeMap 정렬로).

## 3. Deferred
- **도메인 필터 → GraphQL `IssueFilter` 변환**(`linear/client.rs:152`의 `Option<Value>` 경계에 물리는 부분).
  Orca 소스가 `main/linear/issue-list-filter.ts`로 **별도 파일**이라 이번 정찰 스코프 밖 → 후속 PR.
- 소비자 배선(renderer state ↔ 캐시 키) = 사람눈.

## 4. 순서
단일 PR. 불변식: js_trim(P1), 캡 순서·단위·reject(P2), parse 검사 순서(P3), null/absent 규칙 분리(P4),
priority 두 정책(P5), canonical 출력(P6), 정규화 후 empty + falsy-0(P7), `str::cmp` divergence 문서화(Q1),
Serialize struct 필드 순서(Q1b), UTF-16 길이 캡(Q2), assignee 강등 축자(Q3), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]], [[js-lowercase-two-mechanisms]]
