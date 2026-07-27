# Plan — gitlab-projects (`suaegi-misc` 모듈 1개, 단일 PR)

조사: `2026-07-27-github-input-bounds.md`와 **같은 Explore 정찰**(3모듈 일괄).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 25L / 오라클 53L. 런타임 import 0.
3모듈 배치의 **PR 2/2**(PR 1은 #111 머지). 결정 클러스터가 PR 1과 **완전히 분리**된다 —
클럭 시그니처 / JS `slice` 의미론 / 신규 공개 구조체. 공유 코드도 공유 함정도 0.

## 0. 배치 — `suaegi-misc`
[[suaegi-misc-placement-rule]]: 유일한 import가 `import type`(런타임 간선 0), 외부 의존 0.
⚠ 크레이트 헌장이 "**no clock**"이라고 명시하지만 그건 **import에 대한 진술**이다.
L1처럼 `now: &str`로 받으면 이 모듈은 **클럭을 import하지도 읽지도 않는다** → 헌장 그대로 만족.
선례: `suaegi-claude-roster`("모듈은 클럭을 읽지 않는다; 모든 타임스탬프는 호출자가 준 `i64` ms 파라미터").

## 1. 계약 결정

- **L1 — ⚠⚠ `now`는 **이미 포맷된 `&str`**로 받는다. `toISOString`을 이식하지 않는다.**
  TS는 `now: Date = new Date()`를 받아 **딱 한 곳**(`:24`)에서 `now.toISOString()`으로만 쓴다.
  비교도, 정렬도, 산술도 없다 → **관측 가능한 기여가 그 문자열 하나뿐**이다.
  직접 포맷하려면: 그레고리력 일수↔날짜 변환, 0-패딩, **항상 소수점 3자리**,
  0000–9999 밖의 **확장 연도 `±YYYYYY`**, 그리고 **invalid Date에서 `RangeError` throw** —
  약 40줄에 오라클 커버리지 0인 새 실패 모드가 생긴다. `chrono`를 넣으면 **빈 `[dependencies]`가 깨진다**.
  → `now: &str`, **required, 기본값 없음**(클럭이 없으니 `None`이 폴백할 대상이 없다).
  TS의 `= new Date()` 기본값은 **호출자 책임**임을 doc에 명시. 잃는 것은 `RangeError` 팔 하나 —
  `new Date()`는 절대 invalid가 아니고 유일한 생산 호출자는 `now`를 아예 안 넘긴다.
  ⚠ **보너스 근거**: 오라클이 기대값을 리터럴이 아니라 `fixedNow.toISOString()`로 쓴다(`test:9`,`:21`) —
  **`toISOString`을 `toISOString`과 비교하는 자기참조 검사**라 자기일관적인 틀린 포맷터는 **보이지 않는다**.
  `&str` 순수 통과면 이 문제 자체가 사라진다.
- **L2 — dedupe 키는 `host === host && path === path`**(`:23`), 엄격 문자열 동등 →
  **대소문자 구분**, trim 없음, 호스트 소문자화 없음. `filter`이므로 **일치하는 걸 전부 제거**한다.
  ⚠ `&&`는 고정돼 있다(`||`면 오라클이 깨진다). **대소문자 구분은 미고정**(혼합 케이스 픽스처 0).
  ⚠ **같은 `(host,path)`가 여러 개인 입력이 없다** → **remove-all vs remove-first가 구별 불가** → 핀.
- **L3 — filter를 prepend **앞에** 한다**(`:23`→`:24`). 순서는 **관측 가능**하다:
  새 head 자신이 필터 술어를 만족하므로 prepend-then-filter-all은 **방금 넣은 항목을 지운다**(오라클이 잡는다).
  ⚠ 그러나 **prepend-then-keep-first-occurrence는 진짜 등가**다(엔트리 필드가 3개뿐이고 head가 완전 신규).
  → **mutation 대상이 아니라 등가로 문서화**한다([[mutation-survivor-triage]] 원인 ②).
- **L4 — ⚠ `.slice(0, max)`는 `Vec::truncate`와 **한 값 부류에서 갈린다**.**
  JS는 `ToIntegerOrInfinity(end)` 후 `final = end < 0 ? max(len+end, 0) : min(end, len)`:
  | `max` | JS | 순진한 `truncate(max as usize)` |
  |---|---|---|
  | `-1` | **마지막 1개만 버림**(len-1개 남음) | `-1.0 as usize`가 **0으로 포화** → **전부 버림** ✗ |
  | `NaN` | `0` → `[]` | 우연히 일치 |
  | `2.9` | 0 방향 절단 → 2개 | 일치 |
  | `+∞` | 전체 | 포화 후 no-op → 일치 |
  → **`Option<f64>` + `ToIntegerOrInfinity` 헬퍼**로 쓴다(#111의 K2와 같은 논거: 시그니처가 계약이다).
  생산도 오라클도 `max`를 **한 번도 안 넘기므로** `usize` 포트는 100% green이다.
- **L5 — 입력을 변경하지 않고 반환 배열은 **새 것**이다**(`filter`/스프레드/`slice`로 3개 할당).
  ⚠ 오라클의 "불변" 테스트(`test:47-52`)는 **제거 대상이 아닌 엔트리**를 쓴다 →
  제자리 splice 포트도 **살아남는다**. Rust에서 `&[…]`를 받으면 **구조적으로 표현 불가**가 되지만,
  나중에 `&mut`로 리팩터될 때를 대비해 **케이스는 핀으로 남긴다**.
  ⚠ JS는 살아남은 엔트리를 **참조로** 복사한다(별칭 관측 가능). Rust 소유 `Vec`은 클론이라 별칭이 표현 불가 → 주석.
- **L6 — ⚠⚠ 살아남은 엔트리의 `lastOpenedAt`은 **갱신되지 않는다**. head만 `now`를 받는다.**
  그런데 오라클이 이걸 **한 번도 단언하지 않는다**(`:20`은 `.path`만 map, `:21`은 `result[0]`만,
  `:28-29`는 `toMatchObject({host,path})`) → **모든 엔트리에 `now`를 찍는 구현이 오라클 전량 통과**한다.
  → 핀: `result[1].last_opened_at == "2026-05-07"`(원래 값 그대로).
- **L7 — `GITLAB_RECENTS_MAX = 10`이 **심볼 참조뿐**이다** → **리터럴 핀**.
  ⚠ 게다가 오라클이 `lastOpenedAt: \`2026-05-0${i}\``로 픽스처를 만들어서, MAX가 11이 되면
  템플릿이 조용히 `2026-05-010`으로 망가지는데도 **테스트는 그대로 통과**한다.
- **L8 — 타입은 필드 3개 전부 **非옵션 `String`**`(gitlab-types.ts:235-238`):
  `struct GitLabRecentProject { host: String, path: String, last_opened_at: String }` + `PartialEq`.
  ⚠ `lastOpenedAt`은 타입상 **그냥 문자열**이다(Date 아님) — L1의 `&str` 결정과 일관된다.
  **`serde` 없음**(모듈이 파싱도 직렬화도 하지 않는다; 선례 `claude_roster`·`project_runtime`).
  `pinned` 필드는 **범위 밖**(모듈이 건드리지 않고 호출자가 그대로 통과시킨다).
- **L9 — 상류에 **recents 기록 경로의 통합 커버리지가 0**이다**(`ipc/gitlab.test.ts:114`가
  `recordGitLabProjectRecent`를 `vi.fn()`으로 **목킹**한다). 기록만 한다.

## 2. 오라클 & 핀
**오라클 5케이스 전량**(`gitlab-projects.test.ts:7-52`).

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**L6 살아남은 엔트리의 `last_opened_at` 보존**(모든 엔트리 스탬핑 포트를 죽이는 유일한 핀);
**L7 `GITLAB_RECENTS_MAX` 리터럴**; **L4 `max` 6팔**(`None`·`0`·`-1`·`NaN`·`2.9`·`+∞`, 특히 **`-1`이 마지막 1개만 버림**);
**L2 대소문자 구분**(`GitLab.com` vs `gitlab.com`은 **별개 항목**) + **같은 `(host,path)` 중복 2개 → 전부 제거**;
L5 **제거 대상 엔트리로** 불변 확인; `existing.len() > max`로 진입하는 경우;
빈 host/path; L1 `now`가 순수 통과됨(임의 문자열도 그대로 나옴).

*mutation:* L1 `now`를 파싱/재포맷, L2 `||`로·소문자화 추가·`position()+remove()`로(첫 개만 제거),
L3 prepend를 filter 앞으로, **L4 `truncate(max as usize)`로**, L5 입력 제자리 변경,
**L6 모든 엔트리에 `now` 스탬핑**, L7 상수 값 변경, L8 필드 순서·`PartialEq` 제거.
**L3의 prepend-then-keep-first는 mutation 대상 아님**(등가 — §1 증명).

## 3. 순서
단일 PR. export가 상수 1개 + 함수 1개 + 구조체 1개뿐이라 쪼갤 seam이 없다.
크레이트 헤더 모듈 수(현재 twenty-eight)·목록·`Cargo.toml` 설명을 같이 고친다(신규 1개는 **v1.4.146-rc.0**).
불변식: `suaegi-misc`(§0), **`now: &str` 필수·기본값 없음**(L1), 대소문자 구분 + 전부 제거(L2),
filter 먼저 + keep-first는 등가(L3), **JS `slice` 의미론**(L4), 불변 입력(L5),
**살아남은 엔트리 타임스탬프 보존**(L6), 상수 리터럴(L7), 非옵션 3필드 + serde 없음(L8),
상류 커버리지 공백 기록(L9), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[mutation-harness-mtime-trap]],
[[suaegi-misc-placement-rule]], [[orca-source-location]], [[suaegi-impl-model-sonnet]]
