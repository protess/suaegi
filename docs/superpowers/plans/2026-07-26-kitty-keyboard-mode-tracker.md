# Plan — terminal-kitty-keyboard-mode-tracker (`suaegi-term` 신규 모듈, 단일 PR)

조사: Explore 정찰(소스 198L + 오라클 150L 통독, **PTY 바이트→문자열 디코드 경로를 종단간 추적**,
형제 포팅 모듈 4개 대조) + 리드 독립 검증.

**이 PR은 2026-07-25 terminal-detectors 플랜의 D2 defer를 닫는다.**

## 0. D2 해소 — `\x9b`는 `C2 9B`이지 `9B`가 **아니다**

미결이던 질문: "JS의 `'\x9b'`가 바이트 스트림에서 무엇이냐."

**정찰 증거 + 리드 실측으로 확정:**
- Orca의 모든 `ptySpawn(...)` 호출이 **`encoding`을 넘기지 않는다**(`local-pty-utils.ts:202,249,290`,
  `relay/pty-handler.ts:881`) → node-pty의 **`encoding: 'utf8'` 기본값**이 적용된다.
  (`null`을 넘겨야 Buffer가 나오는데 아무도 안 넘긴다.)
- PTY 경로 전체에 `latin1`/`'binary'`/`TextDecoder`가 **0회**다(리드가 grep으로 재확인 —
  유일한 히트는 무관한 git-diff `kind: 'binary'` 판별자).
- 시그니처가 전부 `string`이다: `onPtyData(ptyId, data: string, ...)`(`orca-runtime.ts:6512`),
  `scan(data: string)`(`O:59`).

**따라서:**
| 스트림 바이트 | node-pty utf8 디코드 결과 | JS `\x9b`와 매치? | Rust `&[u8]` 등가물 |
|---|---|---|---|
| `1B 5B` | `ESC [` | 예 | `b"\x1b["` |
| `C2 9B` | U+009B | **예** | **`b"\xc2\x9b"`** |
| 단독 `9B` | **U+FFFD**(불완전 UTF-8) | **아니오** | **매치하면 안 됨** |

⚠ **단독 `0x9B` 바이트를 매치하면 발산이 아니라 회귀다.** 0x9B는 UTF-8 continuation 범위라
평범한 다바이트 문자 안에 나온다 — `⚛` U+269B = `E2 9A 9B`, `♛` U+265B = `E2 99 9B`.
바이트 단위 `lastIndexOf(0x9B)`는 **문자 중간을 잘라내고**, 바이트 정규식은 `⚛>1u`를
`CSI > 1 u`로 **오인**한다.

**H1 — 채택: (a) Orca 충실.** `(?:\x1b\[|\xc2\x9b)`, tail 앵커는 `0x1b` 또는 2바이트 `C2 9B`.
실제 8비트 C1 CSI는 무시된다 — **Orca도 무시하므로 그게 맞다**.
(b) "터미널 정확"(단독 `9B`도 C1로 인정)은 오라클과 어긋나고 위 오탐을 부른다 →
**의도적 non-goal로 모듈 헤더에 기록**한다(`reply_query_scan.rs:9-13` 스타일).

⚠⚠ **H2 — 오라클 테스트를 그대로 옮겨 적으면 안 된다. 재인코딩해야 한다.**
`T:79,81`의 `'\x9b<99u'`는 JS에서 **5자**지만 바이트로는 **6바이트** `b"\xc2\x9b<99u"`다.
`b"\x9b<99u"`라고 쓰면 **다른 테스트를 인코딩해 틀린 구현을 검증**하게 된다.

## 1. 배치 — `suaegi-term`의 최상위 모듈, 새 의존 0
`crates/suaegi-term/src/kitty_keyboard_mode_tracker.rs`. `lib.rs`에 `pub mod`만 추가
(`bell_detector`/`partial_escape_tail`는 crate root에 re-export되지 **않는다** — 이웃을 따른다).
`regex`는 이미 있다 → `regex::bytes::RegexBuilder` + `.unicode(false)` + `LazyLock`
(`reply_query_scan.rs:19-30` 선례). 스택은 `VecDeque`(std).

## 2. 계약 결정

- **H3 — `partial_escape_tail`를 **재사용 금지**. 다른 알고리즘이다.**
  이건 **문자열 접미사 휴리스틱**(마지막 introducer 인덱스 + body 문자클래스)이고,
  그건 **완전한 VT500 상태기**(8상태, OSC/DCS/CAN/SUB 전부 모델링)다.
  구체적 발산: `"\x1b]0;title"`(미완 OSC)에서 `partial_escape_tail`는 **전체를 반환**하고
  `extractScanTail`은 **`''`를 반환**한다(body가 `null`). 겹치는 곳에서 우연히 일치할 뿐이다.
- **H4 — `Number('')`는 `0`이지 `NaN`이 **아니다**.** `O:118`, `O:139` 둘 다 여기 의존한다.
  도달 가능: `CSI = 3 ; u` → `[3, 0]`, `CSI > u` → `[0]`, `CSI ? ; h` → `[0, 0]`.
  ⚠ Rust `"".parse()`는 **Err**다 → **빈 문자열을 명시적으로 0으로** 매핑해야 한다.
  (형제 `terminal-private-mode-tracker.ts:30-32`는 빈 값을 **skip**한다 — 이 모듈은 **안 한다**.
  두 동작은 교환 불가.)
- **H5 — `|=`/`&= ~`는 JS **32비트 부호 있는** 연산(ToInt32)인데 `=`는 **생 f64**를 대입한다.**
  `O:166`/`O:168` vs `O:148`/`O:161`/`O:164`. 즉 `currentFlags`가 set 후 `3e9`였다가
  or 후 **음수**가 될 수 있다. `u32`를 고르면 ≥2³¹에서, `i32`로 통일하면 `=`-set에서 조용히 발산한다.
  → **`i64` 저장 + 비트 연산 직전에 명시적 `to_int32` 헬퍼**. 실제 kitty 플래그는 `0..=31`이고
  유일한 소비자가 `> 0`만 본다(`keyboard-handlers.ts:386`) → **관측 불가능하지만 명시적으로 문서화 + 핀**.
- **H6 — `stack.shift()`는 **앞**에서 제거한다**(`O:144`). `Vec::pop()`은 **틀린 번역** →
  `VecDeque::pop_front()`. 오라클 `T:85-91`이 값을 순환시켜서 잡아준다(상수 값이었으면 못 잡았다).
- **H7 — pop-to-empty는 **원래 비어 있었어도** 플래그를 0으로 만든다**(`O:157-158`).
  `if popped_any && stack.is_empty()` 가드를 넣으면 틀린다. `T:29-31`·`T:50`이 못박는다.
- **H8 — 화면 전환에 **이미-활성 가드가 없고**(`O:126-134`, 주석 `:123-125`가 의도라고 명시),
  **모든 파라미터를 순회**한다**(`O:117`).
  → `\x1b[?1049h`를 두 번 보내면 **두 번 스왑**하고, `\x1b[?1049;47h`는 **한 시퀀스 안에서 두 번 스왑**한다.
  **둘 다 오라클 커버리지 0.** 첫 매치에서 `break` 금지, 가드 추가 금지.
- **H9 — `extractScanTail`이 정규식 루프 **앞에** 돌고, 정규식은 **보존된 tail을 포함한 `input` 전체**를 스캔한다**
  (`O:79-80`). 이중 계산이 안 나는 유일한 근거는 `isIncompleteSequenceBody`(`O:196`)가
  **종결 문자(`u`/`h`/`l`/`p`)를 포함한 body를 절대 받지 않는다**는 것이다.
  그 술어를 "개선"하면 push가 **두 번 적용된다**.
- **H10 — 리셋이 **세 가지, 서로 비대칭**이다.**
  ① `reset()`(`O:48-57`): 8필드 **전부** 초기화(`scanTail` 포함).
  ② **RIS `\x1bc`**(`O:85-91`): `scanTail`을 **지역 변수에 빼두고** `reset()` 후 **복원**하며,
     그 다음 `alternateScreenSwitchObserved = true`로 **강제**한다.
     ⚠ Rust에서 `*self = Self::default()`로 지름길 내면 **tail이 날아간다**. 오라클 커버리지 0.
  ③ **DECSTR `!p`**(`O:105-114`): `currentFlags`/`mainFlags`/`altFlags` 0 + 두 스택 비움.
     `alternateScreenActive`·`alternateScreenSwitchObserved`·`scanTail`은 **건드리지 않는다**.
- **H11 — `mode`는 truthiness이고 4 이상은 **조용한 no-op**이다**(`O:162-169`).
  `parsed.length > 1 && parsed[1] ? parsed[1] : 1` → `CSI = 5 ; 0 u`는 **mode 1(set)**로 붕괴한다
  (`??`였다면 0이 유지된다). `else`가 없어 mode 4/5/…는 아무 일도 안 한다. 둘 다 커버리지 0.
- **H12 — `replay` 플래그는 **`>` push만** 게이트한다**(`O:142`). 화면 전환·DECSTR·pop은 동일하게 적용된다.
  그리고 `currentFlags = parsed[0] || 0`는 **replay 여부와 무관하게** 실행된다(`O:148`).
  ⚠ `orca-runtime.ts:8351-8359`가 **`?1049h/l` 분류 목적으로** `scanReplay(snapshot.data)`를 부른다 —
  **프로덕션 핵심 경로인데 유닛 커버리지 0**.
- **H13 — 4096 캡은 `>`(초과)이고 body 유효성 검사 **앞에** 있다**(`O:178`). 커버리지 0.
- **H14 — `parsed[0] || 1`(`O:152`)이라 `CSI < 0 u`는 **1회 pop**한다**(0회가 아니다).
  `Math.max(1, ...)`는 charset에 `-`가 없어 도달 불가지만 축자 유지.

## 3. 오라클 & 핀

**오라클 13케이스 전량**(`T:5-149`), **단 `T:79,81`의 두 `\x9b` 케이스는 `b"\xc2\x9b…"`로 재인코딩**(H2).

**추가 핀(오라클 침묵 — 17개 무커버 분기 중 최소 다음):**
**H1 오탐 방지: `"⚛>1u"`가 플래그를 바꾸지 않는다**(전체 결정의 mutation-catcher);
`\x9b`(=`C2 9B`) + `!p` / + `?1049h` / 단독 tail 보존; H13 4096 캡; H9 `body === null`
(`"\x1b]0;title"`, `"\x1bO"`); H4 빈 파라미터 3종(`\x1b[>u`, `\x1b[=u`, `\x1b[?;h`);
H11 mode 0 붕괴·mode 5 no-op; DECSET `47`·`1047`(오라클은 `1049`만); H8 다중 파라미터 이중 스왑·
`?1049h` 연속 이중 스왑; H10② RIS의 tail 보존(`scan("\x1bc\x1b[>")` 후 `scan("1u")`)·
RIS가 observed를 true로; H10③ DECSTR이 `isAlternateScreen`을 안 바꿈; H12 **`scanReplay` + `?1049h`**;
H5 32비트 랩(`=` 후 `|=`); H6 스택 캡에서 **앞**이 밀림; `?1049h`가 청크 경계로 쪼개진 경우.

*mutation:* H1 `b"\x9b"` 매치·`\xc2\x9b` 제거, H3 `partial_escape_tail` 재사용, H4 빈 문자열→Err/skip,
H5 `u32`/`i32` 통일, H6 `pop_back`, H7 already-empty 가드 추가, H8 `break` 추가·가드 추가,
H9 body 술어에 종결문자 허용, H10 `*self = default()`·observed 미설정·DECSTR이 화면 전환,
H11 `??` 의미론·mode 4를 set으로, H12 replay가 pop/화면전환도 게이트, H13 `>=`, H14 `|| 1` 제거.

## 4. 순서
단일 PR. 상태기가 미묘하지만 **원자적**이다 — 오라클 `T:109-115`가 스택·화면전환·리셋을
한 테스트에서 엮으므로 쪼갤 수 없다. 소비자 배선은 형제 클러스터와 마찬가지로 **범위 밖**(사람눈).
**후속 별건:** 같은 `extractScanTail` 휴리스틱을 쓰는 형제 3개
(`terminal-private-mode-tracker`, `terminal-mouse-mode-mirror`, `terminal-color-scheme-protocol`)는
**별도 마일스톤**. 그때 공용 헬퍼를 검토한다(오늘은 kitty 쪽이 진부분집합).
불변식: **`C2 9B`(H1) + 오라클 재인코딩(H2)**, 별도 tail 알고리즘(H3), 빈→0(H4), 32비트 비트연산(H5),
`pop_front`(H6), 무조건 zeroing(H7), 가드 없음·전 파라미터(H8), tail 선행 + 종결문자 배제(H9),
세 리셋 비대칭(H10), mode truthiness(H11), replay는 push만(H12), `>` 캡(H13), `|| 1`(H14),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-impl-model-sonnet]]
