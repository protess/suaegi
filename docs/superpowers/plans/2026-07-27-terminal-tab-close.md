# Plan — workspace-session-terminal-tab-close (신규 `suaegi-tabclose` 크레이트, 의존 0, 단일 PR)

조사: Explore 정찰(소스 276L + 오라클 231L 통독, `types.ts`에서 **읽는 필드만** 확인).
출처 `reference/orca/` = **v1.4.146-rc.0**. import 전부 type-only → 런타임 의존 0.

**public 표면은 함수 하나 + 결과 타입 하나**지만 내부는 조밀하다:
탭의 **두 표상**(legacy row + unified tabs), 전 worktree에 걸친 PTY 집합 산술,
그룹 재구성, 재귀 레이아웃 prune, 6-way 서피스 포커스 도출.

## 0. 배치 — 신규 leaf, `[dependencies]` 빈다
`.trim()` 0회, 정규식 0개, 문자열 길이 연산 0개 → `suaegi-misc`조차 불필요.
`serde` 금지(입력은 이미 타입이 있는 상태이고 이 크레이트는 직렬화하지 않는다).
내부 모듈 분할: `lib.rs`(타입 + 리듀서), `layout.rs`(prune), `focus.rs`(pick_next_active_tab + derive_active_surface).

## 1. 계약 결정

- **N1 — ⚠⚠ 포커스 승계는 **MRU 우선**이지 이웃이 아니다**(`O:16-28`).
  ① `recentTabIds`를 **꼬리에서 앞으로** 훑어 생존자 중 처음 걸리는 것,
  ② 없으면 `tabOrder`에서 **첫 번째 닫히는 위치보다 오른쪽**의 첫 생존자,
  ③ 없으면 **마지막 생존자**(= 닫힌 게 맨 끝이었을 때의 왼쪽 이웃), ④ 없으면 `null`.
  "이전 형제"나 "다음 형제"로 단순화하면 틀린다. `findIndex`가 `-1`이면 ②는 **첫 생존자**로 퇴화한다.
- **N2 — prune의 붕괴는 **살아남은 자식이 split을 통째로 대체**한다**(`O:42-47`).
  `direction`과 `ratio`가 **조용히 버려진다**. 재래핑하지 않는다.
  자식이 둘 다 죽으면 split 자신이 사라지고 부모의 붕괴 규칙이 이어받으며,
  루트에서 사라지면 `tabGroupLayouts[worktreeId]` **키를 지운다**(`O:229`).
  ⚠ 순서는 **정확히 보존**된다(`first`는 `first`로) — 재정렬·재균형·정렬 없음.
  ⚠ 트리는 **원본 session**에서 읽는다(`O:208`), `next`가 아니다.
- **N3 — PTY 뺄셈은 **모든 worktree**를 훑는다**(`O:171-180`), 대상 worktree만이 아니다.
  `Object.values(session.tabsByWorktree)` 전체를 돌며 `tab.id !== tabId`인 탭의 PTY를 합집합에 넣고,
  닫는 탭의 PTY 중 거기 없는 것만 죽인다. worktree 하나만 보면 **다른 worktree가 쓰는 PTY를 죽인다**.
- **N4 — unified 탭 매칭은 **OR**다**(`O:75-77`): `tab.entityId === tabId || tab.id === tabId`.
  한 `tabId`에 **여러 unified 탭이 걸릴 수 있다**.
- **N5 — `closedVisibleIds`는 **raw `tabId`를 항상 포함**한다**(`O:182-183`) —
  그 id를 가진 unified 탭이 하나도 없어도.
- **N6 — 조기 반환 두 개**(`O:163-168`):
  ① row도 없고 unified도 0개 → `{closed:false, pinned:false}` + **동일 session 참조**;
  ② `terminalRow?.isPinned || unified.some(isPinned)` → `{closed:false, pinned:true}`.
  ⚠ `||`이지 `??`가 아니다 → `isPinned: false`는 `.some(...)`으로 **떨어진다**.
- **N7 — 파일 폴백만 **`filePath`로 키잉**한다**(`O:109`), `id`가 아니다. 터미널·브라우저는 `id`.
- **N8 — `activeUnified`는 **id와 groupId 둘 다** 일치해야 한다**(`O:93-95`).
  그리고 `activeGroup`은 `find(id) ?? groups[0] ?? null`로 **첫 그룹으로 폴백**한다(`O:92`).
- **N9 — 서피스 도출은 6-way**(`O:113-136`)이고, 세 종류의 폴백이 **서로 독립**이다
  (각각 "이전 선택이 살아있으면 유지, 아니면 index 0, 아니면 null").
  세 번째 분기에서 `contentType === 'simulator'`면 `'simulator'`, 그 외 전부 `'editor'`
  (`'diff'` 등도 **editor로 접힌다**).
- **N10 — 정렬이 **한 곳도 없다**. 순서는 전부 보존된다.**
  `ptyIdsToKill`은 **삽입 순서**가 관측 가능하다(closing PTY 집합의 순서:
  row ptyId → `ptyIdsByLeafId`의 `Object.values` 순서 → remote session id).
  ⚠ 오라클은 `.sort()`로 비교하므로(`T:111`) **순서를 고정하지 못한다** → 핀 직접 작성.
- **N11 — `tabOrder`가 빈 그룹은 통째로 버려진다**(`O:202`). 그룹이 전부 사라지면
  `activeGroupIdByWorktree[worktreeId]`와 `tabGroupLayouts[worktreeId]` **키를 지우고**
  `tabGroups[worktreeId] = []`는 **여전히 쓴다**(`O:218`).
- **N12 — 입력을 변경하지 않는다.** 새 최상위 객체를 만들고 맵들은 얕은 복제,
  개별 `Tab`/`TerminalTab`/스냅샷 객체는 **공유**한다.
  ⚠ 오라클에 `toBe` 참조 단언이 **없다**(`toEqual` 뿐) → Rust는 `&State` 받아 소유 `State` 반환.
- **N13 — `collectTabPtyIds`의 세 소스는 **truthy 검사**다**(`O:57-66`):
  `rowPtyId`는 non-null **이면서 non-empty**, remote session id도 마찬가지.
  빈 문자열 PTY id는 **집합에 안 들어간다**.

## 2. 오라클 & 핀
**오라클 5케이스 전량**(`T:88-230`).

**추가 핀(오라클 침묵):** N1 세 폴백 각각 + `findIndex == -1` 퇴화 + MRU가 꼬리부터임;
N2 3단 중첩 트리·단일 자식 붕괴에서 `direction`/`ratio` 소실·양쪽 사망·루트 소멸(키 삭제)·순서 보존;
N3 **다른 worktree가 쓰는 PTY는 안 죽는다**; N4 `entityId`/`id` 각각 매칭 + 복수 매칭;
N5 unified 탭이 없어도 raw id가 닫힘 집합에 듦; N6 `isPinned: false`가 `.some`으로 떨어짐 + not-found 반환값;
N7 파일 폴백이 `filePath` 키잉(`id`로 하면 실패하는 케이스); N8 groupId 불일치 시 `activeUnified`가 null;
N9 6분기 전부 + `'diff'` → `editor` 접힘 + `'simulator'`; **N10 `ptyIdsToKill` 순서 정확 단언**;
N11 마지막 그룹 소멸 시 두 키 삭제 + `tabGroups[wt] == []` 잔존; N13 빈 문자열 PTY id 제외.

*mutation:* N1 MRU 제거·앞에서부터 훑기·②③ 순서 교환, N2 붕괴 시 재래핑·`ratio` 보존·자식 순서 교환,
N3 대상 worktree만 스캔, N4 `&&`로·`entityId`만, N5 raw id 미포함, N6 `??`로·pinned 반환 뒤바꾸기,
N7 `id` 키잉, N8 groupId 검사 제거·`groups[0]` 폴백 제거, N9 분기 순서·`simulator` 접힘,
N10 정렬 추가, N11 키 삭제 생략·`tabGroups` 키까지 삭제, N13 truthy를 `is_some`으로.

## 3. 순서
단일 PR. export가 리듀서 하나뿐이라 부분 출하는 호출 가능한 표면이 없는 크레이트를 남긴다 → seam 없음.
내부는 `layout.rs`/`focus.rs`로 나눠 각자 단위 테스트를 두고, 오라클 5케이스는 리듀서 레벨에서 검증한다.
불변식: 의존 0(§0), **MRU 우선 포커스**(N1), 붕괴가 split을 대체(N2), **전 worktree PTY 뺄셈**(N3),
OR 매칭(N4), raw id 포함(N5), `||` 조기 반환(N6), `filePath` 키잉(N7), id+groupId 동시 일치(N8),
6분기 + editor 접힘(N9), **순서 보존 + `ptyIdsToKill` 순서 핀**(N10), 빈 그룹 제거와 키 삭제(N11),
불변 입력(N12), truthy PTY 수집(N13), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
