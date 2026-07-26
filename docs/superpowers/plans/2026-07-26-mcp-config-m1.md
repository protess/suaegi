# Plan — mcp-config M1: 경로/후보 계층 (신규 `suaegi-mcp` 크레이트, 의존 0)

조사: Explore 정찰(`mcp-config.ts` 275L + `.test.ts` 179L 통독, 소비자·크레이트 헌장 확인).
**소스에 import가 단 하나도 없다**(타입 import조차). 유일한 런타임 의존 요인은 `JSON.parse`(`O:118`) 하나뿐 →
**그 부분만 M2로 분리하면 M1은 의존 0으로 성립한다.**

## 0. 크레이트 배치 — 신규 leaf `suaegi-mcp`
- **기존 크레이트에 자연스러운 자리가 없다.** `suaegi-misc`는 **의존 0 헌장**(serde조차 없음)이라 M2의 `serde_json`을
  못 받고, `suaegi-path`도 **zero-dependency by design** 헌장이며, `suaegi-secrets`는 OS keyring 백엔드 계층이다.
- **리포의 확립된 패턴이 "Orca shared 모듈 1개 = leaf 크레이트 1개"**다(`suaegi-fuzzy`/`-taskquery`/`-workref`/
  `-gen-prompt` 전부 단일 모듈 포트; workref는 regex+url 두 의존을 가진 단일 모듈). 275L은 그보다 작지 않다.
- 형제 MCP 모듈이 생길 가능성은 낮다(정찰: `linear-mcp-issue-list.ts`는 **타입 전용 41L**, 나머지는 IO/UI) —
  그래도 격리 이점(신뢰 불가 입력 파서 + 시크릿 마스킹의 mutation 검증)이 단독 크레이트를 정당화한다.
- **M1은 `[dependencies]`를 비운 채로 만든다.** M2에서 `serde_json`만 추가.

## 1. 계약 결정 (M1 범위)

- **V1 — `suaegi-path` 헬퍼 **재사용 금지**. 셋 다 "거의 맞지만 정확히 틀리다"(정찰 §7.3).**
  - `is_windows_absolute_path_like`(`cross_platform_path.rs:77`)는 UNC에서 **2-컴포넌트 요구가 없다** →
    `\\server`(share 없음)·`//`·`///a/b`를 true로 판정 → `can_inspect_local_mcp_config_root`가 **Orca가 허용하는
    경로를 거부**한다.
  - `get_runtime_path_basename`(`:256`)은 **후행 슬래시를 트림**하고 `\`와 `/`를 **둘 다** 분할 →
    `"a/"`에서 Orca는 `""`, 이건 `"a"`.
  - `normalize_runtime_path_separators`(`:87`)는 `/+`를 collapse하고 UNC를 복원 → `O:159`의 단순 치환과 다르다.
  - **`std::path::Path::{parent,file_name}`도 금지**(플랫폼 고정 + `.`/`..` 정규화 + 후행 슬래시).
  → `relative_parent_dir`/`relative_basename`을 **축자 손코딩**: `\`→`/` **단순 전체 치환** 후 `rfind('/')`;
    `None`이면 부모는 `""`, basename은 **치환된 전체 문자열**; 있으면 `[..sep]` / `[sep+1..]`.
- **V2 — 후보 테이블은 순서가 계약이고, `format`은 **고유키가 아니다**.**
  4개 원소 순서(`O:36-61`): ① `workspace`/`Workspace`/`.mcp.json` ② `cursor`/`Cursor`/`.cursor/mcp.json`
  ③ `claude`/`Claude`/`.claude.json` ④ `claude`/`Claude workspace`/`.claude/mcp.json`.
  **`claude`가 두 번 나온다** → `format`을 HashMap 키로 쓰면 후보 하나가 사라진다. **동일성은 `relative_path`**
  (렌더러도 그걸로 Set을 만든다). `servers_path`는 넷 다 `["mcpServers"]` — "세 포맷"은 **경로 차이일 뿐 스키마 차이가 아니다**.
- **V3 — 부모 디렉터리 dedup은 **삽입 순서 보존**(JS `Set`).**
  `['', '.cursor', '', '.claude']` → 빈 문자열 제거 → dedup → **`['.cursor', '.claude']`**.
  ⚠ **`BTreeSet`/정렬 금지** — 정렬하면 `['.claude', '.cursor']`가 되어 오라클 `T:145`가 즉시 깨진다.
  `Vec` + `contains` 또는 순서 보존 셋으로.
- **V4 — `can_inspect_local_mcp_config_root`는 손코딩.** `is_windows_host`면 **무조건 true**(단락).
  아니면 아래 둘 중 하나에 걸릴 때만 false:
  ① **드라이브**: `[A-Za-z]` + `:` + 구분자(`\` 또는 `/`) 1개 → **`C:` 단독은 불일치**(= inspectable).
  ② **UNC**: 구분자 **정확히 2개** + 비구분자 1자 이상 + 구분자 1개 + 비구분자 1자 이상 →
     **`\\server`(share 없음)는 불일치**, **`///a/b`도 불일치**(`{2}` 뒤 `[^\\/]+`가 세 번째 `/`에서 실패, 백트래킹 불가).
  둘 다 **문자열 시작 앵커만**(`/m` 없음). 정규식 크레이트 없이 바이트 검사로 충분.
- **V5 — 이 모듈은 **어디서도 trim하지 않는다**.** `O:` 전체에 `.trim()` 0회. 경로·이름 어디에도 넣지 말 것(핀).
- **V6 — `select_existing_mcp_config_candidates`는 후보 순서를 보존**(`filter`)하고 **존재하는 것을 전부** 반환한다.
  단일 승자 선택·병합·오버라이드 로직은 **없다**(호출자가 전 후보를 순회하며 없는 건 `missing`으로 표시).
  매칭: `entries.get(parent_dir)` 없으면 빈 슬라이스로 간주 → `name == basename && !is_directory` **정확 일치**
  (대소문자 구분, 유니코드 정규화 없음). 엔트리 순서는 결과에 무관.
- **V7 — `MCP_STARTER_CONFIG`는 **후행 개행 포함** 정확히 `"{\n  \"mcpServers\": {}\n}\n"`.**
  (M1에서는 상수만 노출, 유효성 검사는 M2 오라클 `T:136-142`.)

## 2. 마일스톤 M1 (단일 PR)
신규 크레이트 `crates/suaegi-mcp` (**`[dependencies]` 비움**), 워크스페이스 members에 추가.
`src/lib.rs` + `src/config.rs`(또는 lib 직접): 타입 `McpConfigFormat`(3), `McpConfigCandidate`,
`McpConfigDirectoryEntry`; 상수 `MCP_CONFIG_CANDIDATES`(4, 순서 보존), `MCP_STARTER_CONFIG`;
함수 `get_mcp_config_parent_dirs`, `get_mcp_config_candidate_parent_dir`,
`select_existing_mcp_config_candidates`, `can_inspect_local_mcp_config_root`;
private `relative_parent_dir`, `relative_basename`.
**M2로 연기:** `inspect_mcp_config_content`, `mask_mcp_env`, `summarize_mcp_server`, `read_command`/`read_url`/
`resolve_transport`, `extract_object_at_path`, 그리고 `McpServerTransport`/`McpServerStatus`/`McpServerSummary`/
`McpConfigInspection` 타입 — 전부 JSON 계층이다.

**오라클(M1 해당분 = 테스트 8·9, `T:144-178`):** `get_mcp_config_parent_dirs()` → `['.cursor','.claude']`
(**삽입 순서**); 후보별 부모 → `['', '.cursor', '', '.claude']`; `select_existing` → `['Workspace','Cursor']`
(루트에 `.claude`가 **디렉터리가 아닌 파일**로 존재해도 후보③은 basename 불일치, 후보④는 `.claude` 맵 엔트리 부재);
`can_inspect_local_mcp_config_root` 6케이스(`C:\repo`→false, UNC 백슬래시→false, UNC 슬래시→false,
`/Users/me/repo`→true, 앞 둘이 `is_windows_host=true`면 →true).

**추가 핀(오라클 침묵):** V1 `"a/"`의 basename이 **`""`**(suaegi-path와 다름)·구분자 없는 경로의 basename=전체·
`\`만 쓰는 경로; V3 정렬 아님을 증명(`.cursor`가 `.claude`보다 **앞**); V4 `C:` 단독→true, `\\server`→true,
`///a/b`→true, `C:/`(슬래시)→false; V5 공백 패딩 경로가 trim되지 않음; V2 `claude` 포맷이 **2회** 등장하고
`relative_path`가 서로 다름; V6 엔트리 순서 무관·`is_directory=true`면 불일치.

*mutation:* V1 suaegi-path 헬퍼로 교체·후행 슬래시 트림 추가, V3 정렬 셋 사용, V4 UNC 2-컴포넌트 요구 제거·
드라이브 구분자 요구 제거·`is_windows_host` 단락 제거, V5 trim 추가, V6 순서 미보존·`is_directory` 무시,
V2 후보 순서 변경·`format` 키로 dedup.

## 3. Deferred → M2 (JSON 계층)
정찰의 열린 질문 대부분이 M2 소관이다:
- **Q1 `JSON.parse` 에러 메시지**: 오라클 `T:27`이 `contains "JSON"`을 요구하는데 **serde_json 메시지엔 "JSON"이 없다**
  (`"EOF while parsing..."`) → 접두사 합성 필요. 파일 내용 미노출 계약과의 균형 결정.
- **Q2 키 순서**: `Object.entries` 순서가 **출력 배열 순서**이고 오라클이 엄격 검사한다.
  ⚠ **`serde_json`의 `preserve_order` 피처는 절대 켜지 말 것** — cargo feature unification으로 **워크스페이스 전역**에
  전파되어 suaegi-{core,keys,tracker,git,forge,term,search,gen-prompt}의 직렬화 순서를 조용히 바꾼다(확정 사항).
  → 문서 순서를 보존하는 수동 수집으로 해결.
- Q3 `(?i-u)` vs 리터럴 전개(민감 env 키 패턴), Q4 `raw.type`을 enum으로 승격 금지(미지 값 관대 폴스루),
  Q5 `""`의 falsy 처리, Q7 프로토타입 경로, T8 `enabled !== false` 엄격 비교, T16 `String()` 강제변환
  (`{}`→`"[object Object]"`, `[1,2]`→`"1,2"` — `to_string()`과 다름).

## 4. 순서
M1 단일 PR. 불변식: 신규 크레이트 의존 0(V0), suaegi-path/std::path 재사용 금지·축자 손코딩(V1),
후보 순서·`relative_path` 동일성(V2), **삽입 순서 dedup**(V3), 손코딩 Windows-root 술어(V4), **trim 없음**(V5),
전부 반환·순서 보존(V6), 상수 바이트 정확(V7), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]]
