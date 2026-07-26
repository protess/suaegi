# Plan — project-execution-runtime (신규 `suaegi-project-runtime` 크레이트, 단일 PR)

조사: Explore 정찰(`project-execution-runtime.ts` 294L + `.test.ts` 332L 통독, 상류 호출자 실측,
크레이트 헌장 대조). **소스에 import가 0개**다. 오라클 12케이스.

## 0. 배치 — 신규 leaf `suaegi-project-runtime`, 의존 1개

```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # js_trim 전용
```
- **`serde_json` 불필요.** `unknown` 표면 전체가 "객체냐 / `kind`가 문자열이면 무엇이냐 / `distro`가
  문자열이면 무엇이냐" 셋뿐이다. 키 순서·수치 포맷·중복 키를 **하나도 관측하지 않는다**.
  파싱은 이미 상류에서 끝났다(`local-project-runtime-resolution.ts:36-37`이 역직렬화된 store 값을 넘긴다).
  리포 기준선(`suaegi-mcp/Cargo.toml:11-20`)은 **`JSON.parse` 자체의 엣지케이스를 재현해야 할 때만** 승인 →
  해당 없음. → **작은 입력 enum을 손코딩**한다.
- **`suaegi-misc`에 넣지 않는다** — 그 헌장은 "작고 자족적인 순수 문자열/수치 헬퍼 묶음"인데
  이건 4-enum 타입 격자 + repair 프로토콜을 가진 정책 해석기다(상류 소비자 25곳).
- **`regex` 없음.** 유일한 정규식 `/[\\/]/`(`O:292`)는 무플래그 2-ASCII 문자 클래스 → `char` 술어.

## 1. 계약 결정

- **E1 — `unknown` 입력은 손코딩 enum.** 관측 가능한 전부: `is_record`(`typeof === 'object' && !== null`,
  **배열은 true, 함수는 false**), `kind`가 문자열인지와 그 값, `distro`가 문자열인지와 트림 결과.
  `distro: null` / `distro: 42` / `distro` 부재는 **출력이 전부 동일** → 입력 모델에서 하나로 접어도 된다.
- **E2 — ⚠ `Wsl` 변형 4개를 **절대 한 enum으로 합치지 말 것**. 차이가 load-bearing이다.**
  `LocalWindowsRuntimePreference::Wsl`은 `distro: String`(**항상 비어있지 않다**),
  `GlobalWindowsRuntimeDefault::Wsl`은 `distro: Option<String>`. 이 nullability 차이가 동작을 가른다:
  **선호도**의 distro가 없으면 `inherit-global`로 붕괴하고(`O:104`), **전역 기본값**의 distro가 없으면
  `wsl`로 남아 나중에 `wsl-distro-required` repair를 낳는다(`O:116`→`O:183`).
  합치면 `T:24-26`이 `inherit-global`에서 `wsl`로 뒤집힌다.
  `windows-host`도 **세 union에** 나온다(둘은 payload 없음, 하나는 payload 있음) → 별도 enum 4개.
- **E3 — ⚠ `context.wslAvailable === false`는 **엄격 비교**이고, 이게 **흔한 프로덕션 경로**다.**
  `undefined`(= WSL 프로브가 아직 캐시되지 않음)는 repair를 **내면 안 된다**.
  실제 호출자가 프로브 전에는 항상 `undefined`를 넘긴다(`local-project-runtime-resolution.ts:29-31`).
  → **`wsl_available == Some(false)`**. `!wsl_available.unwrap_or(true)`를 부주의하게 쓰거나
  `unwrap_or(false)`를 쓰면 **프로브 이전의 모든 resolve가 가짜 `wsl-unavailable` repair가 되고**,
  호출자는 repair에서 throw한다(`local-windows-terminal-runtime.ts:46-50`).
- **E4 — ⚠ `Some(&[])`와 `None`은 **다르다**.** `Array.isArray([])`는 true → **빈 목록은 "모든 distro가
  없음"**을 뜻하고, `null`/부재는 "모름, 있다고 가정"이다(`O:273`).
  `if distros.is_empty() { return false }`로 접으면 의미가 뒤집힌다. 상류도 의도적으로 구분한다
  (`local-project-runtime-resolution.ts:32`).
- **E5 — 건초더미는 정규화하지 않는다.** `distro`는 트림되지만(`O:284`) `availableWslDistros`의 원소는
  **트림도 케이스폴딩도 하지 않는다**(`O:273`). `.includes`는 정확·대소문자 구분.
  → `iter().any(|d| d == distro)`. `eq_ignore_ascii_case`/`trim` 추가 금지.
- **E6 — ⚠ 같은 줄에 **정규식과 full-Unicode `toLowerCase`가 붙어 있다**. 가장 실수하기 쉬운 지점.**
  `O:292` `value.trim().split(/[\\/]/).pop()?.toLowerCase()`:
  - `.trim()` = **ECMAScript** → `suaegi_misc::js_trim`(`O:284`도 동일).
  - `.split(/[\\/]/)` = 정규식이지만 **무플래그 2-ASCII 클래스** → `char` 술어. `regex` 크레이트 불필요.
  - `.toLowerCase()` = **full Unicode** → **`str::to_lowercase()`**. ⚠ 옆에 정규식이 있다고 해서
    `to_ascii_lowercase`로 쓰면 안 된다 — 이 파일엔 `/i` 정규식이 **하나도 없다**.
    (`suaegi-browser-url` B2와 **정반대 방향의** 함정이다: 거기선 `/i`라서 ASCII-only가 맞았다.)
    [[js-lowercase-two-mechanisms]]
- **E7 — repair cacheKey 두 개는 **문자열이 다르다**. 통합 금지.**
  `O:191`은 리터럴 `:default`를 **하드코딩**하고 distro를 보간하지 **않는다**;
  `O:208`은 `${distro ?? 'default'}`로 **보간한다**(그리고 그 `?? 'default'`는 **도달 불가 죽은 코드** —
  `O:183`이 이미 distro가 비어있지 않음을 보장한다). 두 `format!`을 **따로 리터럴로** 유지한다.
  오라클 `T:278`이 `'project-1:repair:wsl-distro-required:default'`를 못박는다.
- **E8 — `windows-host` cacheKey는 **의도적으로 reason을 뺀다****(`O:238` `${projectId}:windows-host`) →
  `project-override`와 `global-default` 결과가 **캐시 키를 공유한다**. "개선"하지 말 것.
  `wsl` 해석 키(`O:222`)도 reason은 빼고 distro는 넣는다.
- **E9 — `reason: 'migration-fallback'`은 **도달 불가**다.** union(`O:13`)과 헬퍼 파라미터(`O:229`)에는
  있지만 **호출자가 아무도 넘기지 않는다**(`O:163`/`O:175` 둘뿐). 변형은 남기되 **생성하지 않고**
  모듈 doc에 명시한다. `deriveGlobal…`의 `fallbackReason`을 여기에 배선하는 "완성"은 **없는 동작의 발명**이다.
  선례: `protocol_compat.rs:15-23`.
- **E10 — 해석 결과는 **2-variant sum type**.** `{status:'resolved', runtime}` XOR
  `{status:'repair-required', repair}`(`O:53-55`) — repair일 때 `runtime` 필드가 **아예 없다**.
  `struct { runtime: Option<_>, repair: Option<_> }`는 "둘 다 Some"·"둘 다 None"을 표현 가능하게 만든다.
- **E11 — ⚠ 검사 **순서** 두 곳이 계약이고 **둘 다 오라클 미검증**이다.**
  ① `resolveWslRuntime`: `!distro`(`O:183`)가 가용성(`O:196`)보다 **먼저** →
     `distro: null` + `wslAvailable: false`면 **`wsl-distro-required`**이지 `wsl-unavailable`이 아니다
     (오라클엔 이 조합이 없다).
  ② `getWslRepairReason`/`getLegacyWslFallbackReason`: **unavailable이 distro-missing보다 먼저**
     (`O:261`→`O:263`, `O:247`→`O:250`). 레거시 경로는 `T:79-89`가 못박지만 **resolve 경로는 미검증**.
- **E12 — 선호도의 distro가 망가지면 `inherit-global`이지 `windows-host`가 **아니다****(`O:104`).
  즉 프로젝트 설정이 손상되면 **전역 기본값으로 위임**하고, 그 결과 여전히 WSL이 될 수 있다.
  "안전하게" `windows-host`로 바꾸면 사용자에게 보이는 런타임 선택이 바뀐다. 오라클 `T:24-26`.
- **E13 — `normalizeGlobalWindowsRuntimeDefault`는 **오라클이 import조차 안 한다**.**
  비-레코드 입력 → `windows-host`(`O:111-112`) 가지는 직·간접 커버리지가 **0**이다 → **핀 직접 작성**.
  그리고 이 함수엔 `inherit-global` 케이스가 **아예 없다** — `inherit-global`도 `windows-host`가 된다(`O:119`).
- **E14 — reason enum 둘을 합치지 말 것.** `ProjectExecutionRuntimeRepairReason`(3개,
  `wsl-unavailable`/`wsl-distro-missing`/`wsl-distro-required`)과
  `LegacyWindowsRuntimeFallbackReason`(**2개**, `legacy-` 접두)은 값도 개수도 다르다
  (레거시엔 `distro-required`가 **없다** — null distro를 통과시키기 때문, `O:139`).
  합치면 잘못된 문자열이 repair cacheKey로 새고 불가능한 레거시 상태가 표현 가능해진다.
- **E15 — `isWslShell`은 **오라클 커버리지 0**이다**(`O:131`의 우변에 도달하는 테스트가 없다 —
  유일한 후보 `T:43-47`이 `localAgentRuntime:'host'`라 먼저 반환된다).
  **두 구분자 모두**로 분할하고(`C:/…/wsl.exe`와 `C:\…\wsl.exe` 둘 다 매치),
  마지막 세그먼트에 **정확 일치**(`wsl.exe` 또는 `wsl`) — `wsl.exe.bak`·`mywsl`은 false,
  `" wsl.exe "`는 트림 덕에 true. 전부 핀 직접 작성.
- **E16 — 레거시 우선순위.** `localAgentRuntime === 'host'`가 **모든 것을 이긴다**(`O:127`, 터미널 셸 스니핑보다 먼저).
  distro는 `localAgentWslDistro` → `terminalWindowsWslDistro` 순(`O:133-134`, 둘 다 `normalizeDistro` 경유라
  공백뿐인 agent distro는 terminal로 폴백). `fallbackReason`은 **항상 존재**하고 값이 `null`일 뿐이다.

## 2. 오라클 & 핀

**오라클 12케이스 전량:** `T:9-20` 3종 왕복 / `T:22-30` `null`·공백 distro·미지 kind → inherit-global /
`T:34-39` null 설정 → host+null / `T:41-52` `'host'`가 wsl 셸을 이김 / `T:54-65` agent distro 우선 /
`T:67-77` terminal distro 폴백 / `T:79-89` `legacy-wsl-unavailable` / `T:91-101` `legacy-wsl-distro-missing` /
`T:105-125` 비-Windows → `local-host` / `T:127-147`·`T:149-169` global-default host /
`T:171-192` global-default wsl / `T:194-214`·`T:216-237` project-override / `T:239-259` `wsl-unavailable` /
`T:261-281` `wsl-distro-required` + **리터럴 `:default`** / `T:283-309` 프로젝트별 독립 키 /
`T:311-331` `wsl-distro-missing`.

**추가 핀(오라클 침묵):** E3 `wslAvailable` **부재** → repair 없음(가장 중요); E4 `Some([])` vs `None`;
E5 `["Ubuntu"]`에 `"ubuntu"`·`" Ubuntu"` 불일치; E6 U+FEFF/U+0085 양방향·U+212A가 `to_lowercase`로 접힘;
E11① `distro:null` + `wslAvailable:false` → **`wsl-distro-required`**; E11② resolve 경로에서
unavailable이 distro-missing을 이김; E13 비-레코드 → `windows-host`·`inherit-global` → `windows-host`;
E15 `isWslShell` 전량(두 구분자·대문자 `WSL.EXE`·`" wsl.exe "`·`wsl.exe.bak`·`mywsl`·비문자열);
E8 두 reason이 같은 host cacheKey를 만든다(충돌을 **명시적으로** 단언); E1 배열 입력·`undefined`·문자열·수치;
E12 손상된 distro가 전역 기본값으로 위임되어 **WSL로 끝날 수 있음**(end-to-end);
E16 `undefined` 설정·`'auto'` 런타임·`{kind:'wsl'}` distro 부재 → `distro: None`인 wsl 기본값.

*mutation:* E2 두 Wsl 변형 통합, E3 `unwrap_or`류로, E4 빈 슬라이스를 `None`으로, E5 트림/케이스폴딩 추가,
E6 `str::trim`·`to_ascii_lowercase`, E7 두 cacheKey 통합·`:default` 보간, E8 host 키에 reason 추가,
E9 `migration-fallback` 배선, E11 두 순서 뒤바꾸기, E12 `windows-host` 반환, E13 비-레코드를 wsl로,
E14 reason enum 통합, E15 구분자 하나만·접두 일치 허용, E16 distro 우선순위 교환.

## 3. 순서
단일 PR. 정찰도 단일 PR을 권고했다 — 세 private 헬퍼(`normalizeDistro`/`isKnownMissingDistro`/`isRecord`)와
공유 context 타입을 모든 함수가 쓰므로 어떤 분할도 넷을 복제한다.
불변식: 손코딩 입력 enum·의존 1개(§0), **4-enum 분리**(E2), 엄격 `Some(false)`(E3), `Some([])`≠`None`(E4),
건초더미 미정규화(E5), `js_trim` + **full-Unicode** lowercase(E6), cacheKey 두 형태 분리(E7),
host 키의 의도적 충돌(E8), 도달 불가 변형 보존(E9), sum type(E10), 검사 순서 둘(E11),
손상 시 위임(E12), 미커버 함수 핀(E13), reason enum 분리(E14), `isWslShell` 전량 핀(E15),
레거시 우선순위(E16), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[js-lowercase-two-mechanisms]],
[[suaegi-impl-model-sonnet]]
