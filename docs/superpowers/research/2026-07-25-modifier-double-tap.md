# Research: `modifier-double-tap-detector` — Rust 포팅 계약서

**대상**: Orca `src/shared/modifier-double-tap-detector.ts` (153L) + `.test.ts` (136L) @ v1.4.150-rc.0
**성격**: **STATEFUL 이지만 DETERMINISTIC**. 시각은 `Date.now()` 가 아니라 `process(event, timestampMs)` 파라미터로 **주입**(:97) → 100% 테스트 가능. `.test.ts` 가 오라클, 순수 상태기계이므로 **verbatim 포팅** 대상.

## 결론 요약 (먼저 읽을 것)

1. **윈도우 상수 = `DOUBLE_TAP_WINDOW_MS = 300`** (:5). 비-export, 내부 전용, 사용자 설정 불가.
2. **임계 비교는 `<=` (inclusive)** — `timestampMs <= this.state.deadlineMs` (:125). 그리고 **deadline 은 keyDown 이 아니라 keyUp 시점에 미리 계산**된다: `deadlineMs = timestampMs + DOUBLE_TAP_WINDOW_MS` (:142). 즉 측정 간격은 **`(두번째 keyDown 시각) − (첫번째 keyUp 시각)`** 이지 first-down↔second-down 이 아니다. 주석(:3-4) "max gap between the **first release** and the **second press**" 와 정확히 일치. **Rust 포트는 keyUp 에서 deadline 을 precompute 하고 keyDown 에서 `ts <= deadline` 로 비교해야 한다. down 에서 delta 를 빼는 방식으로 바꾸면 앵커가 틀어진다.**
3. **상태기계는 3-phase tagged union** (:89-92): `idle` / `down1{modifier}` / `armed{modifier, deadlineMs}`. Rust 로는 `enum DetectorState { Idle, Down1(Mod), Armed{modifier, deadline_ms} }`.
4. **`PhysicalModifierToken` 은 type-only import** (`import type` :1) — 값 의존성 0. 정의는 `keybindings.ts:153-154` `type ModifierToken = 'Mod'|'Cmd'|'Ctrl'|'Alt'|'Shift'; type PhysicalModifierToken = Exclude<ModifierToken, 'Mod'>` = **`'Cmd'|'Ctrl'|'Alt'|'Shift'` 4개** (Mod 제외). Rust 는 4-variant enum 을 자체 재현. **'Mod' 은 이 검출기에 절대 안 들어온다** (:51 주석 "always a physical token, never 'Mod'").
5. **모듈은 사실상 self-contained** — 유일한 import 가 위 type-only 한 줄. 값/런타임 의존성 없음. `Date.now()`·`performance.now()` 등 미참조 (grep 0건). 완전 순수 + 주입된 시각.

---

## 0. 파일 구조 / import

- 모듈 `modifier-double-tap-detector.ts:1` — `import type { PhysicalModifierToken } from './keybindings'` **한 줄뿐**. `import`(비-type) 0건, `require` 0건. → Rust 포트는 `keybindings` 의 어떤 코드도 가져올 필요 없이 `PhysicalModifier` enum 만 새로 정의.
- 오라클 `.test.ts:1-7` — `vitest` + 대상에서 `ModifierDoubleTapDetector`, `modifierFromKeyEvent`, `toModifierDoubleTapEvent`, `type ModifierDoubleTapEvent` import. **`otherModifierHeld`(비-export 헬퍼)·`reset()`·타입 `DetectedDoubleTap`/`ModifierKeyEventLike`/`PhysicalModifierToken` 은 오라클이 직접 이름으로 호출하지 않음** (reset 은 test9 에서 호출됨 :106; otherModifierHeld 는 toModifierDoubleTapEvent 경유로만 커버).

---

## 1. Public surface

### 1.1 consts / types

- **`DOUBLE_TAP_WINDOW_MS = 300`** (:5) — 비-export const. 윈도우 폭.
- **`type ModifierDoubleTapEventType = 'keyDown' | 'keyUp'`** (:7) — export. Rust: 2-variant enum.
- **`type ModifierDoubleTapEvent`** (:10-17) — export. 필드: `type: ModifierDoubleTapEventType`; `modifier: PhysicalModifierToken | null` (비-모디파이어 키면 null, :12-13); `isModifierOnly: boolean` (다른 모디파이어 미보유의 bare 모디파이어일 때만 true, :14-15); `isAutoRepeat: boolean`.
- **`type DetectedDoubleTap = { modifier: PhysicalModifierToken }`** (:19) — export. 검출 결과.
- **`type ModifierKeyEventLike`** (:21-30) — export. 플랫폼 원시 이벤트: `type`; optional `code?`, `key?` (string); optional `shift?`/`control?`/`alt?`/`meta?` (boolean, **주의: `control` 이지 `ctrl` 아님**); optional `isAutoRepeat?`.
- **`type PhysicalModifierToken`** — import-only (§0.4). 값 재현: `Cmd|Ctrl|Alt|Shift`.
- **`DetectorState`** (:89-92) — **비-export** 내부 상태 union. `{phase:'idle'} | {phase:'down1', modifier} | {phase:'armed', modifier, deadlineMs}`.

### 1.2 함수 exports

- **`modifierFromKeyEvent(code?, key?): PhysicalModifierToken | null`** (:52-60) — pure. §2.
- **`toModifierDoubleTapEvent(event: ModifierKeyEventLike): ModifierDoubleTapEvent`** (:79-87) — pure. §2.
- **`otherModifierHeld(event, modifier): boolean`** (:62-76) — **비-export** 헬퍼. §2.

### 1.3 class `ModifierDoubleTapDetector` (:94-153)

- 내부 상태: `private state: DetectorState = { phase: 'idle' }` (:95). **초기값 idle.** Rust: `struct` with `state: DetectorState`, `Default = Idle`.
- 메서드: `process(event, timestampMs): DetectedDoubleTap | null` (:97-110, `&mut self`); `reset(): void` (:112-114); `private onModifierDown(modifier, isAutoRepeat, timestampMs)` (:116-138); `private onModifierUp(modifier, timestampMs)` (:140-152).
- 순수성: 부수효과는 오직 `this.state` 변이. I/O·전역 없음. Rust 매핑: 모든 메서드 `&mut self`, 반환 `Option<DetectedDoubleTap>`.

---

## 2. `modifierFromKeyEvent` / `toModifierDoubleTapEvent` — 키이벤트 매핑

### 2.1 매핑 테이블 (:32-48)

- **`MODIFIER_BY_CODE`** (:32-41): `ShiftLeft`→`Shift`, `ShiftRight`→`Shift`, `ControlLeft`→`Ctrl`, `ControlRight`→`Ctrl`, `AltLeft`→`Alt`, `AltRight`→`Alt`, `MetaLeft`→`Cmd`, `MetaRight`→`Cmd` (8개, L/R 각각). **`Meta*`→`Cmd`** 로 리네임됨에 주의.
- **`MODIFIER_BY_KEY`** (:43-48): `Shift`→`Shift`, `Control`→`Ctrl`, `Alt`→`Alt`, `Meta`→`Cmd` (4개). **key 는 `Control`/`Meta` (DOM `KeyboardEvent.key` 값), token 은 `Ctrl`/`Cmd`.**

### 2.2 `modifierFromKeyEvent(code, key)` (:52-60)

- :56 `if (code && MODIFIER_BY_CODE[code]) return MODIFIER_BY_CODE[code]` — **code 우선**. code 가 truthy(빈문자열 아님) 이고 맵에 있으면 그 값.
- :59 `return key ? (MODIFIER_BY_KEY[key] ?? null) : null` — 아니면 key 로 조회, 없으면 null; key 자체가 falsy 면 null.
- **정확 문자열 매치, case-fold 없음.** `'shiftleft'`·`'SHIFT'` 등은 매치 안 함. Rust: `HashMap` 또는 `match` 로 exact-arm.

> **T4 (JS-only 풋건, Rust 면역)**: `MODIFIER_BY_CODE[code]` 는 JS 오브젝트 인덱싱이라 `code === '__proto__'`·`'constructor'` 등 prototype 키가 truthy(함수/오브젝트) 를 반환 → :56 이 이를 `PhysicalModifierToken` 인 것처럼 반환하는 **잠재 타입버그**. 실제 키코드는 절대 이 문자열이 아니므로 미발현. **Rust `HashMap::get`/`match` 는 이 클래스가 원천적으로 불가능** — 포트는 그대로 안전. 정보로만 기록, 재현 불필요.

### 2.3 `otherModifierHeld(event, modifier)` (:62-76)

- 반환: **이 이벤트의 대상 modifier 를 제외한 다른 모디파이어가 하나라도 눌려있는가**.
- :63-64 `modifier !== 'Shift' && event.shift` → true; :66-67 `!== 'Ctrl' && event.control`; :69-70 `!== 'Alt' && event.alt`; :72-73 `!== 'Cmd' && event.meta`; :75 else false. (플래그명 `control`/`meta` 주의.)

### 2.4 `toModifierDoubleTapEvent(event)` (:79-87)

- :80 `modifier = modifierFromKeyEvent(event.code, event.key)`.
- :82 `type: event.type` (그대로).
- :84 `isModifierOnly: modifier !== null && !otherModifierHeld(event, modifier)` — **비-모디파이어(null)면 false; 다른 모디파이어 병행이면 false.**
- :85 `isAutoRepeat: Boolean(event.isAutoRepeat)` — undefined→false.

---

## 3. 상태기계 EXACTLY

### 3.1 `process(event, timestampMs)` — 게이트 (:97-110)

1. :101 `if (event.modifier === null || !event.isModifierOnly)` → :102 `state = idle`, :103 `return null`. **비-모디파이어 키 또는 코드된(다른 모디파이어 병행) 모디파이어는 제스처를 깬다.** (:98-100 주석: keyUp 에서 isModifierOnly:false 는 다른 모디파이어가 아직 눌림 상태를 뜻함.)
2. :105 `if (event.type === 'keyUp')` → :106 `onModifierUp(modifier, ts)`, :107 `return null`. **keyUp 은 절대 emit 하지 않는다.**
3. :109 else → `return onModifierDown(modifier, event.isAutoRepeat, ts)`.

### 3.2 `onModifierDown(modifier, isAutoRepeat, timestampMs)` (:116-138) — emit 지점

- **완성(second tap) 조건** :121-125, 네 항 AND:
  `state.phase === 'armed'` && `state.modifier === modifier` (같은 모디파이어) && `!isAutoRepeat` && **`timestampMs <= state.deadlineMs`** (**`<=` inclusive** — §결론2).
  → :127 `state = idle`, :128 **`return { modifier }`** (유일한 emit).
- **auto-repeat** :131-134: 완성 아니고 `isAutoRepeat` 면 → `state = idle`, `return null`. (키를 누르고 있는 것 = 탭 아님.)
- **그 외 fresh press** :136-137: `state = { phase:'down1', modifier }`, `return null`. **어떤 신선한 bare-모디파이어 press 든 first-tap 으로 (재)시작** — armed 였으나 모디파이어 불일치/윈도우 초과여도 여기로 떨어져 down1 로 리셋(테스트2·4).

### 3.3 `onModifierUp(modifier, timestampMs)` (:140-152) — arm / 팬텀-클리어

- :141 `if (phase === 'down1' && state.modifier === modifier)` → :142 `state = { phase:'armed', modifier, deadlineMs: timestampMs + DOUBLE_TAP_WINDOW_MS }`. **arm; deadline 은 여기서 확정.**
- :149 `else if (phase === 'armed' && state.modifier === modifier)` → :150 `state = idle`. **armed 상태에서 (중간 keyDown 없이) 같은 모디파이어 keyUp 이 오면 armed 해제** — allowlist 경로에서 second keydown 이 상위에서 삼켜졌을 때 stale armed 로 팬텀 완성되는 것 방지 (:145-148 주석).
- **그 외**(idle, 또는 모디파이어 불일치) → **아무 변화 없음** (fall-through, 명시 분기 없음). 예: `down1{Shift}` 인데 `up{Alt}` 오면 두 조건 다 거짓 → down1 유지.

### 3.4 `reset()` (:112-114)

- `state = { phase: 'idle' }`. 외부 강제 리셋.

### 3.5 전이표 요약

| 현재 | 이벤트 | 결과 상태 | 반환 |
|---|---|---|---|
| any | modifier==null 또는 !isModifierOnly | idle | null |
| any | keyUp(m), phase!=down1/armed 또는 모디파이어≠ | 변화없음 | null |
| down1(m) | keyUp(m) | armed(m, ts+300) | null |
| armed(m) | keyUp(m) | idle | null |
| armed(m) | keyDown(m), !repeat, ts<=deadline | idle | **{m}** |
| armed(m') / any | keyDown(m), !repeat (완성조건 미충족) | down1(m) | null |
| any | keyDown, isAutoRepeat (완성 아님) | idle | null |
| any | reset() | idle | — |

---

## 4. 트랩 클래스 × Rust 발산 위험

- **T1 임계 방향 `<=`** (:125) — **inclusive**. `ts == deadline` (정확히 300ms 갭) 도 **emit**. Rust `<=` 그대로. `<` 로 바꾸면 경계에서 한 케이스 어긋남 — 오라클엔 정확-경계 테스트 **없음**(테스트1=200<310, 테스트2=400>310) → **핀 추가 필요**(§6-E1).
- **T2 deadline precompute 앵커** (:142 vs :125) — 간격은 firstUp↔secondDown. down 에서 `ts - lastDownTs` 로 재구현하면 **의미 변경**. keyUp 에 deadline 저장 필수.
- **T3 시각 산술 / 정수타입** — JS `number`(float). 연산은 **덧셈 1회 + 비교뿐**(뺄셈·오버플로우·모듈러 없음). 음수/비단조 timestamp 도 순수 산술로 정의됨(테스트엔 단조증가만). Rust 는 **`i64` 권장**(가정상 음수 delta 안전, 현실값 오버플로우 무관). `u64` 도 동작하나 `ts + 300` 이 유일 산술이라 실질 동일 — §6 Codex 확인.
- **T5 enum 아이덴티티** — `state.modifier === modifier` (:123,141,149) = enum 동등. `PhysicalModifierToken` 4-variant. **'Mod' 부재** 재확인. Rust `#[derive(PartialEq, Eq, Clone, Copy)] enum PhysicalModifier { Cmd, Ctrl, Alt, Shift }`.
- **T6 문자열 case/whitespace** — `modifierFromKeyEvent` 는 **exact-match, no case-fold, no trim** (§2.2). code/key 문자열 그대로. Rust exact `match`/`HashMap`. `.trim()`·`toLowerCase` 0건.
- **T7 auto-repeat 위치** — 완성분기가 `&& !isAutoRepeat` 를 먼저 요구(:124)하므로, armed 에서 **repeat=true 인 second down 은 완성 안 되고 :131 로 → idle** (테스트 미커버, §6-E2).
- **T8 class → Rust struct** — 상태 단일 필드, 모든 메서드 `&mut self`, private 헬퍼는 `impl` 내 assoc fn. `process` 반환 `Option<DetectedDoubleTap>`. 깔끔 매핑, 트레잇/제네릭 불필요.

---

## 5. 오라클 케이스별 (`.test.ts`)

- **T1 (:31-36) 윈도우 내 완성** — down Shift@0→down1; up@10→armed(d=310); down@200 (200≤310) → **{Shift}**. Crux: happy-path inclusive.
- **T2 (:38-43) 윈도우 초과** — down@0/up@10(d=310)/down@400 → 400>310, 완성실패→ down1, **null**. Crux: 초과는 fresh down1 로 재시작.
- **T3 (:45-51) 중간 비-모디파이어 리셋** — armed 후 otherKey@20(modifier null)→ :101 idle; down Shift@100→ down1, **null**. Crux: 비-모디파이어가 idle 강제.
- **T4 (:53-61) 다른 모디파이어 = fresh** — armed{Shift} 상태서 down Alt@100→ 모디파이어 불일치, down1{Alt}; up Alt@110→armed{Alt,410}; down Alt@150 → **{Alt}**. Crux: 잘못된 모디파이어는 완성 아니라 새 제스처 시작.
- **T5 (:63-70) auto-repeat hold** — down Shift@0→down1; down Shift{repeat}@30→ :131 idle(**null**); up@500→ idle 유지; down@520→down1, **null**. Crux: repeat keyDown 이 pending down1 소멸.
- **T6 (:72-77) isModifierOnly false** — down Shift{isModifierOnly:false}@0→ :101 idle(null); up@10→idle; down Shift@100→down1, **null**. Crux: 코드된 모디파이어 즉시 idle.
- **T7 (:79-88) keyUp 누락된 두번째 keyDown** — down@0→down1; down@50(non-repeat)→ armed 아니므로 완성실패, down1 재시작; up@60→armed(360); down@200 → **{Shift}**. Crux: keyUp 유실 시 fresh down 이 emit 아니라 down1 재시작.
- **T8 (:90-100) allowlist 억제 → armed 클리어** — down@0→down1; up@10→armed(310); up@20(중간 keydown 없음)→ :149 idle; down@200 → **null**. Crux: armed 중 같은 모디파이어 keyUp 이 stale armed 팬텀완성 차단.
- **T9 (:102-108) reset()** — armed 후 reset()→idle; down@100→down1, **null**. Crux: 외부 리셋.
- **T10 (:110-135) 정규화** — `modifierFromKeyEvent('ShiftLeft','Shift')='Shift'`; `('MetaRight','Meta')='Cmd'`; `('ControlLeft','Control')='Ctrl'`; `('KeyA','a')=null`. `toModifierDoubleTapEvent({keyDown,ShiftLeft,Shift,shift:true})={keyDown,Shift,isModifierOnly:true,isAutoRepeat:false}`; `+meta:true`→`{Shift,isModifierOnly:false}`; `{KeyA,a}`→`{null,isModifierOnly:false}`. Crux: code 우선·Meta→Cmd·다른모디파이어→isModifierOnly false·비모디파이어→null.

### 미커버 엣지 (핀 추가 권장 — §6)

- **E1 정확 경계**: up@0→deadline 300, down@300 → `300<=300`=**emit**. `<`/`<=` 판별 유일 테스트. **필수 핀.**
- **E2 완성 press 의 auto-repeat**: armed 에서 same-mod repeat down within window → :124 `!isAutoRepeat` 실패→:131 idle, **null**. 미커버.
- **E3 armed/down1 중 다른 모디파이어 keyUp**: `down1{Shift}` + `up{Alt}`(isModifierOnly:true) → onModifierUp(Alt) 두 조건 거짓 → **down1{Shift} 유지**. 미커버.
- **E4 비단조/음수 timestamp**: 정의는 순수 산술이나 오라클 없음. 포트는 산술 그대로 유지.
- **E5 code 존재-미매핑 + key 매핑 불일치**: 예 `code='Space', key='Shift'` → code 미매핑 → key 로 'Shift'. code-우선은 둘 다 매핑될 때만 관찰됨. 미커버.

---

## 6. Codex 교차검증 open questions

1. **시각 정수타입 + 임계 방향**: `timestampMs` 를 Rust `i64` 로 주입 확정? (유일 산술 `ts+300`, 유일 비교 `ts<=deadline` :125,142). `u64` 대비 음수/비단조 안전마진 vs 원본 float 의미. 그리고 **`<=` inclusive 를 그대로**(E1 경계 = emit) 유지 확정?
2. **상태-리셋 규칙 완전성**: (a) :101 비-모디파이어/코드모디파이어 → idle; (b) :131 auto-repeat → idle; (c) :136 fresh down → down1 재시작; (d) :150 armed 중 same-mod keyUp → idle(팬텀 방지); (e) :140-152 모디파이어 불일치 keyUp → **무변화**(fall-through). 이 5개 전이가 전부인지, 특히 (e) 무변화가 의도인지.
3. **`modifierFromKeyEvent` 매핑 충실도**: 8-code + 4-key 테이블(:32-48), **code 우선**(:56), exact-match no-case-fold, `Meta*→Cmd`/`Control→Ctrl` 리네임. T4 prototype-키 풋건은 Rust `match`/`HashMap` 으로 자동 소거 — 재현 불필요 확인.
4. **class → Rust struct 매핑 clean 여부**: 단일 `state: enum{Idle,Down1(Mod),Armed{modifier,deadline_ms}}` + `&mut self` 메서드 4개(process/reset/on_down/on_up), 반환 `Option<DetectedDoubleTap>`. 트레잇/제네릭/내부가변성 불필요 확인. `Default=Idle`.
5. **type-only import 재현**: `PhysicalModifierToken`(:1, keybindings.ts:153-154) 은 값 0-의존 → suaegi 측 독립 4-variant enum 정의로 충분한지, 아니면 이미 포팅된 keybindings 크레이트의 enum 을 재사용해야 하는지(중복/단일소스).
6. **추가 핀 E1-E5** 를 포팅 시 회귀테스트로 명시 추가할지 (특히 E1 경계, E2 완성-repeat) — mutation-verify 대상.
