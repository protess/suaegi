# Plan — 키바인딩 시스템 (`suaegi-keys` leaf 크레이트) 확정

조사: `docs/superpowers/research/2026-07-23-keybinding-resolution.md` (Orca @
v1.4.150-rc.0, 인용 file:line 고정). Codex 교차검증 판정 **IMPLEMENTABLE-AFTER-FIXES**
반영(iced 0.14 API를 소스로 직접 확인, `to_latin` 트랩 발견). 이 문서가 구현 계약이다.

## 0. 결정 (조사 + Codex 확정)

- **새 leaf 크레이트 `suaegi-keys`** (workspace member). 의존: `serde`/`serde_json`/
  `thiserror`/`tempfile`(파일레이어)만 — **iced/tokio/다른 suaegi 크레이트 0**. Orca
  `keybindings.ts:2-3`이 타입 2개만 import(런타임 의존 0)라 100% mutation 검증 가능.
  `suaegi-secrets`/`suaegi-http` leaf 선례.
- **클론 가치**: suaegi에 앱-레벨 키보드-액션 레이어 0. 모든 비-터미널 UX(탭전환·사이드바·
  quick-open)가 이 레이어에 gated. `terminal/input.rs`는 raw 키를 터미널로 넘기기만.
- **범위**: 순수 해석/저장만. Settings 단축키 에디터 UI(~45 renderer 파일)와 dispatch
  (액션 실제 실행)는 deferred. 이 마일스톤은 "이 키 이벤트 → 어느 action id?"에 답하고
  커스터마이즈를 영속화한다.
- **불변식**: 사용자 글로벌/Orca 경로 절대 안 씀(suaegi config root만, temp+rename 원자적).
  transient 파싱 이슈는 diagnostic로 강등(false-negative 금지). 모든 함수 mutation 검증.

## 1. Codex 반영 픽스 (구현자 필독)

- **F1 — M3의 `to_latin()` 금지 (결정적)**: iced `iced_core::Key::to_latin(physical)`
  (`key.rs:60-117`)는 `c < 0x370` 스칼라를 "이미 Latin"으로 **그대로 반환**한다
  (`:68-70`). macOS Option+A는 `å`(U+00E5=229 `< 0x370`)로 compose되므로 `to_latin`이
  physical fallback 없이 `Some('å')`를 돌려줌 — Orca `shouldUseMacOptionLetterPhysicalFallback`
  (`keybindings.ts:1864-1876`)의 정반대. **resolver/어댑터는 raw `Key`/`Physical::Code`/
  `Modifiers`를 Orca fallback 포팅에 먹이고 `to_latin()`을 이 경로에서 절대 안 쓴다**
  (`input.rs:43`이 copy/paste용으로 이미 쓰고 있어 유혹적 — 명시 금지).
- **F2 — `tab.newAgent.*` 템플릿 패밀리를 크레이트에 넣지 말 것**: suaegi 에이전트 목록은
  `suaegi-term`(alacritty/portable-pty 끌어옴)의 33-entry `&'static str` 테이블
  (`agent.rs:103-639`)이라 leaf `suaegi-keys`가 의존 불가. Orca `TuiAgent` union과도 별개
  목록. **84행 레지스트리 전체 포팅하되 이 템플릿 패밀리만 제외** → M6(앱 경계)에서
  suaegi-term live `agent_defs()` 주입해 배선.
- **F3 — M5 원자적 쓰기는 `suaegi-core::persistence::Store` 의존 금지**: leaf 격리 위해
  자체 ~15줄 `tempfile` temp+rename 구현(의식적 소량 중복, 오류 아님).
- **F4 — `KeybindingInput`은 4 canonical bool**: Orca 구조체(`:156-169`)는 8개
  modifier-ish 필드지만 `App.tsx` 두 DOM 이벤트 생성 콜사이트용 compat shim
  (`hasModifier`의 `??` fallback `:1117-1130`)일 뿐 — Rust는 `alt/meta/control/shift`
  4개 bool만.

## 2. 마일스톤 (smallest-first, 각 독립 mutation-verifiable; TS 테스트 벡터가 오라클)

`keybindings.test.ts`(1721줄, 68 describe/it, ~100% 이식 가능 — vitest+모듈만 import,
DOM/electron 0)를 각 마일스톤의 mutation 오라클로 함께 포팅한다.

- **M1 — 레지스트리 + 코드 파스/canonicalize.** `KeybindingActionId`(닫힌 union,
  **템플릿 agent 패밀리 제외**), `KeybindingDefinition`, 84 defs(플랫폼별 defaults·scope·
  flags), `parseKeybinding`(`:1294`)/`canonicalizeParsedKeybinding`(`:1327`)/modifier
  grammar(`Mod` 가상→darwin=Cmd/else=Ctrl `:1837-1848`). `formatKeybinding`(글리프)도
  여기(충돌 메시지용). *crux:* 84행×3플랫폼 무오류 전사. *mutation:* 파스가 modifier/키를
  놓치거나 `Mod` 플랫폼 해석이 뒤집히면 잡는 테스트.
- **M2 — normalize/validate + digit-index.** `normalizeKeybinding*`(`:1414`), bare/
  shift-only/digit-index finalizer(1-9→1 canonical `:1475-1486`), per-action 규칙.
  `KeybindingValidationResult`는 Rust enum(TS union 오버로드 흉내 금지). *crux:* digit-index
  canonical-to-`1`와 충돌 아이덴티티 상호작용.
- **M3 — 이벤트→액션 resolver (crown jewel, 최고 리스크).** `KeybindingInput`(4 bool),
  `keybindingFromInput`(`:1739`)/`keybindingMatchesInput`(`:2018`)/`keybindingMatchesAction`
  (`:2083`)/`matchKeybindingDigitIndex`(`:2113`) + **모든 fallback verbatim 포팅**:
  mac Option-compose(`:1864-1892`), 비-Latin/AltGr physical fallback(`:1598-1621`,
  `shouldUseSemanticPunctuation :1935`), 터미널 정책(orca-first/terminal-first
  `:1809-1835`). **F1(to_latin 금지) 적용.** 순수 struct 대상 먼저 포팅 + 이식한 TS
  벡터(~105 fallback 케이스)로 검증. *crux/mutation:* Option+accented가 shortcut로
  오발화 안 함, AltGr 텍스트가 shortcut로 안 뺏김.
- **M4 — effective bindings + 충돌 탐지.** `getEffectiveKeybindingsForAction`(`:1772`,
  defaults+overrides 병합), `findKeybindingConflicts`(`:2235`, scope/conflictGroup
  버킷팅 + 플랫폼별 conflict identity `:2043-2065`, customized-only 리포팅, digit-index
  특수). *crux:* conflict identity 함수 + "customized action 참여 시에만 리포트" 규칙.
- **M5 — 파일 레이어.** `readKeybindingFile`(`:248`, 레거시 flat root 관용 `:273-276`,
  drop-conflicts fixpoint `removeConflictingOverrides :212-246`), `writeKeybindingOverride`
  (`:426`, validate·충돌거부·active-platform-only write `:454-468`), **자체 tempfile
  temp+rename(F3)**. config: `dirs::config_dir()/suaegi/keybindings.json`(data.json 형제,
  `persistence_thread.rs:27-34` 선례), 문서형 `{version, keybindings, platforms:{darwin,
  linux,win32}}`(`keybinding-file.ts:33-43`). **레거시 마이그레이션 2개
  (`migrateLegacyKeybindings :302`, `seedLegacyTabSwitchBindings :335-381`)는 skip**
  (fresh 클론엔 트리거 불가, 의미있는 mutation 테스트 불가 — 문서에 의도적 skip 명시).
  *crux:* drop-conflicts fixpoint, active-platform-only write. *불변식:* 글로벌 config 금지.
- **M6 (크레이트 아님, 앱 통합) — iced 어댑터 + agent 패밀리 + 액션 1개 배선.** suaegi-app
  에서 iced `KeyPressed{key, physical_key, modifiers}`(`input.rs:282-302`에 이미 흐름)
  → `KeybindingInput` 매핑(F1 준수), resolver 호출, action 라우팅. **템플릿 agent 패밀리를
  여기서** suaegi-term `agent_defs()` 주입해 구성. 유일한 비순수 코드, 마지막·최소·[일부 사람눈].

## 3. 순서
M1 → M2 → M3 → M4 → M5 (전부 순수 leaf, 자율검증) → M6 (앱 어댑터, 일부 사람눈).
관련: [[suaegi-workflow]], [[mutation-verify-regression-tests]], [[path-denylist-case-insensitive]]
