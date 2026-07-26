# Plan — setup-agent-sequencing + setup-runner-command (신규 `suaegi-setupseq` 크레이트, 단일 PR)

조사: Explore 정찰(4개 파일 통독 — 소스 257L+87L, 오라클 425L+69L).
**이 모듈들은 셸 명령 문자열을 만든다** → 인용/이스케이프가 최고 가치 축이다.

## 0. ⚠ 출처 버전이 바뀌었다 — 이 PR은 **1.4.146-rc.0** 기준이다
지금까지의 74개 provenance 주석은 `v1.4.150-rc.0`(세션 스크래치패드 사본)을 인용한다.
**그 사본은 이번 세션 중 정리됐다.** 남은 완전한 사본은 리포의 `reference/orca/`(HEAD `c0f0810`,
`package.json` = **1.4.146-rc.0**)뿐이다.
두 사본은 `mcp-config.ts`에선 바이트 동일이었지만 `keybindings.ts`에선 **다르다** → 진짜 다른 스냅샷이다.
→ **이 PR의 주석은 `@ v1.4.146-rc.0`으로 정확히 적는다.** 기존 74개는 그때 쓴 소스 기준으로 맞으므로 건드리지 않는다.
[[orca-source-location]]

## 1. 배치 — 신규 leaf `suaegi-setupseq`, 두 모듈 한 크레이트
`setup-agent-sequencing`은 `setup-runner-command` 없이 리뷰 불가하다(`markerPath`가
`runnerScriptPathForShell`에서 나오고 windows/posix 분기가 `resolution.shell`이다).
그리고 `setup-runner-command` 단독은 오라클 6 assertion뿐이라 독립 PR로 얇다.
```toml
[dependencies]
suaegi-misc = { path = "../suaegi-misc" }   # js_trim
suaegi-path = { path = "../suaegi-path" }   # is_windows_absolute_path_like
```
- **K14 — `suaegi-path::is_windows_absolute_path_like`는 여기선 **재사용이 맞다**.**
  `setup-runner-command.ts:1`이 `cross-platform-path`에서 **바로 그 함수**를 import하고
  이미 `crates/suaegi-path/src/cross_platform_path.rs:77`에 이식돼 있다.
  ⚠ mcp-config M1에선 같은 함수를 **거부**했는데(그쪽 계약은 UNC 2-컴포넌트를 요구했고 이 함수엔 없다),
  여기선 Orca가 실제로 이 함수를 쓰므로 **재사용이 충실한 포트**다. 대비를 주석에 남긴다.
- **K13 — `suaegi-misc`에 `js_trim_start`를 추가하지 말 것.** 5개 이상이 의존하는 크레이트를 건드리게 된다.
  필요하면 로컬에서 `trim_start_matches(is_js_whitespace)`로 손코딩.

## 2. 계약 결정

- **K1 — 이름이 같고 순서가 반대인 enum 둘을 **합치지 말 것**.**
  `SetupRunnerCommandPlatform = 'windows'|'posix'`(입력 힌트)와
  `SetupRunnerCommandShell = 'posix'|'windows'`(출력 셸)는 **다른 타입**이다 —
  `'windows'` 플랫폼이 `'posix'` 셸을 낳을 수 있다(WSL UNC 경로). Rust에서 별도 타입 2개.
- **K2 — `quotePosixArg`: `^[A-Za-z0-9_./:-]+$`면 **맨몸**, 아니면 `'…'`로 감싸고 `'` → `'\''`.**
  두 파일에 **동일 본문이 중복**돼 있다(`SAS:237-242`, `SRC:77-83`) → Rust에서도 모듈별 복제
  (`utf16_slice_prefix` 선례와 동일 헌장). 공백·`"`·`$`·백틱·`%`·`^`·개행 전부 중화된다. **안전.**
- **K3 — `quoteWindowsArg`는 `"` → `""`인데 그건 cmd.exe 이스케이프가 **아니다**.**
  `CommandLineToArgvW` 규약이라 cmd 파서에선 인용 패리티가 깨진다. **축자 이식 + 문서화.**
- **K4 — ⚠⚠ `escapeCmdSetValue`가 **오라클 커버리지 0**이고 **바로 그게 주입 가능한 지점**이다.**
  `SAS:248-250`: `"` → `""` 후 `[%!^]` → `^$&`.
  `set "V=…"` 안에 들어가고 `wrapCmd`가 전체를 **다시** 인용하면서 그 따옴표를 또 두 배로 만든다 →
  `markerPath = C:\a"&calc&".done`이면 cmd 패리티가 깨져 **`&calc&`가 인용 밖으로 나와 실행된다**.
  프로덕션에선 도달 불가(`"`는 NTFS 파일명에 불법이고 nonce는 UUID)라 상류가 고치지 않은 것으로 보인다.
  → **축자 이식하고 "고치지 말 것"**, 그리고 **주입 가능한 출력 자체를 핀으로 박는다**
  (`escape_cmd_set_value("a\"b") == "a\"\"b"` + `"`를 포함한 경로의 전체 명령 스냅샷),
  주석에 **의도적으로 보존된 상류 위험**임을 명시.
  ⚠ `^%`는 **cargo-cult**다 — cmd에서 `^`는 `%`를 이스케이프하지 않는다. 그래도 축자 재현.
  (`^!`는 `wrapCmd`가 `/v:on`을 주므로 **실제로 유효**하다.)
- **K5 — `startupCommand`의 맨몸 보간은 **설계상 주입 가능**하다**(`SAS:132` `exec ${…}`) —
  그게 사용자의 셸 명령 자체이고 단어분할·확장이 **되어야** 한다. `SAS:126-129`의 휴리스틱 두 개가 유일한 가드.
  축자 이식. (`SAS:130`의 `eval '…'` 경로는 `quotePosixArg` 후 `eval`이 재파싱 — 역시 설계다.)
- **K6 — `waitTimeoutSeconds`는 `Math.max(1, Math.floor(x))`뿐이다**(`SAS:102`, `:193`).
  `NaN`/`Infinity`가 살아남아 셸 텍스트에 `NaN`/`Infinity`로 박힌다. Rust는 `i64`로 모델링하고
  **그 도달 불가 케이스를 주석으로 명시**한다(f64를 쓰면 재현해야 한다 — 굳이 하지 말 것).
- **K7 — nonce에 `:`가 들어가면 `IFS=: read -r seen status` 분해가 깨져**(`SAS:106`)
  `$seen`이 nonce와 영영 같아지지 않고 **게이트가 항상 타임아웃**한다. 텍스트상 안전하지만 의미상 위험 → 문서화.
- **K8 — `wslUncToLinuxPath`의 `(\/.*)?$`는 `/s`가 없어 `.`이 LF/CR/U+2028/U+2029를 **제외**한다**(`SRC:73`).
  그런 문자가 든 경로는 조용히 `/`로 변환된다. 주입이 아니라 **다른 실패**다. 축자 이식 + 핀.
- **K9 — WSL 호스트 검사의 `/i`는 `/u`가 없어 **ASCII 전용 폴딩**이다**(`SRC:66-69`).
  오라클이 `WSL.LOCALHOST` 대문자를 쓴다. Rust `(?i)`는 유니코드라 넓어진다 → ASCII 비교로.
- **K10 — 런치 커맨드 힌트: env 값이 이기되 **공백뿐이면 폴백**한다**(`SAS:16-22`, 오라클 `SAST:38-51`이
  `'   '`를 못박는다). ECMAScript trim → `js_trim`.
- **K11 — `getSetupAgentSequenceShellForTests`는 테스트 전용 export**다. Rust에선 `pub` 유지하되
  이름에 의도를 남기고 doc에 "테스트 관측용"임을 명시(오라클이 직접 호출한다).
- **K12 — 기존 셸 인용 헬퍼 셋 다 **재사용 금지**.**
  `suaegi-misc/src/powershell_argument.rs`의 두 함수는 **다른 Orca 모듈**(`powershell-native-argument.ts`)의
  포트로 의미가 다르다(`'` 두 배 + 백슬래시-런 처리). `quoteWindowsArg`/`escapeCmdSetValue`는
  그걸로 표현 불가.

## 3. 오라클 & 핀
**오라클 전량**: `setup-runner-command.test.ts` 6 assertion / `setup-agent-sequencing.test.ts` 전 케이스.
⚠ 단 `SAST:30-36`은 **다른 모듈**(`setup-agent-startup-policy.ts`)을 검증하므로 **이 크레이트에 넣지 않는다**.
정찰이 찾은 **외부 오라클 2개**도 핀으로 이식: `main/providers/windows-shell-args.test.ts:384-388`
(슬래시 드라이브 경로 + **인용 패리티** 단언, 이슈 #7236)과 `renderer/src/lib/setup-runner.test.ts:10-53`.

**추가 핀(오라클 침묵):** **K4 전량**(`escape_cmd_set_value`의 `"`/`%`/`!`/`^` 각각 + `"` 포함 경로의
전체 명령 스냅샷 = 주입 가능 출력을 의도적으로 고정); K3 `"` 포함 경로의 cmd 출력;
K2 bare/quoted 분기 경계(`-`·`:`·`/`·`.`는 bare, 공백·`'`는 quoted)와 `'` → `'\''`;
K6 timeout 0/음수/1 경계; K7 `:` 포함 nonce; K8 개행 포함 UNC 경로 → `/`;
K9 U+212A/U+017F가 WSL 호스트 검사에 **불일치**; K10 `'   '` 폴백·U+FEFF/U+0085 양방향;
K1 `'windows'` 플랫폼 + WSL UNC → `'posix'` 셸.

*mutation:* K2 bare 분기 제거·`'\''`를 `\'`로, K3 `""`를 `\"`로, K4 세 치환 각각 제거·"수정",
K5 `startupCommand`에 인용 추가, K6 `max(1,..)` 제거, K8 `(?s)` 추가, K9 `(?i)` 유니코드,
K10 `str::trim`·env 우선순위 반전, K1 두 enum 통합, K14 손코딩 술어로 교체.

## 4. 순서
단일 PR, **커밋 2개**로 나눈다(M1 `setup_runner_command` + 오라클 / M2 `setup_agent_sequencing` + 나머지) —
이스케이프 리뷰가 깨끗한 첫 diff를 갖도록.
불변식: **버전 표기 1.4.146-rc.0**(§0), 두 모듈 한 크레이트·`suaegi-path` 재사용(§1/K14),
두 enum 분리(K1), posix 인용 안전(K2), **windows 인용의 축자 보존 + 주입 핀**(K3/K4),
설계상 주입 지점 보존(K5), 타임아웃 모델(K6), nonce `:` 위험 문서화(K7), 개행 UNC(K8),
ASCII 폴딩(K9), env 힌트 폴백(K10), 테스트 전용 export(K11), 기존 헬퍼 재사용 금지(K12),
`suaegi-misc` 미변경(K13), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
