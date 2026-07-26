# Plan — standalone utils M3 (tailnet-address, github-repo-identity-key, remote-runtime-error)

조사: `docs/superpowers/research/2026-07-25-standalone-utils.md` (9모듈 정찰). 리드가 3개 소스 **전문 직접 재확인**.
이 문서가 구현 계약. 남은 SUBTLE 2개(#5 harness-injected-user-turns, #6 codex-auth-errors)는 **Codex 교차검증 진행 중** → 다음 마일스톤.

## 0. 대상
- `tailnet_address.rs` ← `tailnet-address.ts` (21L/18L) → **`suaegi-misc`**
- `remote_runtime_error.rs` ← `remote-runtime-client-error-classification.ts` (43L/39L) → **`suaegi-misc`**
- `github_identity.rs` ← `github-repository-identity-key.ts` (19L/14L) → **`suaegi-forge`**

## 1. 계약 결정

- **E1 — tailnet: `\d`는 ASCII 전용, 선행 0 허용, 오버플로는 reject.**
  `/^\d+$/`(`:8`)의 JS `\d`는 **ASCII `[0-9]`**. Rust regex `\d`는 Unicode Nd(아랍-인도 숫자 통과) → **regex 쓰지 말고**
  `!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())`로 하드코딩(`suaegi-misc`는 dep-free — regex 금지).
  **선행 0 허용**: JS `Number("0000000100")`=100 → Rust `parse::<u64>()`=Ok(100). **`part.len()<=3` 같은 지름길 금지**(선행 0 케이스를 깬다).
  **오버플로**: JS는 초장문 숫자→`Infinity`→`isInteger` false→`false`; Rust `parse::<u64>()`→`Err`→`false`. **결과 동일**(둘 다 거부).
  → `part.parse::<u64>().map_or(false, |v| v <= 255)`. `octet < 0`(`:14`)은 정규식이 `-`를 막아 **죽은 코드**(이식 생략 가능, 주석).
  `split('.')`은 4조각 정확히(`:3`) — Rust `split('.')`도 후행 빈 조각 보존이라 동일. **`split_terminator` 금지.**
  CGNAT 판정(`:20`): `octets[0]==100 && (64..=127).contains(&octets[1])`.
- **E2 — github identity: `""` falsy ×2, `js_trim`(Rust `trim` 아님), full-Unicode `to_lowercase`(ASCII 아님), owner/repo는 trim 안 함.**
  `isDefaultGitHubHost`(`:5`) = `!host?.trim() || host.trim().toLowerCase()=='github.com'`:
  `None`→true, `Some("")`/`Some("   ")`→trim 후 `""`=**JS falsy**→true, 그 외 trim+lower 비교.
  → `host.map(js_trim).is_none_or(str::is_empty) || host.map(|h| js_trim(h).to_lowercase()).as_deref()==Some("github.com")`.
  `githubRepoIdentityKey`(`:16-18`): slug=`owner.to_lowercase()/repo.to_lowercase()` — **owner/repo는 trim 안 함**(비대칭 보존,
  `" Acme "`→`" acme "`). host=`repo.host.map(|h| js_trim(h).to_lowercase())`. 반환은 `host &&
  !isDefaultGitHubHost(host)`일 때만 `{host}/{slug}`, 아니면 `slug` — **`host &&`의 `""` falsy 가드도 남길 것**.
  **`to_ascii_lowercase` 금지**(IDN 호스트 발산). `suaegi-git/src/remote_identity.rs:44-46`가 같은 판단의 선례.
- **E3 — `suaegi-forge`에 `suaegi-misc` 의존 추가(js_trim 재사용, 6번째 사본 방지).** `suaegi-misc`는 **의존 0개 순수
  leaf**라 사이클 없음. `Cargo.toml`에 `suaegi-misc = { path = "../suaegi-misc" }`.
  **`provider.rs:101`의 `self.host == "github.com"` 재배선은 이번 범위 아님** — 소비자 배선 = 사람눈(이전 마일스톤과 동일 원칙).
  모듈만 추가하고 기존 코드는 **건드리지 않는다**.
- **E4 — remote-runtime-error: code는 대소문자 구분 정확 일치 / message는 소문자화 substring (비대칭이 load-bearing).**
  `RECOVERABLE_CODES` 5개(`:4-8`) 정확 전사, `error.code &&`(`:25`)의 `""` falsy 스킵 보존.
  `RECOVERABLE_MESSAGE_FRAGMENTS` 8개(`:12-19`) 정확 전사(전부 소문자), `message.to_lowercase()`(**full Unicode**) 후 `contains`.
  **`toRemoteRuntimeClientErrorLike`(`:32-43`)는 이식 불가** — Rust에 `unknown` 없고 `String(error)`의 JS 의미
  (`"[object Object]"`/`"undefined"`)는 재현 불가. → 구조체 `RemoteRuntimeClientErrorLike { code: Option<String>,
  message: String }`(TS 타입 1:1)와 **`from_message(&str)` 생성자**만 제공하고, `unknown` 스니핑은 **호출자 책임으로 문서화**.
  오라클의 `toRemoteRuntimeClientErrorLike(new Error(msg))` 케이스는 `from_message(msg)`로 이식(의도 보존).

## 2. 마일스톤 M3 (단일 PR)
- `suaegi-misc/src/tailnet_address.rs`: `is_tailnet_ipv4_address(&str)->bool` (E1).
- `suaegi-misc/src/remote_runtime_error.rs`: `RemoteRuntimeClientErrorLike`(+`from_message`),
  `is_recoverable_remote_runtime_connection_error(&…)->bool` (E4).
- `suaegi-forge/src/github_identity.rs`: `is_default_github_host(Option<&str>)->bool`,
  `github_repo_identity_key(owner,repo,host: Option<&str>)->String` (E2), + `js_ws` 로컬 미사용·misc 의존(E3).
- 각 `lib.rs` 선언/re-export + `suaegi-misc` 크레이트 doc 모듈 목록 갱신.

**오라클(3 테스트 전부):** tailnet 8케이스(경계 `100.63.255.255`/`100.128.0.1`, IPv6, 3조각);
github 3어서션(`' GitHub.com '` trim+ci, 기본호스트 생략, GHES `ghe.example:8443` **포트 보존**);
remote-error 4블록(code 화이트리스트 4 + 닫힌 화이트리스트 2 + Error→message 5).

**추가 핀(오라클 공백):** E1 선행 0(`100.0064.0.1`→true)·빈 조각·`>255`·초장문 숫자·**Unicode 숫자 거부(`١٠٠`)**·후행 점;
E2 `None`/`""`/`"   "` host·U+FEFF 패딩(js_trim)·비-ASCII 케이스(`GHE.ÉXAMPLE`→full lower)·owner/repo **trim 안 함**;
E4 미검증 code `timeout`·미검증 프래그먼트 3개·code `""` 스킵·대소문자 구분(code `TIMEOUT`→false)·message 대소문자 무시.

*mutation:* ASCII digit→`char::is_numeric`(Unicode Nd), 선행0 지름길, `<=255`→`<255`, CGNAT 경계 64/127,
`js_trim`→`str::trim`(U+FEFF), `to_lowercase`→`to_ascii_lowercase`(É), owner/repo에 trim 추가, `""` falsy 가드 제거,
code 비교를 대소문자 무시로, 프래그먼트 누락.

## 3. Deferred
- **#5·#6 SUBTLE 쌍** — Codex 교차검증 결과 반영 후 다음 마일스톤.
- `provider.rs:101` 재배선, 소비자 배선 전반 = 사람눈.
- `toRemoteRuntimeClientErrorLike`의 `unknown` 스니핑(재현 불가, 문서화).

## 4. 순서
단일 PR. 불변식: ASCII-digit 하드코딩+선행0(E1), `""` falsy·js_trim·full-lower·owner/repo 비대칭(E2),
forge→misc 의존·배선 금지(E3), code/message 비대칭(E4), **regex 의존 추가 금지**, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-impl-model-sonnet]], [[suaegi-workflow]]
