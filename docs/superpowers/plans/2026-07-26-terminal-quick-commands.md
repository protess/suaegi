# Plan — terminal-quick-commands (신규 `suaegi-quickcmd` 크레이트, 단일 PR)

조사: Explore 정찰(소스 272L + 오라클 388L 통독, `tui-agent-config`에서 **쓰이는 한 줄만** 확인).
**오라클 픽스처가 전부 ASCII다** → 여섯 개 캡에 대해 아무것도 증명하지 못한다(J1).

## 0. 배치 — 신규 leaf, 의존 1개
```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # js_trim (+ 로컬 js_trim_end)
```
`suaegi-misc` 헌장("작은 순수 헬퍼, 정책 없음")에 안 맞는다 — 이건 **정규화·검증 정책 + 6개 캡 + 정확-형태 프로토콜 검사**다.
**`serde`/`serde_json` 금지**(J11 참조 — `deny_unknown_fields`로는 표현 불가), `regex` 불필요.

**`tui-agent-config` 표면은 한 줄뿐이다**(`O:74`
`isTuiAgent(agent) && TUI_AGENT_CONFIG[agent].promptInjectionMode !== 'stdin-after-start'`)
→ **로컬로 들고 온다**(`suaegi-claude-roster` 선례). 정찰이 확인한 값 집합과
`stdin-after-start`인 에이전트 목록을 그대로 표로 옮기고 **미러링임을 주석에 명시**한다.

## 1. 계약 결정

- **J1 — ⚠ 캡 여섯 개가 전부 **UTF-16 code unit**이고 **오라클이 전부 ASCII라 증명력이 0**이다.**
  `O:42`(repoId 200), `O:119`(idBase 80), `O:122`(idBase 76), `O:129`(label 80),
  `O:144`(prompt 6000), `O:152`(command 4000). 바이트·`chars().count()`·UTF-16 세 구현이
  **오라클에서 구별 불가**하다. 여섯 곳 **각각** astral 경계 핀 필수.
- **J2 — ⚠ 서로게이트 straddle은 JS에서 **lone surrogate**를 낳고 Rust `&str`은 표현 불가 → **snap-down**.**
  선례 헬퍼 `utf16_slice_prefix`(`suaegi-forge/src/repo_icon.rs:190`,
  `suaegi-misc/src/codex_auth_errors.rs:79`) — **헌장상 모듈마다 복사**한다.
  결과는 79/199/5999/3999 unit.
  ⚠ **이 모듈 고유의 2차 결과**: `parse_normalized_*`(`O:227`)가 클라이언트 페이로드를 호스트 재정규화 결과와
  **정확 동등 비교**한다 → JS 클라이언트가 80-unit-with-lone-surrogate로 정규화한 label을 보내면
  Rust 호스트는 79로 정규화 → 불일치 → **페이로드 전체 거부**(사용자 편집이 거절된다).
  이론이 아니라 실제 교차 런타임 실패다. **모듈 헤더에 명시.**
- **J3 — `O:122`의 `MAX_QUICK_COMMAND_ID_LENGTH - 4`.**
  JS `slice(0, 음수)`는 `max(0, len+end)`라 total이지만 Rust `usize` 뺄셈은 **패닉/랩**한다
  → `const _: () = assert!(MAX_QUICK_COMMAND_ID_LENGTH >= 4);`로 컴파일 타임에 잡는다.
  ⚠ 그리고 `- 4` 예산은 **suffix ≥ 100에서 틀리다** — 산출 id가 81+ unit이 되어 캡을 **넘는다**.
  **그 오버플로를 축자 이식한다. 고치지 말 것.**
- **J4 — `O:119`는 80, `O:122`는 76으로 **폭이 다르다**.** 하나로 "단순화"하면 길이 77~80인 idBase의
  충돌 키 공간과 산출 id가 바뀐다. ⚠ 오라클은 `'status'`(6자)라 **두 폭이 똑같이 no-op** → 픽스처가
  두 구현을 일치시킨다. 80자 id 충돌 핀 필수.
- **J5 — flatten의 `\r\n|` 정규식 교대는 **오라클이 못 잡는다**.**
  `O:268`이 줄마다 `.trim()`을 하므로 `'echo one\r'.trim() === 'echo one'` → 단순 `/\n/`이나
  CRLF를 두 조각으로 쪼개는 구현(빈 조각은 `O:269`가 제거)이 **동일 출력**을 낸다.
  → `'a\rb'`(단독 CR)처럼 교대가 실제로 갈리는 핀을 쓴다.
- **J6 — `.trim()`과 `.trimEnd()`가 **인접해 있고 의도적으로 다르다**.**
  label은 `O:116` **양끝** trim, prompt(`O:142`)와 command(`O:148`)는 **뒤만**.
  오라클이 prompt의 선행 공백 보존은 못박지만 **command의 선행 공백은 어떤 테스트도 안 본다**
  → 단일 `trim` 구현이 통과한다. `{command: '  git status  '}` → `'  git status'` 핀 필수.
- **J7 — trim이 slice **앞**이고 **뒤에 재-trim이 없다**.**
  `O:142`→`O:144`, `O:148`→`O:152`. 4000번째 unit이 공백이면 **그 공백이 남는다**.
  ⚠ `suaegi-gen-prompt/src/commit_message_generation.rs:120-124`는 **반대 순서**(`slice` 후 `trimEnd`)라
  그 모듈 감각으로 짜면 뒤바뀐다. 오라클의 `'y'.repeat(4001)`엔 공백이 없어 못 잡는다.
- **J8 — trim 7곳 전부 ECMAScript 의미론** → `suaegi_misc::js_trim` + 로컬 `js_trim_end`.
  `str::trim`은 U+FEFF를 **안** 지우고 U+0085를 **지운다**(정반대). 오라클은 ASCII 공백뿐이라 못 잡는다.
- **J9 — `appendEnter: record.appendEnter !== false`는 **엄격**이다**(`O:153`).
  `0`·`'false'`·`null`·부재 전부 **`true`**, 리터럴 `false`만 `false`.
- **J10 — 개수 검사 두 곳이 **연산자도 동작도 다르다**.**
  `O:157` `normalized.length >= 40` → `break` = **절단**(그리고 검사가 push **뒤**라 40번째는 살아남는다).
  `O:221` `input.length > 40` → `null` = **거부**(정확히 40은 통과). 그리고 `O:221`은
  **원본 원소 수**(null·malformed 포함)를 센다.
- **J11 — ⚠ `hasExactKeys`는 **정확-형태 검사**이고 `serde`로는 표현 불가.**
  `O:165-168` `Object.keys(record).length === keys.length && keys.every(hasOwn)`.
  → 추가 키는 **치명적**(`{type:'global', repoId:'x'}` 거부), 그리고 **값이 `undefined`인 키도 개수에 든다**
  (`{...canonical, scope: undefined}`는 개수 검사를 통과한 뒤 `O:174`에서 실패).
  `deny_unknown_fields`는 후자를 못 잡는다 → **입력은 타입 구조체가 아니라 키 집합을 가진 무타입 레코드로 모델링**한다.
- **J12 — 생성 id의 카운터는 `normalized.length + 1`**(`O:118`) — 입력 인덱스가 아니라 **지금까지 방출된 개수**다.
  push마다 단조 증가하므로 이전에 생성된 id와 충돌하지 않는다.
- **J13 — `rawId`에 `||`를 쓴다**(`O:118`) → trim 후 **빈 문자열도** 생성 id로 폴백한다.
  비문자열 `id`는 `O:98`에서 `''`가 되므로 같은 경로.
- **J14 — `O:133-136`의 `agent === null` 분기는 **도달 불가**다**(`O:113-115`가 이미 반환).
  축자 유지하고 **주석으로 도달 불가 명시**(`suaegi-project-runtime` E9 선례).
- **J15 — 기본 목록은 **비어 있다****(`O:20`). 폐기된 프리셋의 두 id는 `REMOVED_PRESET_IDS`(`O:18`)로만 살아남아
  **trim된 id 기준으로** 드롭된다(`' default-pwd '`도 드롭). 반환 배열은 **매 호출 새로 만든다**
  (오라클은 `toEqual([])`뿐이라 공유 참조 반환도 통과 — Rust `Vec::new()`는 자연히 안전).
- **J16 — flatten은 개행이 없으면 **같은 객체를 반환한다****(`O:261-262`, 오라클 `test:339-347`이
  **`toBe` 참조 동등성**으로 못박는다). Rust에선 `Cow::Borrowed` vs `Cow::Owned`로 재현하고,
  선택을 헤더에 문서화한다(`String` 반환은 이 핀을 무의미하게 만든다).
- **J17 — 드롭 규칙**: 세 문자열(`label`/`command`/`prompt`) 중 **하나라도 있으면 살린다**(`O:109-111`,
  주석이 "편집 중 저장" 근거를 밝힌다). `action === 'agent-prompt'`인데 agent가 `null`이면 **드롭**(`O:113-115`).
  `record.action`은 `'agent-prompt'`가 아니면 전부 `'terminal-command'`로 기본값(`O:103-104`).

## 2. 오라클 & 핀
**오라클 18케이스 전량**(`test:17-387`).

**추가 핀(오라클 침묵):** **J1/J2 여섯 캡 각각 astral straddle → snap-down**(79/199/5999/3999)과
경계 −1/정확/+1; J3 suffix 100에서 id가 81 unit이 되는 **오버플로 축자 재현**;
J4 80자 idBase 충돌(76 vs 80 폭이 갈리는 케이스); J5 `'a\rb'` 단독 CR;
J6 `{command:'  git status  '}` → `'  git status'`; J7 4000번째가 공백일 때 **공백 잔존**;
J8 U+FEFF/U+0085 양방향(label·prompt·command·repoId 각각); J9 `appendEnter`의 `0`/`'false'`/`null`/부재;
J10 `O:157`의 정확히 40(41번째 드롭)·`O:221`의 정확히 40 통과 vs 41 거부·원본이 malformed를 포함해 41개;
J11 추가 키 거부·**값이 `undefined`인 키**가 개수에 듦; J12 malformed 선행 원소가 있을 때의 생성 번호;
J13 공백뿐인 `id`·비문자열 `id`; J15 `' default-pwd '`(공백 패딩) 드롭; J16 개행 없을 때 `Cow::Borrowed`;
J17 세 문자열 조합 8가지·`action`이 미지 값일 때 terminal-command 폴백.

*mutation:* J1 `len()`/`chars().count()`, J2 snap-down 제거(패닉 또는 절단 위치 변경), J3 `- 4` 제거·오버플로 "수정",
J4 두 폭 통일, J5 `\r\n` 교대 제거, J6 `trim_end`→`trim`(command), J7 slice 후 재-trim 추가,
J8 `str::trim`, J9 truthiness로, J10 `>=`↔`>` 교환·절단↔거부 교환·검사를 push 앞으로,
J11 추가 키 허용·`undefined` 키 무시, J12 입력 인덱스 사용, J13 `||`→`??`,
J15 미trim id로 비교, J16 항상 `Owned`, J17 드롭 조건 각각.

## 3. 순서
단일 PR. 정규화기 하나에 캡·트림·드롭·id-uniquing이 전부 엮여 있고 오라클 18개 중 다수가 그 하나를 탄다 → seam 없음.
불변식: 로컬 미러 표면·의존 1개(§0), **UTF-16 캡 6개 + snap-down**(J1/J2), 캡 산술 축자(J3/J4),
CR 교대(J5), trim 종류 구분(J6) 및 **순서**(J7), ECMAScript trim(J8), 엄격 `!== false`(J9),
두 개수 검사 분리(J10), **무타입 레코드 + 정확-형태**(J11), 방출 카운터(J12), `||` 폴백(J13),
도달 불가 분기 보존(J14), 빈 기본값·trim된 드롭 id(J15), `Cow` 참조 동등성(J16), 드롭 규칙(J17),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-impl-model-sonnet]]
