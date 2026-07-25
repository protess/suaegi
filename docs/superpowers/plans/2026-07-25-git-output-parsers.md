# Plan — git 출력 파서 클러스터 (cquoted/remote-identity/merge-tree-cap/status-limit) 확정

조사: `docs/superpowers/research/2026-07-25-git-output-parsers.md` (Orca @ v1.4.150-rc.0). Codex 교차검증 판정
**VALIDATED-WITH-CORRECTIONS** (5모듈 전 테스트·트랩 CONFIRMED, 정정 2). **이 문서가 계약이며 조사를 supersede**.
리드가 4모듈 소스 전부 직접 재확인(cquoted 옥탈 루프 `:43-70`, remote `:16-102`, merge-tree `:1-27`,
status-limit `:6-28`).

## 0. 결정
5모듈 중 **순수 4개 이식**(→ `suaegi-git` 신규 top-level 모듈), **capability-cache는 defer**(stateful async
single-flight 인프라 — suaegi-git 러너/async 소유권 확정 후, Codex 권고 Q4).
- `cquoted_path.rs` ← git-cquoted-path.ts
- `remote_identity.rs` ← git-remote-identity.ts
- `merge_tree_capability.rs` ← git-merge-tree-capability.ts
- `status_limit.rs` ← git-status-limit.ts

deps 추가: `regex = { workspace=true }`, `url = { workspace=true }`(둘 다 워크스페이스 기존).

## 1. Codex 정정/결정 (구현자 필독)
- **E1 — 옥탈 도메인은 0–511, u8 아님. `u16::from_str_radix(_,8)` 후 `& 0xff`로 mod-256 narrow.** Orca는 최대 3자리
  옥탈 `\000`–`\777`(=511)을 받아 `parseInt(octal,8)`(`:57`) 후 `Uint8Array.from`이 **mod 256 narrow**(`\777`→511→255).
  `u8::from_str_radix("777",8)`는 **실패**(511>255) → 발산. → `u16` 파싱 후 `(v & 0xff) as u8`. **핀: `\777`→byte 0xFF→
  `from_utf8_lossy`→U+FFFD, `\101`→'A'.**
- **E2 — remote tie-break은 "첫 named"가 아니라 이름 정렬.** `deriveGitRemoteIdentity`(`:91-94`)는 priority(upstream=0/
  origin=1/기타=2) 정렬 후 동점은 `localeCompare`(`:93`). Rust는 **`str::cmp`(코드포인트, 의도적 non-locale 계약,
  문서화)** — 테스트가 동점 2개를 안 비교하므로 통과. `sort_by(|a,b| pri_a.cmp(&pri_b).then(a.name.cmp(&b.name)))`(안정).
- **cquoted 출력 = 손실 `String`**(Codex Q1). byte 버퍼 누적(리터럴 바이트+단문자 escape 바이트+옥탈 narrow 바이트)
  후 **`String::from_utf8_lossy` 한 번**. JS는 옥탈 런마다 `TextDecoder`하지만, UTF-8 self-sync + 리터럴 char의 첫
  바이트는 continuation(0x80-0xBF) 불가 → 전체 버퍼 1회 lossy 디코드와 **동치**. `Vec<u8>`/OsString byte-exact API는
  defer. byte-native 순회(escape 관련 바이트 전부 ASCII → `&str.as_bytes()` 인덱스 루프 panic-free).
- **remote-URL: `://` 있으면 `url` 크레이트, 없으면 scp regex.**(Codex Q3) `url::Url::parse` → `host_str()`(포트 제외,
  JS `hostname` 동형)·`path()`(JS `pathname` 동형). scp regex `^([^@\s:]+@)?([^:\s]+):(.+)$`(Rust `\s`=Unicode=JS 동형).
  정규화 순서: trim → Windows-FS(`^[A-Za-z]:[\\/]`) 거부 → `://`분기 → host `trim().to_lowercase()`(full Unicode
  lowercase, JS `.toLowerCase()` 동형; **NOT** to_ascii) → path 앞뒤 `/` strip **후** `.git` strip(대소문자 보존) →
  pathname 퍼센트-디코드 금지. host·path 둘 다 non-empty여야 `host/path`, else None.
- **merge-tree = 에러 텍스트 매칭**(버전 아님). `GitCommandError{message,stderr,stdout: Option<&str>}` 구조체로
  모델(JS getGitErrorText의 object 필드 브랜치 — Error/string은 message로 매핑). 3+1 정규식 `(?i)` +유니코드 `\s`,
  `` [`']? ``(backtick·apostrophe), `.test`=unanchored(=`is_match`). 정규식 verbatim(`:14,17,18,24`).
- **status-limit:** `resolve_git_status_limit(Option<f64>)->i64`(`is_finite && fract()==0 && >=0` else DEFAULT=1000).
  `cap_git_status_entries<T>(Vec<T>, i64, CapPrevious)->CapResult<T>`: `exceeded=limit>0 && len>limit`(0=무제한);
  미초과 AND `!prev.did_hit_limit`면 `{entries, did_hit_limit:false, status_length:None}`; else `{entries: 초과면
  take(limit), did_hit_limit:true, status_length: Some(max(prev.status_length??0, len))}`(원본 len, 끈적 보존).

## 2. 마일스톤
### M1 — git 파서 4모듈 (`suaegi-git` top-level, 단일)
- `cquoted_path.rs`: `decode_git_cquoted_path(&str)->String`(E1 옥탈 u16-narrow, 단문자 escape, 가드).
- `remote_identity.rs`: `GitRemoteIdentity`/`GitRemoteEntry`, `normalize_git_remote_url(&str)->Option<String>`,
  `parse_git_remote_verbose_output(&str)->Vec<GitRemoteEntry>`, `derive_git_remote_identity(&str)->Option<GitRemoteIdentity>`(E2 정렬).
- `merge_tree_capability.rs`: `GitCommandError`, `is_unsupported_merge_tree_write_tree_error`,
  `is_unsupported_merge_tree_merge_base_error`(3+1 `(?i)` 정규식).
- `status_limit.rs`: `DEFAULT_GIT_STATUS_LIMIT`, `resolve_git_status_limit`, `cap_git_status_entries<T>`, `CapPrevious`/`CapResult`.

**오라클(4 테스트 전부):** cquoted(BOM·인접 멀티바이트 옥탈); remote(https/scp/ssh/mixed-case→동일 canonical·포트
폐기·host 소문자+path 보존·Windows null·derive upstream>origin·단일 origin/mirror); merge-tree(3 write-tree 형태
+merge-base backtick=true·unrelated/failed=false); status-limit(resolve 정수 게이트·cap 초과/끈적/무제한).

**추가 핀:** E1 `\777`→U+FFFD·`\101`→A; E2 정렬(동점 2 'other' 이름 정렬 확인); remote 포트-drop·`.git`+슬래시 순서;
cquoted non-UTF8 byte lossy; status-limit `Number.isInteger` 부정(1.5/NaN/-1)·limit=0.

*mutation:* 옥탈 radix 8→10·narrow 제거, `.git` strip 순서/제거, host to_lowercase 제거, scp `://` 분기 반전,
priority 상수 스왑, tie-break 정렬 제거, `Number.isInteger` 게이트 제거, cap `limit>0`(무제한) 제거, 끈적 `did_hit_limit`.

## 3. Deferred
- **capability-cache**(stateful async single-flight, Codex Q4) — 러너/async 소유권 후 별도.
- 파서 소비자 배선(cquoted→diff/ls-files, remote-identity→origin 식별, merge-tree→러너 fallback, status→cap UI) = 사람눈.
- cquoted byte-exact `Vec<u8>`/OsString API, `localeCompare` locale-exact 정렬 = 문서화된 의도적 발산.

## 4. 순서
M1 단일(4모듈 + 오라클 4 + E1·E2 핀). 불변식: 옥탈 u16-narrow(E1), 이름 정렬 tie-break(E2), host-소문자/path-보존,
`url` 크레이트 `://`분기, 손실 String, 매 회귀 mutation 검증, capability-cache defer. 관련:
[[mutation-verify-regression-tests]], [[suaegi-workflow]], [[subagent-output-untrusted]]
