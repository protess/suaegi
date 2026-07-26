# Plan — claude-subagent-roster (신규 `suaegi-claude-roster` 크레이트, 단일 PR)

조사: Explore 정찰(소스 295L + 오라클 376L 통독, 상류 호출자 `agent-hook-listener.ts` 실측,
`agent-status-types.ts`에서 **가져오는 두 항목만** 확인).

## 0. 배치 — 신규 leaf, 의존 1개

```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # js_trim 전용 (O:120의 유일한 .trim())
```
- `suaegi-misc` 헌장("작은 자족 순수 헬퍼, 정책 없음")에 맞지 않는다 — 이건 **가변 키 저장소 위의 조정 정책**이다.
  `suaegi-project-runtime`·`suaegi-workspace-cleanup`이 같은 이유로 분리했다.
- `suaegi-app`에 묻으면 iced/tokio 뒤에 갇혀 **격리된 mutation 테스트가 불가능**하다.
- **`serde_json` 불필요** — 관측 표면이 "배열이냐 / 객체스러우냐 / `type`이 두 리터럴 중 하나냐 /
  `id`가 문자열이고 트림 후 비었냐 / `agent_type`·`description`이 문자열이냐 / `status`가 정확히
  `'running'`이냐"뿐이다. 키 순서·중복 키·수치 포맷·중첩 깊이를 **하나도 안 본다** → 손코딩 입력 enum
  (`suaegi-project-runtime` E1 선례). 파싱은 이미 상류에서 끝났다.
- **`regex` 불필요**(I6), **`chrono` 불필요**(시계를 안 읽는다), **`indexmap` 불필요**(I13은 `Vec`로).

**`agent-status-types`의 두 항목은 로컬로 들고 온다.** 그 파일(383L)의 본체는 별도 스코프의
wire 정규화 계층이고 이미 포팅된 `suaegi-app/src/agent_status/parse.rs`와 겹쳐 **병합 결정**이지 포팅 결정이 아니다.
- `AGENT_STATUS_MAX_SUBAGENTS = 32` → 로컬 상수, **미러링임을 주석에 명시**.
  (모듈이 이미 이 선례를 만든다: `O:6`의 `CLAUDE_SUBAGENT_ID_MAX_LENGTH = 64`가 그쪽 private 상수의 손복사다.)
- `AgentSubagentSnapshot` + `AgentSubagentState`(**4변형** `working|blocked|waiting|idle`) → 로컬 정의.
  로스터는 `working`만 방출하지만 시딩 호출자가 `state !== 'working'`로 거른다 → **나머지 3변형도 load-bearing**.

## 1. 계약 결정

- **I1 — `??`는 `''`를 **덮어쓴다**.** `O:70`,`:71`,`:186`,`:187`. `undefined`/`null`일 때만 기존 값을 유지한다.
  `unwrap_or_default`나 "비어있지 않을 때만 갱신"으로 쓰면 발산한다.
  ⚠ **모든 픽스처가 non-empty 아니면 `undefined`**라서 오라클이 두 구현을 구별 못 한다 → 핀 필수.
- **I2 — ⚠ `options?.inventoryComplete !== false`에서 **부재는 "완전"**이다**(`O:166`, `:211`).
  옵션 객체 부재·필드 부재·`true`·`null`·`0` 전부 **완전** → sweep 수행. **명시적 `false`만** 억제한다.
  기본값 `false`인 Rust `bool`로 쓰면 **sweep 전체가 뒤집힌다**.
  오라클은 빈-목록 분기(케이스 18/19)만 잡고 **sweep 분기는 항상 기본값으로만** 도달하므로,
  호출자에게 명시를 강요하는 시그니처는 테스트를 초록으로 둔 채 실사용을 드리프트시킨다.
- **I3 — ⚠ id 길이 가드(`O:65`)는 **어떤 픽스처도 발동시키지 않는다**.** 빈 id도 65자 id도 없다 →
  **가드를 통째로 빼도 26/26 통과**한다. 그리고 `.length`는 **UTF-16 code unit**이다
  (바이트도 chars도 아니다) → `encode_utf16().count()`. 양쪽(`== 0`, `> 64`) 다 핀.
- **I4/I5 — `O:120`의 `.trim()`은 **길이 검사 전용**이고, 저장되는 id는 **트림되지 않는다****(`O:130`).
  `' a1 '`는 **공백째 저장**되어 로스터 키 `'a1'`과 **매치되지 않는다**.
  트림은 `suaegi_misc::js_trim`(U+FEFF 포함/U+0085 제외 — Rust `str::trim`과 정반대).
  ⚠ 공백 padding id 픽스처가 **하나도 없다** → 두 항목 다 핀 필수.
- **I6 — `/^[0-9a-f]+$/i`는 `/u`가 없어 **ASCII 전용**이다**(`O:56`) → `is_ascii_hexdigit`.
  Rust `regex`의 `(?i)`는 유니코드 폴딩이라 클래스가 넓어진다. `regex` 크레이트를 쓰지 말 것.
- **I7 — `id.startsWith('a')`는 **대소문자 구분**이다**(`O:56`). `'Aprobe1-6d3c'`는 **false**.
  모든 픽스처가 소문자 `a`/`t`로 시작 → `eq_ignore_ascii_case` 포트가 **보이지 않는다**. 핀 필수.
- **I8 — `separator > 1`은 **엄격**이다**(`O:56`). `'a-ff'`(하이픈이 인덱스 1)는 **false**.
  `>= 1`/`> 0`으로 써도 픽스처엔 인덱스 1 하이픈이 **없어서** 통과한다. 핀 필수.
  (Rust `rfind`는 **바이트** 인덱스라 단위가 다르지만, UTF-16에서 1이면서 바이트로 >1이려면
  첫 글자가 비-ASCII여야 하고 그러면 `startsWith('a')`가 먼저 실패한다 → 발산 불가. ASCII 스캔으로 쓰고 주석.)
- **I9/I10 — 정렬 비교자는 **명시적**이다**(`O:293`):
  `a.startedAt - b.startedAt || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0)`.
  → `started_at.cmp().then_with(|| <id 비교>)`. **뺄셈 금지**(i64 오버플로 + JS의 `NaN`→타이브레이크 낙하 상실).
  ⚠ JS `<`/`>`는 **UTF-16 code unit 순서**, Rust `str::cmp`는 **UTF-8 바이트(=코드포인트) 순서** —
  astral 문자 vs U+E000..U+FFFF에서 다르다. 픽스처가 전부 ASCII라 일치 → **문서화 + 핀**
  (또는 `encode_utf16()` 비교기; 어느 쪽이든 명시적 결정으로).
  기본 문자열 정렬로 포팅하면 케이스 26이 **잡아준다**(`['a','z','b']` vs `['a','b','z']`).
- **I11 — 캡 3곳 전부 `>=`이고 전부 "최신 것을 버리고 축출하지 않는다"**(`O:80`, `:123`, `:228`).
  ⚠ `O:80`의 **갱신 경로가 캡 검사보다 먼저**라(`O:69-77`) 기존 행 갱신은 크기와 무관하게 성공한다.
  `O:123`은 **필터 후** 개수를 센다 → 유효 32개 뒤에 쓰레기가 와도 `truncated: false`.
  `O:228`은 `O:80`과 **중복이라 관측 불가**하지만 **축자 유지**한다(I14).
- **I12 — 두 플래그를 **plain `bool`**로 모델링한다.** `O:218`은 truthiness 부정,
  `O:219`는 엄격 `!== true`인데, 값이 `undefined`/`true`만 나오므로 `Option<bool>` + `!= Some(true)`로 하면
  **모든 픽스처에서 동일**해 구별 불가. `bool`로 하고 `O:75`의 `= undefined`를 `= false`로.
- **I13 — `pendingRunningTasks`는 **삽입 순서가 관측 가능**하다.**
  `O:228`의 `break` 때문에 마지막 빈 슬롯을 누가 가져가느냐가 순서에 달렸다.
  중복 id의 last-wins/삭제(`O:183`,`:192`)는 픽스처에선 **구조적으로 도달 불가**(중복 id가 없다).
  → `Vec<(String, Task)>` + last-wins upsert + 제거. `HashMap`은 실제 중복/캡 경계에서 발산한다.
  ⚠ 반면 **로스터 자체는 순서 무관**(`O:293`의 정렬이 총순서라 삽입 순서를 지운다) → `HashMap`/`BTreeMap` 무방.
  **이 근거를 크레이트 헤더에 적는다** — 초록 테스트는 이 선택의 증거가 아니다.
- **I14 — `O:228`의 중복 캡 검사를 **삭제하지 말 것**.** 오늘은 관측 불가지만
  `upsertWorkingClaudeSubagent`의 캡 의미가 바뀌는 순간 load-bearing이 된다.
- **I15 — `hasTeammateTypedTask`는 **필터 이전 전체 배열**에 대해 계산된다**(`O:173`).
  루프 안/뒤로 옮기면 상수 `false`가 된다. (이건 오라클이 **잡아준다** — 케이스 10/13/15/20/21.)
- **I16 — 조정은 **id로만** 한다**(`O:179`). 이름·agentType·description·인덱스로는 절대 매칭하지 않는다
  (오라클 케이스 25가 agentType 폴백 부재를 못박는다).
  sweep 생존은 **4조건 전부** 필요하고(`O:213-222`), 기본은 **제거**다.
- **I17 — 로스터는 **제자리 변경**되고, 외부에서도 직접 `set`된다**(`agent-hook-listener.ts:2400-2405`).
  → `backgroundTasksAuthoritative: true`로 시딩하는 **공개 생성/삽입 경로**가 필요하다(upsert 함수만으로는 부족).

## 2. 오라클 & 핀

**오라클 26케이스 전량**(`T:27-375`).

**추가 핀(오라클 침묵):** I1 `''`가 덮어쓰고 `None`이 보존함(upsert·fold 양쪽);
I2 sweep 분기에서 기본값=완전·명시 `false`만 억제; **I3 길이 가드 양쪽 경계(0자, 64자, 65자)와 UTF-16 단위**
(64개 astral 문자 id는 128 unit → 거부); I4/I5 U+FEFF 패딩 id 거부·U+0085 패딩은 **통과**·
`' a1 '`가 **공백째 저장되어 키 매칭 실패**; I6 `'ateam-6D3C'`(대문자 hex) **통과**·비-ASCII hex 거부;
I7 `'Aprobe1-ff'` **false**; I8 `'a-ff'` **false**·`'ateam-'`(빈 접미) false·`'bteam-ff'` false;
I9 동일 `startedAt`의 id 타이브레이크·비-ASCII id 순서 결정 문서화;
I11 `O:123`의 정확히 32 경계(`truncated: false`)·유효32+쓰레기; 갱신이 캡을 무시함;
I12 두 플래그 조합 4가지; I13 중복 id last-wins·캡 경계에서 pending 순서;
`readClaudeBackgroundAgentTasks` 미검증 입력: `null`/`{}`/`42`인 `background_tasks`·**배열 원소**·
비문자열 `id`·공백뿐인 id·비문자열 `agent_type`/`description`·`type:'subagent'`+비-running;
`claudeRosterHasWorkingSubagent(None)`·`claudeRosterToSnapshots(None)`;
방출 스냅샷의 `agentType`/`description`/`startedAt` 값과 **`model` 부재**;
`claudeTeammateIdMatchesName`의 빈 name(접두 `"a-"`)·빈 접미(`'aprobe1-'`).

*mutation:* I1 `unwrap_or_default`/빈 문자열 보존, I2 기본값 반전, I3 가드 제거·`len()`·`chars().count()`,
I4 `str::trim`, I5 저장 시 트림, I6 `(?i)` 유니코드, I7 대소문자 무시, I8 `>= 1`, I9 `str::cmp`·기본 정렬,
I10 뺄셈, I11 `>=`→`>`·갱신을 캡 뒤로, I12 `Option<bool>` + `!= Some(true)`, I13 `HashMap`,
I14 `O:228` 검사 제거, I15 `some()`을 루프 뒤로, I16 sweep 4조건 각각 제거·name 폴백 추가.

## 3. 순서
단일 PR. fold가 upsert와 `isClaudeTeammateLifecycleId`에 의존하고 26케이스 중 11개가 fold를 탄다 → seam 없음.
**별건 후속:** ① `suaegi-app/src/agent_status/` 배선(로스터 소유 슬라이스 `agent-hook-listener.ts:2328-2405`
포팅이 선행) ② `agent-status-types.ts` 포팅 + 미러 항목 통합.
불변식: 손코딩 입력 enum·의존 1개(§0), `??` 의미론(I1), **부재=완전**(I2), UTF-16 길이 가드(I3),
`js_trim` + 미트림 저장(I4/I5), ASCII hex(I6), 대소문자 구분(I7), 엄격 `> 1`(I8),
명시 비교자 + UTF-16 순서 결정(I9/I10), 캡 3곳 `>=`(I11), plain `bool`(I12), pending 순서 보존(I13),
중복 검사 유지(I14), 전체 배열 스캔(I15), id-only 조정(I16), 공개 시딩 경로(I17), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[js-lowercase-two-mechanisms]],
[[suaegi-impl-model-sonnet]]
