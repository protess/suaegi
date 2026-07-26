# Plan — mcp-config M2: JSON 계층 (`suaegi-mcp`에 serde_json 추가)

조사: Explore 정찰(`mcp-config.ts` 275L + `.test.ts` 179L 통독, 소비자 2개, **모든 JS 의미론을 node v20.20.2에서 실측**).
M1(경로/후보 계층)은 PR #85로 머지 완료(`crates/suaegi-mcp/src/lib.rs`, 17 테스트, 의존 0).

## 0. 분할 — 2 PR

정찰은 단일 PR을 권고했으나 **W4/W5(ECMAScript `String()`·`Number::toString`)가 정찰 추정보다 크다**.
유일하게 방어 가능한 seam으로 자른다 — 그 seam은 **자체 오라클이 있다**(`T:122-134`가 `maskMcpEnv`를 직접 호출하는
suite 내 유일한 테스트).

- **M2a — 값 계층**: 순서 보존 JSON 값 + `js_string_of` + ECMAScript 수치 포맷 + 민감 패턴 2개 + `mask_mcp_env`.
  오라클 테스트 6(`T:122-134`). `parse` 진입점을 pub으로 노출해 dead code 0.
- **M2b — 요약 계층**: `extract_object_at_path`, `read_command`/`read_url`/`resolve_transport`,
  `summarize_mcp_server`, `inspect_mcp_config_content`. 나머지 오라클 6개(`T:16-22, 24-29, 31-76, 78-93, 95-120, 136-142`).

## 1. 의존

`serde_json.workspace = true` + `serde`(**derive 불필요**). 그게 전부다.
- ⚠ **`preserve_order` 피처 절대 금지** — feature unification으로 워크스페이스 전역 직렬화 순서를 바꾼다.
  리포 선례: `suaegi-keys/src/file.rs:41-46`이 같은 이유로 거부.
- **`regex` 추가 안 함.** 패턴 2개 전부 손코딩(W6) — 손코딩하면 `/i`-without-`/u` 의미론이 **자동으로** 맞는다.
  M1의 `can_inspect_local_mcp_config_root` 선례와 동일.
- **JSON 파서 손코딩 금지.** `serde_json` 사용은 `suaegi-gen-prompt/src/pull_request_generation.rs:5-11`의
  기존 승인 근거를 그대로 원용한다.

## 2. M2a 계약 결정

- **W1 — 순서 보존 JSON 값 타입을 직접 만든다.**
  `serde_json::Value`의 `Map`은 `BTreeMap`이라 `{filesystem, docs, old, broken}`을
  `{broken, docs, filesystem, old}`로 재정렬 → 오라클 `T:49-75`가 **즉시 깨진다**.
  → `enum JsonValue { Null, Bool, Number, String, Array(Vec<_>), Object(Vec<(String, JsonValue)>) }`에
  **손으로 쓴 `Deserialize`**(`serde::de::Visitor` + `MapAccess`). `#[derive]` 쓰지 않는다.
  `serde_json` 파서는 `preserve_order`와 무관하게 **문서 순서로** 맵 엔트리를 넘겨주므로 visitor가 그대로 회수한다.
- **W2 — 문서 순서 ≠ JS 열거 순서. ⚠ `Vec` 삽입 순서만으로는 틀린다.**
  JS 자체 프로퍼티 열거는 **정규 배열 인덱스 키를 먼저, 오름차순 수치로** 뽑고 그 다음 문자열 키를 삽입 순서로 뽑는다.
  실측: `{"zebra":1,"2":2,"alpha":3,"10":4,"1":5,"-1":6,"1.5":7,"01":8,"4294967295":9,"4294967294":10}`
  → `["1","2","10","4294967294","zebra","alpha","-1","1.5","01","4294967295"]`.
  인덱스 키 = **선행 0 없는 정규 10진수이며 `[0, 2^32-2]` 범위**. `"-1"`·`"1.5"`·`"01"`·`"4294967295"`는 **아님**.
  → `Object` 구성 시 두 버킷으로 나눠 재배치한다. `servers`(`O:138`)와 `env`(`O:148`) **양쪽 다** 해당.
  오라클 침묵(테스트 서버명이 전부 비수치) → **핀 필수**.
- **W3 — 중복 키는 "첫 위치, 마지막 값".** 실측 `{"a":1,"b":2,"a":3}` → 키 `["a","b"]`, `a === 3`.
  serde `MapAccess`는 **두 엔트리를 모두** 넘겨주므로 명시 구현 필요(순진한 `Vec::push`는 행이 중복된다).
- **W4 — `js_string_of`: JSON 값에 대한 ECMAScript `String()` 축자 구현.**
  `Value::to_string()`은 **전부 틀리다**. 실측 대응표:
  `null`→`"null"`, `true`→`"true"`, `{"x":1}`→**`"[object Object]"`**, `[1,2]`→**`"1,2"`**, `[]`→`""`,
  `[[1,2],[3]]`→**`"1,2,3"`**(재귀 join), `[null,null,1]`→**`",,1"`**(null 원소는 빈 문자열), `[{}]`→`"[object Object]"`.
- **W5 — 수치는 ECMAScript `Number::toString`.** Rust `f64::to_string()`은 지수 표기를 절대 안 쓴다 → 발산.
  실측: `1e21`→**`"1e+21"`**(Rust `"1000000000000000000000"`), `1e-7`→**`"1e-7"`**(Rust `"0.0000001"`),
  `-0`→**`"0"`**(Rust `"-0"`), `1.2345678901234568e29`→`"1.2345678901234568e+29"`, `5e-324`→`"5e-324"`.
  임계: `1e-7 <= |x| < 1e21`이면 10진, 밖이면 지수(`e+`/`e-`, **부호 항상**, 지수 자릿수 패딩 없음).
  구현: `format!("{:e}", x)`로 **최단 왕복 가수+지수**를 얻어 ECMA-262 §6.1.6.1.20 규칙으로 재조립.
  `serde_json`이 `1`을 `u64`, `1.0`을 `f64`로 타이핑하지만 **둘 다 `"1"`**이어야 한다.
- **W6 — 민감 패턴 2개는 손코딩. 두 패턴의 플래그가 다르다 — 절대 통일하지 말 것.**
  - 키(`O:103-104`): `/(api[_-]?key|auth|bearer|cookie|credential|password|private[_-]?key|secret|session|token)/i`
    — **`i`만**. 앵커 없음(부분문자열). ⚠ **`/i` without `/u`는 비-ASCII를 ASCII로 접지 않는다**
    ([[js-lowercase-two-mechanisms]]). 실측: `ſecret`(U+017F)·`TOKEN`(U+212A KELVIN) → **불일치**.
    Rust `regex`의 `(?i)`는 유니코드 인식이라 **매치해버린다** → `to_ascii_lowercase` + `contains`로 손코딩.
    실측 경계: `MY_AUTHORITY_X`→매치(`auth`), `API KEY`(공백)→**불일치**, `APIIKEY`→**불일치**.
  - 값(`O:105-106`): `/(sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{12,}|xox[baprs]-[A-Za-z0-9-]{12,})/`
    — **플래그 0개**(대소문자 **구분**). 실측: `SK-abcdefghijkl`→**불일치**, `sk-12345678901`(11)→불일치,
    `sk-123456789012`(12)→매치, `ghx_...`→불일치, `prefix sk-abcdefghijkl suffix`→매치(비앵커).
    ⚠ 여기에 `(?i)`를 넣으면 Orca가 노출하는 값을 마스킹한다.
  - 둘 다 **`/g` 없음** → `lastIndex` 상태 없음. "고쳐" 넣지 말 것.
- **W7 — 마스크는 고정 8×U+2022(`'••••••••'`), 키는 원형 보존, 조건은 `키 OR 강제변환된 값`.**
  `O:151`이 **`String()` 강제변환 후의** 문자열에 값 패턴을 적용한다 → `{"K":["sk-abcdefghijkl"]}`도 **마스킹된다**
  (`Value::String` 변형만 검사하면 놓친다). 길이·접두사 보존 **없음**(8 char / 8 UTF-16 unit / **24 UTF-8 바이트**).
- **W8 — `mask_mcp_env` 가드: 객체만 통과, 빈 객체는 `Some(empty)`.**
  `O:143-145` `!env || typeof env !== 'object' || Array.isArray(env)` → `null`·배열·문자열·수치 → `None`.
  ⚠ **`{}` → `Some(빈 맵)`이지 `None`이 아니다** — `McpConfigFileRow.tsx:102`의 렌더 조건이 이걸 본다.
  빈 맵을 `None`으로 접는 순진한 `Option` 처리 금지.
- **W9 — `env`도 순서 보존 컨테이너**(`Vec<(String,String)>`). `McpConfigFileRow.tsx:105-107`이
  `Object.entries(...).join(', ')`로 렌더한다. 오라클은 객체 `toEqual`이라 **침묵** → 핀 필수.

## 3. M2b 계약 결정

- **X1 — `extract_object_at_path`는 평범한 `get`. 프로토타입 하드닝 금지, 그리고 실패는 `valid`다.**
  정찰이 실측으로 확인: `__proto__`는 `JSON.parse`가 **자체 데이터 프로퍼티**로 만들고, 부재 시 읽히는
  `Object.prototype`은 `Object.entries`가 `[]`를 주며, `constructor`/`toString`은 **함수**라 `typeof` 가드에서 탈락 →
  **셋 다 Rust `Map::get`과 출력이 동일**하다. 하드닝하면 오히려 발산한다(주석 + 핀).
  ⚠ `null` 반환 시 `O:130-132`는 **`status:'valid'`, `servers:[]`** — 직관과 반대로 `invalid`가 **아니다**.
  `mcpServers` 부재·`null`·배열·문자열 전부 valid. 오라클 침묵.
- **X2 — `resolve_transport`: `type`을 enum으로 승격 금지, `url` 우선.**
  `O:263-275`는 `'http'`/`'remote'`/`'local'` **3개 리터럴만** `===` 비교하고 그 외(`'sse'`, `'HTTP'`, `'stdio'`,
  수치, `null`, 부재)는 **전부 무시하고 존재 추론으로 폴스루**한다. `#[serde(other)] Unknown` enum은 발산한다.
  ⚠ **`url`이 첫 `if`에 있다** → `{command, url}` → **`http`**, `{type:'local', url}` → **`http`**. 오라클 침묵.
- **X3 — `read_command`는 `[0]`만, `read_url`은 배열 형태 없음. 비대칭은 의도다.**
  `O:247-249`: `[1,"npx"]` → **`undefined`**(첫 문자열을 찾지 **않는다**), `[]` → `undefined`, `[""]` → `""`.
  `O:253-261`: `url` > `httpUrl`, **배열 미지원**. 두 리더를 통합하지 말 것.
- **X4 — `!url`/`!command`는 **falsy**이지 부재가 아니다.**
  `{"type":"http","url":""}` → **`invalid` / `'Missing URL.'`**. `is_none()` 포트는 발산.
  해당 지점 4곳: `O:213`, `O:223`, `O:268`, `O:271`. `command: [""]`도 동일. 오라클 침묵.
- **X5 — `raw.enabled !== false && raw.disabled !== true`는 **엄격 비교**.**
  `enabled: 0`·`null`·`"false"` → **여전히 enabled**. `disabled: 1`·`"yes"` → **여전히 enabled**.
  truthiness 헬퍼(`is_truthy`)를 쓰면 전부 뒤집힌다.
  → `!matches!(get("enabled"), Some(Bool(false))) && !matches!(get("disabled"), Some(Bool(true)))`.
- **X6 — invalid 4분기가 enabled/disabled보다 **먼저** 결정된다.**
  `O:200`에서 계산하지만 `O:236`에서야 소비한다 → `{"enabled":false}` + command 없음 → `disabled`가 아니라
  **`invalid` / `'Missing command.'`**. 오라클 침묵.
- **X7 — 입력은 `Option<&str>`. `Some("")`은 missing이 **아니다**.**
  `O:112`는 `=== null` 검사 → `''`는 `JSON.parse('')`로 흘러가 `invalid`가 된다.
  실제 호출자가 이 경로를 쓴다(`mcp-config-inspection.ts:78`이 바이너리 파일에 `''`를 넘긴다).
- **X8 — 에러 메시지는 **의도적 발산**: `"JSON"`을 포함하되 입력 바이트는 **절대** 넣지 않는다.**
  오라클 `T:27`은 `result.error`에 `toContain('JSON')`을 요구하는데 `serde_json` 메시지엔 `"JSON"`이 **없다**
  (`"EOF while parsing an object at line 1 column 1"`).
  ⚠ 그렇다고 V8을 축자 재현해서도 **안 된다** — V8은 설정 내용을 최대 ~20자 흘린다(실측:
  `{"apiKey":"sk-live-SUPERSECRET-abcdef","x":@}` → `Unexpected token '@', ..."cdef","x":@}" is not valid JSON`).
  테스트 이름(`'reports invalid JSON without exposing file contents'`, `T:24`)이 **Orca에서는 지켜지지 않는다**.
  → `format!("Invalid JSON at line {} column {}", e.line(), e.column())`. 오라클을 통과하면서 의도도 만족한다.
  이 문자열은 사용자에게 보인다(`McpConfigFileRow.tsx:82`) → doc 주석에 승인된 발산으로 명시.
- **X9 — `serde_json`이 `JSON.parse`보다 엄격한 3가지를 승인된 발산으로 문서화.**
  실측: lone surrogate(`"\uD800"`), `1e999`(JS는 `Infinity`), 중첩 깊이 ≥128(V8은 200+ 허용).
  각각 파일 전체를 `valid` → `invalid`로 뒤집는다. `pull_request_generation.rs:26-31`의 기존 문구를 원용.
- **X10 — `.trim()` 0회, `args`는 완전히 무시.**
  `O:186-241`은 `args`를 **읽지 않는다**(`T:38`이 `args`를 주고 `T:50-56` 기대값엔 흔적이 없다).
  `command: "  npx  "`는 그대로 `"  npx  "`이고 truthy. `command`에 접어 넣지 말 것.
- **X11 — `McpServerStatus`(3) ≠ `McpConfigInspection.status`(3). 이름이 겹치지만 다른 타입이다.**
  전자 `enabled|disabled|invalid`(`O:16`), 후자 `missing|valid|invalid`(`O:31`).
  invalid 사유 4종의 문자열: `'Server entry must be an object.'`(엔트리가 객체 아님, **env 키 자체가 없음**),
  `'Missing command or URL.'`, `'Missing URL.'`, `'Missing command.'`.
  ⚠ 첫 번째 문자열은 **오라클에서 한 번도 검증되지 않는다** → 핀 필수.

## 4. 오라클 & 핀

**오라클(JSON 계층 7건):** `T:16-22` missing / `T:24-29` invalid JSON / `T:31-76` **4서버 문서 순서 + env 마스킹 +
`args` 무시 + `enabled:false`→disabled + `unknown`** / `T:78-93` `command` 배열 + `httpUrl` / `T:95-120`
`Missing URL.`·`Missing command.` / `T:122-134` `mask_mcp_env` 직접 / `T:136-142` starter config → valid.

**추가 핀(오라클 침묵 — 이 모듈은 구멍이 크다):** W2 정수형 키 호이스팅; W3 중복 키; W4 `String()` 전량
(`[object Object]`·`"1,2"`·`",,1"`·`"1,2,3"`·`""`); W5 수치 임계 5종; W6 `ſ`/U+212A **불일치**·값 패턴
대문자 불일치·`{12,}` 경계 양쪽·`gh[pousr]_`·`xox[baprs]-`; W7 배열 값이 강제변환 후 마스킹됨; W8 `{}`→`Some(empty)`·
비객체→`None`; W9 env 순서; X1 `mcpServers` 부재/`null`/배열/문자열 → **valid**·`__proto__`·`constructor`;
X2 `'remote'`·미지 `type`·비문자열 `type`·`{command,url}`→http·`{type:'local',url}`→http;
X3 `[1,"npx"]`·`[]`·`{"url":["x"]}`; X4 빈 문자열 `url`/`command`/`[""]`; X5 `enabled:0`·`null`·`"false"`·
`disabled:1`·`"yes"`; X6 `enabled:false` + command 없음 → invalid; X7 `Some("")`; X8 에러에 내용 부재;
X10 미trim·`args` 무시; X11 `'Server entry must be an object.'` 리터럴.

*mutation:* W1 `serde_json::Value` 사용, W2 호이스팅 제거, W3 중복 키 양쪽 push, W4 `to_string()` 사용,
W5 임계 제거·`-0`, W6 `(?i)`로 교체·값 패턴에 `(?i)` 추가·`{12,}`를 `{11,}`로, W7 `Value::String`만 검사,
W8 빈 맵을 `None`으로, X1 실패를 `invalid`로·프로토타입 denylist 추가, X2 `type`을 enum으로·`command` 우선,
X3 배열 스캔·리더 통합, X4 `is_none()`으로, X5 `as_bool().unwrap_or`·truthiness, X6 분기 순서 뒤집기,
X7 `""`를 missing으로, X8 `serde_json` 메시지 그대로.

## 5. 순서
M2a → M2b, 각각 단일 PR + mutation 스윕.
불변식: `preserve_order` 금지·순서 보존 값 타입(W1/W2/W3), ECMAScript 강제변환(W4/W5), 패턴 2개 플래그 분리(W6),
마스크 조건·형태(W7/W8/W9), 관대 폴스루·falsy 의미론(X1~X6), `Option<&str>`(X7), 에러 발산 문서화(X8/X9),
미trim(X10), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[js-lowercase-two-mechanisms]], [[suaegi-impl-model-sonnet]]
