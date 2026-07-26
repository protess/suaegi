# Plan — workspace-cleanup (신규 `suaegi-workspace-cleanup` 크레이트, 의존 0, 단일 PR)

조사: Explore 정찰(`workspace-cleanup.ts` 240L + `.test.ts` 129L 통독, 상류 생산자·소비자 실측).
**소스에 import가 0개**다. 함수 9개 전부 total(throw·async·IO 없음).

⚠ **오라클이 6케이스뿐이고 관측 가능한 동작은 ~40개다.** 이 PR의 가치는 대부분 **직접 쓰는 핀**에 있다.

## 0. 배치 — 신규 leaf, `[dependencies]` **완전히 빈다**

- `suaegi-misc` 헌장은 "작은 자족 순수 문자열/수치 헬퍼"인데 이건 **16-variant blocker 어휘 + 3-tier 격자 +
  dismissal 프로토콜 + fingerprint 스킴** = 정책이다. `suaegi-project-runtime`이 같은 이유로 분리했다.
- **`js_trim` 불필요** — 이 파일엔 `.trim()`이 **0회**다(최근 포팅 모듈 중 처음).
  `regex` 0회, `toLowerCase` 0회, 정규식 0개 → 케이스폴딩 함정 **없음**.
- **`serde`/`serde_json` 불필요** — 파싱도 직렬화도 안 한다. fingerprint는 `Array.join`이지 `JSON.stringify`가 아니다.
  (⚠ 이 타입들은 Orca에서 IPC 페이로드다 → **나중에** optional feature로 추가할 것. 그때
  `createdAt`은 `skip_serializing_if = "Option::is_none"`이 필요하다 — 부재와 null이 다르다.)
- **`chrono` 불필요하고 유해하다** — 모듈이 시계를 **읽지 않는다**. `scannedAt`은 파라미터다.
- **수치 모델은 `i64` 밀리초.** 그러면 `O:197`의 `|| 0`이 no-op이 되고 `join`의 문자열화가
  `i64::to_string()`과 정확히 일치해 `format_ecmascript_float` 복사가 **불필요**해진다(G13).

## 1. 계약 결정

- **G1 — ⚠ `WORKSPACE_CLEANUP_HARD_BLOCKERS`는 **union 전체 16개**다 → `is_hard_blocker`는 사실상 상수 `true`.**
  그래도 **16개 arm을 명시적으로** 적는다(`matches!` 또는 `const [Blocker; 16]`).
  `blockers.is_empty()`로 "단순화"하면 오늘은 동일하지만 **17번째 variant가 추가될 때 정책이 조용히 바뀐다**.
  ⚠ **세 집합을 하나로 합치지 말 것** — `QUEUE_BLOCKERS`(3개)와 `FORCE_REMOVE_BLOCKERS`(4개)의
  **비대칭이 `T:74-90`의 요점**이다(`unpushed-commits`는 queue 가능하지만 force-remove 대상).
- **G2 — `WORKSPACE_CLEANUP_QUEUE_BLOCKERS`는 **export되지 않는다**(`O:132`).** 셋 중 유일하게 `export`가 없다.
  Rust에서 `pub`으로 만들면 소스보다 API 표면이 넓어진다 → **private**.
- **G3 — ⚠ 두 임계값 비교는 **둘 다 `>=`**(포함)이고 **오라클 커버리지 0**이다.**
  `O:214` `scannedAt - lastActivityAt >= ARCHIVED_IDLE_MS`(7일, `isArchived &&` 가드 있음),
  `O:218` `>= IDLE_MS`(30일, **가드 없음**). 정확히 임계값이면 **발동한다**.
  두 상수는 테스트가 **import조차 안 한다** → `>` 오타·상수 교환·`isArchived` 가드 제거를
  **아무 테스트도 못 잡는다**. `Δ = 임계-1 / 임계 / 임계+1` 경계 핀 **필수**.
  ⚠ 두 조건은 **독립**이라 archived + 30일이면 **`['archived','idle-clean']` 둘 다** 나온다
  (ARCHIVED < IDLE이므로 도달 가능). 순서는 **archived 먼저**(`O:216` → `O:219`).
- **G4 — ⚠ 시간 뺄셈은 **부호 있는 `i64`**로. `saturating_sub` 금지.**
  미래 타임스탬프/시계 어긋남이면 차이가 **음수**가 되고 `음수 >= 임계`는 false → **idle 아님**.
  `u64` + `checked_sub().unwrap_or(u64::MAX)`("안전하게")는 이걸 **"최대로 idle"로 뒤집는다**.
- **G5 — `Math.floor`는 −∞ 방향, Rust `/`는 0 방향 → **`div_euclid` 필수**.**
  `O:197` 버킷 = `floor(lastActivityAt / 86_400_000)`. `-86_400_000 < x < 0`에서 JS는 `-1`, Rust `/`는 `0`.
  오라클은 이걸 못 잡는다 — **fingerprint 반환값을 한 번도 들여다보지 않는다**(자기 자신과만 비교).
- **G6 — `??`(`O:196`)와 `||`(`O:197`)가 여섯 줄 간격으로 **의도적으로 다르다**.**
  `classifierVersion: 0`은 **0으로 살아남고**(`??`), `lastActivityAt`의 `0`/`NaN`은 `0`이 된다(`||`).
  `i64` 모델에서 후자는 no-op. 전자를 "0이면 기본값"으로 쓰면 틀린다. 커버리지 0.
- **G7 — ⚠ fingerprint 형식이 **완전히 미검증**이다.** `T:104-109`이 호출은 하지만 **반환값을 검사하지 않는다**
  → `String(head)`를 반환해도 오라클을 통과한다.
  형식은 `{version}|{branch}|{head}|{clean|dirty|unknown}|{bucket}`, `join('|')`.
  3-state 매핑(`O:202`): `null`→`unknown`, `true`→`clean`, `false`→`dirty`
  (⚠ 두 번째는 truthiness라 런타임 `undefined`는 `dirty`; `Option<bool>`이면 무관).
  ⚠ **구분자 이스케이프가 없다** — `branch`가 `a|b`면 충돌한다. **재현할 것, 고치지 말 것**
  (`T:124`가 `|changed`를 덧붙여 만든다 = 실제 6필드 fingerprint와 같아질 수 있는 값).
- **G8 — ⚠ `classifierVersion` 절을 **통째로 빼도 오라클을 통과한다**.**
  `O:238`은 **모듈 상수**와 `===` 비교한다(파라미터 아님). 오라클은 `T:117`/`T:125`에서 **그 상수를 그대로**
  넘기므로 절 3이 없어도 두 단언이 다 통과한다 — **이 파일 최대의 픽스처 우연**.
  → 상수를 읽는 형태를 유지하고(테스트 편의로 파라미터화 **금지**), **버전 불일치 핀을 직접 쓴다**.
  ⚠ 비교는 `==`이지 `>=`가 아니다 → **다운그레이드도 똑같이 무효화**한다.
- **G9 — ⚠ `should_force_removal`이 **`blockers.some(FORCE_REMOVE)` 하나만으로도 오라클을 통과한다**.**
  유일한 호출(`T:81`)이 `clean: true, checkedAt: <num>`이라 앞의 두 disjunct가 **한 번도 발동하지 않는다**.
  그리고 이 함수가 **`false`를 반환하는 것도 한 번도 단언되지 않는다**.
  3-disjunct: `git.clean != Some(true)` **또는** `git.checked_at.is_none()` **또는** force-remove blocker.
  → 세 축을 **독립적으로** 발동시키는 핀 필수.
- **G10 — `git.clean !== true`는 `=== false`가 **아니다**.** `None`(불명)도 `Some(false)`(dirty)와 똑같이
  force-remove를 유발한다(`O:158`). `!clean.unwrap_or(false)`는 여기선 우연히 같지만
  **`unwrap_or` 반사신경은 `suaegi-project-runtime` E3를 깨뜨린 바로 그것**이다 → 관례로 금지.
  `!= Some(true)`로 쓴다.
- **G11 — `git.checkedAt === null`은 **엄격**이라 `checkedAt: 0`은 **유효한 검사**다**(`O:159`, `O:170`).
  `0`을 센티넬로 쓰는 `i64` 모델은 둘 다 뒤집는다. `checkedAt`·`upstreamAhead`·`upstreamBehind`·
  `newestDiffCommentAt` **네 개 전부 `Option<i64>`**, 매직 `0`/`-1` 금지.
  ⚠ `T:92-101`이 `clean`과 `checkedAt`을 **함께** 바꾸므로 `O:169`/`O:170` 두 conjunct가 **분리된 적이 없다**
  → 어느 하나를 빼도 오라클 통과. 축별 독립 핀 필수.
- **G12 — `apply_policy`는 **나머지 필드를 전부 보존**해야 하고, **부재한 `createdAt`은 부재로** 남아야 한다.**
  `O:183`의 spread가 `tier`/`selectedByDefault` 둘만 덮어쓴다. 생산자는 `createdAt` 키를 **의도적으로 생략**한다.
  ⚠ **어떤 테스트도 보존을 단언하지 않는다** → 필드별로 재구성하다 하나를 빠뜨려도 오라클 통과.
  그리고 **입력의 `tier`/`selectedByDefault`는 죽은 값**이다(생산자가 placeholder를 넣는다) →
  **읽지 말 것**. 함수는 **멱등**이다.
- **G13 — tier 격자와 세 겹 중복.** `hasHardBlocker → protected`, 아니면 `canSelect → ready`, 아니면 `review`.
  `canSelect`가 마지막 conjunct로 hard-blocker를 **다시 검사**하므로(`O:171`) `canSelect ⟹ !hasHardBlocker`,
  따라서 `O:180`의 삼항 순서와 `O:185`의 `&& canSelect`는 **관측 불가능하게 중복**이다.
  축자 유지한다. ⚠ 단 `selectedByDefault: canSelect`로 "단순화"하면 `O:171`을 빼는 순간 조용히 깨진다 —
  **세 중복이 서로를 지탱한다**(주석으로 명시).
- **G14 — `reasons` 순서는 고정이고 의미가 있다**(`archived` 먼저). `HashSet` 반환이나 두 `if` 순서 교환은
  UI 라벨 순서를 바꾼다. `blockers`는 `apply_policy`가 **입력 순서 그대로** 통과시켜야 한다
  (모듈 내부는 전부 `.some()`이라 순서 무관하지만 하류가 렌더한다).
- **G15 — `should_hide`의 3-절 conjunction은 **전부 `Some` 안에** 있어야 한다.**
  `O:236`의 `?.`가 `dismissal`이 없을 때 절 2·3을 **도달 불가**로 만든다 → `is_some_and`로.
  `dismissedAt`은 **읽지 않는다**(TTL 없음).

## 2. 오라클 & 핀

**오라클 6케이스 전량:** `T:58-64` 기본 → ready/selected / `T:66-72` `reasons: []` → review /
`T:74-82` `unpushed-commits` → protected + **queue 가능** + force-remove / `T:84-90` `main-worktree`·
`folder-repo` → queue 불가 / `T:92-101` `clean:null, checkedAt:null` → review / `T:103-128` dismissal 일치·
fingerprint 변경 시 불일치.

**추가 핀(오라클 침묵 — 이 PR의 실제 가치):**
G3 두 임계 각각 `-1`/`정확`/`+1` 경계·`isArchived` 가드·**두 reason 동시 발생**;
G4 미래 타임스탬프 → reason 없음; G5 음수 `lastActivityAt`의 버킷이 `-1`; G6 `classifierVersion: 0` 유지;
G7 **fingerprint 문자열 전체를 정확히 단언**(`2|feature|abc123|clean|19675`)·`unknown`/`dirty` 두 arm·
`branch`에 `|`가 든 충돌 케이스; G8 **버전 불일치 → hide 안 함**·다운그레이드도 무효·`dismissal: None`·
`worktreeId` 불일치; G9 세 disjunct **각각 단독** 발동 + **`false` 반환** 케이스;
G10 `clean: None`이 force-remove 유발; G11 `checkedAt: Some(0)`은 유효·`clean`/`checkedAt` 축 분리;
G12 **모든 필드 보존**(부재 `createdAt` 포함)·멱등성·입력 tier 무시;
G13 blocker 있는데 reasons도 있는 경우 → protected; G14 `[archived, idle-clean]` 순서·blocker 순서 보존;
G1 16개 blocker **전부** hard·`unpushed-commits`가 queue 가능하지만 force-remove 대상(비대칭);
G2 queue 집합 3개 전부(`dismissed` 포함).

*mutation:* G1 hard 집합에서 variant 제거·세 집합 통합, G3 `>=`→`>`·상수 교환·`isArchived` 가드 제거,
G4 `saturating_sub`, G5 `/` 사용, G6 `??`를 "0이면 기본값"으로, G7 구분자 변경·필드 순서·3-state arm 교환,
G8 **절 3 제거**·`>=` 비교, G9 disjunct 각각 제거, G10 `== Some(false)`, G11 `Option` 대신 `0` 센티넬,
G12 필드 누락·입력 tier 읽기, G13 `selectedByDefault: canSelect`, G14 reason 순서 교환.

## 3. 순서
단일 PR. 정찰도 단일 PR을 권고했다 — `apply_policy` → `can_select` → `is_hard_blocker` 체인이고
`is_old` 는 한 줄 래퍼라 seam이 없다. **테스트 작성이 포팅 물량보다 크다** — 그걸 이 PR 안에서 끝낸다
(후속 테스트 PR로 미루면 fingerprint 형식·두 임계·버전 무효화가 main에 미고정으로 남는다).
불변식: 의존 0·`i64` 밀리초(§0), 16-arm 명시(G1), queue 집합 private(G2), **`>=` 경계**(G3),
부호 있는 뺄셈(G4), `div_euclid`(G5), `??`/`||` 구분(G6), **fingerprint 형식 직접 핀**(G7),
**버전 절 유지 + 직접 핀**(G8), force-remove 3축 분리(G9), `!= Some(true)`(G10), `Option` 4개(G11),
전 필드 보존·멱등(G12), 세 겹 중복 축자 유지(G13), 순서 보존(G14), `is_some_and`(G15),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-impl-model-sonnet]]
