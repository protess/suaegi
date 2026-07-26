# Plan — native-file-drop (신규 `suaegi-filedrop` 크레이트, 단일 PR)

조사: Explore 정찰(소스 269L + 오라클 277L 통독, 이미 이식된 `clipboard_text.rs` 대조, 상류 호출자 확인).
출처: `reference/orca/` = **v1.4.146-rc.0**(setupseq 선례대로 표기).
정찰이 `clipboard-text.ts`(146) ↔ `clipboard_text.rs` 라인 대조 → **의미 드리프트 없음**, 재사용 안전.

## 0. 배치 — 신규 leaf `suaegi-filedrop`, 의존 1개
```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # measure_clipboard_text_byte_length 전용
```
`suaegi-misc`에 접는 안을 검토했으나 기각: 그 헌장의 핵심어는 *self-contained*이고 기존 16개 모듈은
전부 **크레이트 내 import가 0**이다(예외 `js_ws`는 2줄짜리 공유 프리미티브).
`clipboard_text`는 747줄짜리 정책 모듈이라 여기 의존하면 크레이트 내부 위상이 뒤집힌다.
그리고 이건 **캡 2개 + 프로토콜 경계 타입가드 + 6-variant 라우팅**으로,
`quickcmd`/`claude-roster`/`project-runtime`/`workspace-cleanup`이 네 번 같은 판단을 내린 바로 그 모양이다.
**`js_trim` 불필요**(이 모듈엔 `.trim()`이 0회). `serde`/`serde_json`/`regex` 전부 불필요(L10).

## 1. 계약 결정

- **L1 — ⚠ `NATIVE_FILE_DROP_MAX_PATH_BYTES`는 **총합 캡**이지 경로별 캡이 아니다.**
  누산기가 루프 **밖**(`O:165`)에 하나 있고, 경로별 `stopAfterBytes`는 **남은 예산**
  `maxPathBytes - byteLength`(`O:168`)다. **이름이 거짓말한다** — "max path bytes"는 경로별로 읽힌다.
  ⚠ 경로별 구현은 오라클 대부분을 통과하고 **`test:158-171` 하나에만 걸린다**. 픽스처 하나가 두 해석을 가른다.
- **L2 — 거부 시 보고되는 `byteLength`는 **초과 문자를 포함한 절단 총합**이다.**
  `test:158-171`에서 그럴듯한 값이 넷(`262144`/`262145`/`262159`/`262160`) 있고 정답은 **`262145`**.
  이미 이식된 `measure_clipboard_text_byte_length`가 그 부분합을 정확히 낸다(`clipboard_text.rs:100-106`)
  → **그대로 호출하고 `p.len()`으로 "단순화"하지 말 것.** 수락 경로에선 산술적으로 동일해 오라클을 통과하지만
  **모든 거부에서 보고 값이 조용히 달라진다**.
- **L3 — `??=`는 nullish이지 falsy가 **아니다****(`O:107`). `||=`와는 **빈 문자열에서만** 갈리는데
  오라클 픽스처가 `'leaf-1'`(truthy) 하나뿐이라 **구별 불가**. Rust `Option::get_or_insert_with`가 충실한 철자.
- **L4 — `O:123`의 가드는 **혼합 술어**다**: `destinationDir === undefined && <truthy dir>`.
  빈 문자열 dir는 **latch 없이 건너뛴다** → 뒤의 비어있지 않은 dir가 여전히 이긴다.
  잘못된 단순화 둘 다 위험: (a) `is_some` 우선-승리는 `''`에 latch돼 `O:129`의 `!destinationDir`가 발동해
  잘못 `rejected`가 된다; (b) 마지막-승리는 `test:52-64`에 걸린다(넷 중 유일하게 오라클이 잡는 것).
- **L5 — **디코딩이 존재하지 않는다**. 모듈 이름이 오해를 부른다.**
  `decodeURIComponent` 없음, `file://` 처리 없음, 퍼센트 디코딩 없음, 플랫폼 munging 없음, 따라서 `URIError`도 없음.
  **추가하지 말 것.** `destinationDir`과 `paths` 원소는 **바이트 보존 불투명 문자열**로 다룬다.
- **L6 — `NaN`/`±Infinity` 캡은 캡을 **무력화**하고 `Option<u64>`로는 표현 불가.**
  `options.maxPaths ?? 256`(`O:154`)은 `NaN`을 통과시키고(`NaN`은 nullish 아님) `pathCount > NaN`은 `false`,
  `Number.isFinite(NaN)`도 `false` → **NaN 캡은 전부 수락**한다. `Infinity`도 수락(단 O(n) 전체 스캔).
  음수 `maxPathBytes`는 첫 글자에서 거부하지만 ⚠ **빈 `paths` 배열은 여전히 수락**된다
  (`O:171` 검사가 루프 **안**에 있고 `O:181`이 무조건 반환하므로).
  → 두 캡 다 **`Option<f64>`**로 모델링(`clipboard_text.rs`의 S2/S6와 동일 선례). 오라클 커버리지 0.
- **L7 — ⚠⚠ **오라클이 모든 상수를 심볼로만 참조한다** → 상수 값이 틀려도 전 테스트가 초록이다.**
  `NATIVE_FILE_DROP_MAX_PATHS`, `..._MAX_PATH_BYTES`, `NATIVE_FILE_DROP_TARGET`의 다섯 멤버,
  `ORCA_INTERNAL_FILE_DRAG_TYPE` — **리터럴과 한 번도 비교되지 않는다**.
  `MAX_PATHS = 512`거나 `'file_explorer'`거나 `'text/x-suaegi-file-path'`인 포트가 **전부 통과**한다.
  이것들은 **IPC 경계와 DOM dataset을 건너는 와이어 값**이다 →
  **모든 리터럴을 명시적으로 핀으로 박는다**(MIME 문자열, 다섯 타깃 문자열, `256`, `262144`).
  ⚠ 타깃 5개 중 둘은 **키와 값이 다르다**(kebab-case 값): `fileExplorer→'file-explorer'`,
  `projectSidebar→'project-sidebar'`. 그리고 `Object.values` 순서가 `O:71`에서 load-bearing이다.
- **L8 — `too-many-paths` 거부는 `byteLength: 0`을 **하드코딩**한다**(`O:157`) — 바이트 계산을 아예 시도하지 않는다.
  `paths-too-large`의 절단 총합과 **다른 규칙**이다. 통합하지 말 것.
- **L9 — 두 캡 다 **엄격 `>`**이고 정확히 캡이면 **수락**이다**(오라클 `test:246-254`, `:264-269`이 못박음).
  모듈 내 5곳, 모듈 밖 3곳 전부 `>`로 일치한다.
- **L10 — 입력 검증은 **손코딩 `JsValue` 트리**로. `serde`는 세 가지 이유로 **틀리다**.**
  (a) `serde_json::Number`는 `NaN`/`±Infinity`를 표현 못 해 `O:83`의 유한성 가드가 **도달 불가**가 되는데
  실제 IPC(structured clone)는 그 값을 전달한다; (b) `deny_unknown_fields`는
  `isNativeFileDropPayload`가 **모든 분기에서 허용하는** 추가 키를 거부한다;
  (c) `'rejected'` 분기는 `paths`를 **아예 보지 않는다**(`O:239-245`) — derive로 표현 불가.

## 2. 오라클 & 핀
**오라클 전량**(`test:1-277`).

**추가 핀(오라클 침묵):** **L7 모든 리터럴 값**(MIME·타깃 5개·256·262144·`Object.values` 순서);
L1 경로별 vs 총합을 가르는 케이스; L2 거부 시 보고 `byteLength`가 정확히 `262145`;
L3 빈 문자열 `terminalPaneLeafId`(`??=` vs `||=`); L4 빈 문자열 dir가 latch하지 않고 뒤의 dir가 이김;
L6 `NaN`/`Infinity`/음수 캡 + **음수 캡 + 빈 배열 → 수락**; L8 `too-many-paths`의 `byteLength == 0`;
정찰이 짚은 무커버 분기: `resolveNativeFileDropPath`의 **editor** 분기·**`None` 반환**(생산 기본 경로!)·
`terminalPaneLeafId` 2개 이상일 때 첫-승리·`terminalTabId` 부재·조건부 spread의 **생략** arm·
`too-many-paths`가 `createNativeFileDropPayload`까지 가는 경로·
**`O:199`와 `O:204`의 순서**(초과 경로 **및** `{target:'rejected'}` 동시 케이스)·
composer/project-sidebar 타깃; `measureNativeFileDropPathBytes`(테스트도 생산 호출자도 **0**).

*mutation:* L1 경로별 캡으로, L2 `p.len()`으로 단순화, L3 `||=` 의미론, L4 두 잘못된 단순화 각각,
L6 `Option<u64>`로·`??`를 `||`로, L7 각 리터럴 변경, L8 두 거부의 byteLength 규칙 통합,
L9 `>`→`>=`, L10 추가 키 거부·`rejected`에서 paths 검사.

## 3. 순서
단일 PR. 검증기 하나에 두 캡·측정·거부 보고가 엮여 있고 오라클 대부분이 그걸 탄다 → seam 없음.
불변식: 신규 leaf·의존 1개(§0), **총합 캡**(L1), **절단 총합 보고**(L2), nullish 대입(L3),
혼합 술어(L4), **디코딩 없음**(L5), `Option<f64>` 캡(L6), **모든 와이어 리터럴 핀**(L7),
두 거부 규칙 분리(L8), 엄격 `>`(L9), 손코딩 입력 트리(L10), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
