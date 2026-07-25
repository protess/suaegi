# Plan — task-query (검색 쿼리 DSL 파서) 확정

조사: `docs/superpowers/research/2026-07-24-task-query.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS**(공개 표면·round-trip·규칙 어휘 전부 확인, 정정 4건 +
4개 결정질문 답변). 이 문서가 구현 계약이며 조사를 supersede한다. 인용은 별도 명시 없으면
`src/shared/task-query.ts`.

## 0. 결정 (조사 + Codex 확정)

Orca task/work-item 리스트의 **검색 쿼리 DSL 파서**(`is:pr state:open author:alice label:bug ...`).
자기완결 순수 모듈(**import 0**), 33-케이스 오라클. 소비자는 task-list UI(사람눈, defer) — 이 플랜은
**순수 파서만**. GitHub PR/issue 지향 필터(is:pr·is:issue·review-requested 등).

**크레이트: 새 leaf `suaegi-taskquery`** (suaegi-fuzzy/suaegi-keys 선례 — 자기완결 순수, 타 suaegi 크레이트
의존 0, **regex 크레이트 불필요** — hand-roll 상태기계). round-trip은 **구조적 멱등**(raw-string identity 아님).

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — JS `\s`/trim은 hand-rolled ECMAScript 술어(Rust `is_whitespace`/`str::trim()` 금지).** 4개 `\s`
  regex(`:34`,`:166`,`:290`[`/^repo:[^\s]+$/i`],`:293`[`/\s/`]) + 4개 `.trim()`(`:73`,`:105`,`:161`,`:289`)가
  전부 JS whitespace 집합을 쓴다. **정확한 코드포인트**: U+0009–000D, U+0020, U+00A0, U+1680, U+2000–200A,
  U+2028–2029, U+202F, U+205F, U+3000, U+FEFF. **U+0085·U+180E 제외.** (= ECMAScript `\s`=trim 집합, 이미
  suaegi-search `js_trim`·suaegi-automation cron과 동일 집합.) 공유 술어 `is_js_whitespace(char)` +
  `js_trim(&str)` 하나로 4 regex·4 trim 사이트 전부 처리. 조사 문서가 line 290/293을 뒤바꿔 기재했고 `:289`
  trim을 누락했으니 **소스 라인 그대로**.
- **C2 — case-fold는 `to_ascii_lowercase`.** 3개 `.toLowerCase()`(`:74`,`:106`,`:134`)는 전부 ASCII 리터럴
  (`is:pr`/`author`/`open` 등 `:75-140`)과만 비교 → `to_ascii_lowercase`가 인식 결과 완전 보존(full Unicode
  lowercase의 allocation/확장 서프라이즈 회피). `to_lowercase` 금지.
- **C3 — quote round-trip 결함은 verbatim 재현 + 문서화 + 회귀 핀(고치지 말 것).** 소스 결함 2종:
  (a) `quoteIfNeeded`(`:166`)가 whitespace 포함 값에 `\"` 이스케이프를 방출하는데 tokenizer(`:32-48`)엔
  **백슬래시 분기가 없어** `label:"a \"b"`가 `["a \b"]`로 mangle된다; (b) whitespace 없는 `a"b`는 `:166`이
  래핑 안 하지만 tokenizer가 `"`를 구조적 여는-따옴표로 소비해 `ab`가 된다. **Rust tokenizer를 고치면 Orca가
  mangle하는 입력을 수용해 source-faithful 계약 위반** → 결함 보존 + **두 실패를 회귀 테스트로 핀**. 후일 수정은
  Orca+포트 동시 승인 behavior-change로만.
- **C4 — tokenizer는 hand-roll 상태기계(regex 크레이트 금지).** 실동작은 이미 작은 상태기계(`quote` 상태
  `:22`, 문자 루프 `:32`). 남은 regex는 단일-문자 JS whitespace 판정 + anchored `repo:` 검사뿐 — hand-roll +
  C1 술어가 Rust regex Unicode 시맨틱보다 작고 충실.

## 2. 마일스톤

### M1 — task-query 파서 전체 (`suaegi-taskquery` 신규, 단일 마일스톤)
자기완결 순수 모듈이라 한 PR로 이식. `is_js_whitespace`/`js_trim`(C1), 타입 `ParsedTaskQuery`(`:1-11`:
scope `all|issue|pr`, state `open|closed|all|merged|null`, draft bool, assignee/author/reviewRequested/
reviewedBy `Option<String>`, labels `Vec<String>`, freeText String), `TaskQueryFilterKey`(`:213-220`).
함수: `tokenize_search_query`(`:53`, hand-roll 상태기계 — 따옴표 열기/닫기, **백슬래시 미처리 verbatim C3**,
JS-`\s` 분할), `parse_task_query`(`:57-171`: is:-폼 `:75-97`, key:value 규칙 `:106-140`, `to_ascii_lowercase`
정규화 C2, unknown→freeText 폴백 `:146-148`, freeText join+js_trim `:161`), `serialize_task_query`
(`:173-211`: **캐논 순서** scope→state→draft→author→assignee→review-requested→reviewed-by→labels→freeText,
`quote_if_needed` `:166` **`\"` 이스케이프 verbatim C3**), `with_qualifier`(`:227-285`), `strip_repo_qualifiers`
(`:287-303`: `rawQuery.trim()` `:289`, anchored `repo:` `:290`, JS-`\s` `:293`).

**오라클(33 케이스, 전부 이식):** 토크나이저 5(`test.ts:11-38`), 파서 12(`:40-114`), repo-strip 6(`:122-145`),
serializer 3(`:151-167`: **구조적 round-trip `parse(serialize(parse(raw)))==parse(raw)` `:152-154`** + exact-string
`is:pr state:all` `:165-166`), withQualifier 7(`:171-210`).

**추가 핀(Codex, 오라클 미커버):** (1) **C1 JS-`\s` 발산** — `\u{FEFF}` 분할됨(JS ws), `\u{0085}` 분할 안 됨
(non-ws) [Rust `is_whitespace`면 반대]; (2) **C2 case-fold** — 비-ASCII가 ASCII 토큰으로 안 접힘; (3) **C3 quote
결함** — `label:"a \"b"`→mangle 재현, `a"b`→`ab` 재현(둘 다 회귀 핀, "의도적 결함 보존" 주석).
*mutation:* JS-ws 술어를 `is_whitespace`로, `to_ascii_lowercase`를 `to_lowercase`로, 캐논 순서 뒤섞기,
tokenizer에 백슬래시 처리 추가(C3 위반), 규칙 인식 각각.

## 3. Deferred (명시)
- **task-list 필터 UI**(쿼리 입력·칩·자동완성) = 사람눈.
- **quote round-trip 결함 수정**(C3) — Orca 오라클 미커버, verbatim 보존. 후일 Orca+포트 동시 behavior-change.
- 규칙 어휘 확장(negation `-qualifier`/`is:not:` 없음 — 전부 freeText 폴백, `:146-148`).

## 4. 순서 (확정)
M1 단일 마일스톤(파서 전체 + 오라클 + C1-C3 추가 핀). 불변식: JS-ws 술어 hand-roll(C1), `to_ascii_lowercase`
(C2), quote 결함 verbatim+핀(C3), regex 크레이트 미도입(C4), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]], [[subagent-output-untrusted]]
