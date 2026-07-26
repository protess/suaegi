# Plan — browser-url M1: 로컬 개발 주소 + 인증서 호스트 (신규 `suaegi-browser-url` 크레이트)

조사: Explore 정찰(`browser-url.ts` 333L + `.test.ts` 271L 통독, `url` 크레이트 소스 실측 확인,
크레이트 헌장 6개 대조). 총 3 PR로 분할.

## 0. 크레이트 배치 — 신규 leaf `suaegi-browser-url`

- **`suaegi-misc`/`suaegi-path`**: 둘 다 **의존 0 헌장** → `url`을 못 받는다. 기각.
- **`suaegi-forge`**: `url`이 있지만 **git-forge 도메인**이고 `reqwest`/`tokio`/`suaegi-git`/`suaegi-secrets`까지
  끌고 온다. 인앱 브라우저 주소창을 forge 크레이트 뒤에 두는 건 리포가 이미 지적한 레이어링 스멜. 기각.
- **`suaegi-search`**: 헌장이 **콘텐츠 검색(rg/git-grep)** — 웹 검색엔진 URL과 무관한 동음이의어. 기각.
- → **신규 leaf.** 리포 확립 패턴("Orca shared 모듈 1개 = leaf 크레이트 1개")과 일치.

```toml
[dependencies]
url = { workspace = true }
suaegi-misc = { path = "../suaegi-misc" }   # js_trim / is_js_whitespace
```
**`regex` 추가 안 함** — 패턴 4개 전부 `(?i-u:)` + `[0-9]` + JS-공백 클래스 오버라이드가 필요해
정규식으로 쓰면 이스케이프 수프가 된다. 손코딩이 B3의 혼동 위험을 원천 제거한다.
선례: `suaegi-path`, `suaegi-taskquery`, `suaegi-mcp` 전부 손코딩.

## 1. M1 범위
`browser-url.ts:3-4, 37-125` — **125행 아래를 단 한 줄도 참조하지 않는 완전 자족 클러스터**(정찰 Seam 1).
`LOCAL_ADDRESS_PATTERN`, `classifySchemeLessLocalDevAddress`, `normalizeCertificateHostname`,
`isValidDnsName`, `isIpv4Loopback`, `isEligibleLocalCertificateHost`, `isWildcardBindHost`,
`toHttpsRecoveryUrl`, `toSecureCertificateEndpoint`.
**M1에서 제외**: `resolveRemoteFailureExternalUrl`(`:130-140`) — 유일한 A→B 결합으로
`normalizeExternalBrowserUrl`을 호출한다. 그 호출 대상이 착지하는 M3로 미룬다.

## 2. 계약 결정

- **B1 — `.host`/`.origin`/`.href`는 이 파일 어디에도 없다. 따라서 `port_or_known_default()`는
  **정당한 용도가 0이다**.**
  `.port` 읽기는 두 곳뿐: `O:121` `parsed.port || '443'` → `port().map(|p| p.to_string()).unwrap_or("443")`,
  `O:162` `parsed.port !== ''` → `port().is_some()`.
  ⚠ `port_or_known_default()`는 `https`에서 **우연히** 맞아 보이지만 WHATWG가 지운 기본 포트를 되살린다.
  선례: `hosted_review_gitlab.rs:31-33`, `repo_icon.rs:56-60`.
- **B2 — 케이스 폴딩 메커니즘이 **한 파일에 둘** 있다. 절대 통일하지 말 것**([[js-lowercase-two-mechanisms]]).
  - `O:50`, `:149`, `:181` `.toLowerCase()` = **full Unicode** → Rust `str::to_lowercase()`. U+212A → `k`.
  - `O:4`, `:10` 정규식 `/i` **without `/u`** = **ASCII만 접는다** → U+212A는 `k`로 **안** 접히고
    U+017F(ſ)는 `s`로 **안** 접힌다. Rust `(?i)`는 유니코드 인식이라 **둘 다 매치해버린다**.
  M1에 해당하는 건 `O:50`(toLowerCase)과 `O:4`(`/i`) — **양방향 핀 필수**.
- **B3 — `\d`는 JS에서 ASCII, Rust에서 유니코드 `Nd`.**
  `O:4`(`127(?:\.\d{1,3}){3}`, `(?::\d+)?`)와 `O:69`(`^\d{1,3}$`) 전부 `[0-9]`로 강제.
  `١٢٧.٠.٠.١`가 loopback으로 통과하면 안 된다. 선례: `tailnet_address.rs:11-16`.
- **B4 — trim 3곳은 전부 `suaegi_misc::js_trim`.** `O:38`(`rawInput`), `O:50`(`hostname`).
  Rust `str::trim`은 U+FEFF를 **안** 지우고 U+0085를 **지운다** — JS는 정반대.
  `js_trim("\u{FEFF}localhost:3000")`이 `LOCAL_ADDRESS_PATTERN`에 걸려야 한다.
- **B5 — `normalizeCertificateHostname`은 **순서가 계약**이고 각 스트립은 **정확히 1회**.**
  `O:49-53`: ① js_trim ② `to_lowercase()` ③ `[`+`]`가 **둘 다** 있을 때만 한 쌍 제거 ④ 후행 `.` 하나 제거.
  ③이 ④보다 **먼저**라서 `'[::1].'` → `'[::1]'`(대괄호 안 벗겨짐) → **부적격**. `'[::1]'` → `'::1'` → 적격.
  순서를 바꾸거나 dot-strip을 루프로 만들면 답이 달라진다.
  ⚠ `O:51`의 `slice(1,-1)`은 UTF-16 슬라이스다 → Rust에선 **`strip_prefix('[')`+`strip_suffix(']')`**.
  `&s[1..s.len()-1]`은 다바이트 끝문자에서 **패닉**한다.
- **B6 — `isIpv4Loopback`의 선행 0 거부는 `octets[i] === String(value)` 재직렬화 비교다.**
  `O:67-77`: `split('.')`가 정확히 4조각(**JS split은 후행 빈 조각을 남긴다** → `'127.0.0.1.'`은 5조각 → 거부),
  각 조각이 `^[0-9]{1,3}$`, `values[0] == 127`, 전부 `0..=255`, 그리고 **재직렬화 일치**.
  `'127.00.0.1'` → `String(0)=="0" != "00"` → **false**(오라클 `T:67`).
  ⚠ Rust `split_terminator` 금지 — 후행 빈 조각을 지워 `'127.0.0.1.'`을 통과시킨다.
  ⚠ **`127.0.0.0/8` 전체가 적격**이지 `127.0.0.1`만이 아니다(오라클 `T:` `127.255.255.255` → true).
- **B7 — `isValidDnsName`: 전체 길이 캡은 **UTF-16 code unit**, 라벨 정규식엔 `/i`가 **없다**.**
  `O:56` `length === 0 || length > 253`, `O:62` 라벨 `1..=63`, `O:63` `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$`
  — **플래그 0개**(앞선 `to_lowercase()`에 의존). 선행/후행 하이픈과 빈 라벨 금지.
  캡은 `encode_utf16().count()`로(여기선 관측상 동일하지만 무위험 포트).
- **B8 — `*.local`은 **어디서도 처리되지 않는다**. `*.localhost`는 **깊이 무관**으로 처리된다.**
  `O:87` `normalized === 'localhost' || normalized.endsWith('.localhost')`.
  `foo.local`은 유효한 DNS 이름이지만 둘 다 아니라 **거부**. "친절하게" 추가하지 말 것.
  `'0.0.0.0'`은 **loopback 부적격**이지만 **wildcard**다(`O:90-93`, 오라클이 양쪽 다 못박음).
- **B9 — `classifySchemeLessLocalDevAddress`의 정규식은 **일부러 호스트 문법보다 헐겁다**.**
  `O:4`의 `\d{1,3}`가 `127.0.0.01`을 허용하고, 실제 검증은 전부 `new URL`(`O:43`)에 위임된다
  (WHATWG가 8진수 IPv4 규칙을 적용해 `127.0.0.1`로 정규화, `>65535` 포트 거부, 선행 0 제거, `:0` 허용).
  ⚠ 대조: `isEligibleLocalCertificateHost('127.00.0.1')`는 **원문 경로**라 파서를 안 거치고 **false**다(B6).
  **두 동작이 공존해야 한다** — 한쪽 규칙을 다른 쪽에 적용하면 둘 다 깨진다.
- **B10 — `Url::set_scheme`은 `Result`를 돌려주고 **조용히 no-op 할 수 있다**.**
  `O:101`의 JS 대입은 무조건 성공이다. `let _ = set_scheme(...)`로 결과를 버리면 향후 도달 가능해질 때
  **틀린 URL을 성공으로 반환**한다 → 결과를 반드시 확인하고 실패 시 `None`.
  덤으로 `url` 크레이트는 scheme 교체 후 `set_port(previous)`를 재실행해 기본 포트를 **다시 지운다**
  (정찰이 `url-2.5.8/src/lib.rs:2503-2508`에서 실측) → 오라클 `T:80`
  `'http://localhost:80/path'` → `'https://localhost/path'`가 Rust에서도 성립한다.
- **B11 — `\s`는 ECMAScript 집합이다**(M1에서는 `O:10`이 M2 소관이라 직접 해당은 없지만,
  `O:4`의 `.*`는 `s` 플래그가 없어 **개행을 넘지 않는다**). `"localhost:3000/\nfoo"`는 패턴 **실패** →
  `new URL`로 떨어져 scheme `localhost:`가 되어 `null`. 손코딩 스캐너가 이 두 거부 경로를
  실수로 "허용"으로 합치지 않도록 핀.

## 3. 오라클 & 핀

**오라클(M1 해당 = 블록 2·3·4·5, `T:28-97`, ~40 assertion):**
`classifySchemeLessLocalDevAddress` 수용 5종(`localhost:3000/path`, `127.0.0.1:5173`, `0.0.0.0:8080`,
`[::1]:3000`, `[2001:db8::1]:3000`) + 거부 `app.localhost:3000`;
`isEligibleLocalCertificateHost` **true 8종**(`localhost`, `LOCALHOST.`, `app.localhost`,
`deep.app.localhost.`, `127.0.0.1`, `127.255.255.255`, `::1`, `[::1]`) /
**false 11종**(`0.0.0.0`, `::`, `[2001:db8::1]`, `192.168.1.1`, `localhost.example.com`, `notlocalhost`,
`.localhost`, `-bad.localhost`, `bad-.localhost`, `127.0.0.999`, `127.00.0.1`);
`toHttpsRecoveryUrl` 경로/쿼리/해시/userinfo 보존 + **기본 포트 소거**(`T:80`) + null 4종;
`toSecureCertificateEndpoint` 경로·자격증명 제거, `wss:`→`https:`, IPv6 재괄호, `:443` 기본 + null 2종.

**추가 핀(오라클 침묵):** B2 양방향(U+212A가 `to_lowercase` 경로에선 접히고 `/i` 경로에선 **안** 접힘);
B3 아랍-인도 숫자 거부; B4 U+FEFF 패딩이 js_trim으로 제거됨·U+0085는 **제거 안 됨**;
B5 `'[::1].'`→부적격 및 스트립 1회성; B6 `'127.0.0.1.'`(후행 점) 거부·`127.0.0.0/8` 상단 경계;
B7 253/63 경계 양쪽·빈 라벨; B8 `foo.local` 거부·`a.b.c.localhost` 허용·`0.0.0.0`의 loopback×wildcard○;
B9 `127.0.0.01`이 classify에선 **통과**(→`127.0.0.1`)하고 cert-host에선 **거부**(둘 다 핀);
B10 `set_scheme` 결과 확인·`:80` 소거; B11 개행 tail 거부.

*mutation:* B1 `port_or_known_default` 사용, B2 두 메커니즘 뒤바꿈(양방향), B3 `\d`를 유니코드로,
B4 `str::trim`으로, B5 스트립 순서 교환·dot 루프, B6 `split_terminator`·재직렬화 비교 제거·`127`만 허용,
B7 캡 off-by-one·라벨 정규식에 `/i` 추가, B8 `.local` 추가·`endsWith`를 `==`로, B9 classify에 B6 규칙 적용,
B10 `set_scheme` 결과 무시.

## 4. 후속 (M2/M3)
- **M2 — 검색엔진 + Kagi 세션/리댁션**(`O:6-35, 142-240`). **비밀이 사는 곳**이라 독립 리뷰 표면을 준다.
  핵심 함정: `query_pairs_mut()`가 쿼리가 비면 **맨 `?`를 남긴다**(JS는 query를 null로) →
  `if url.query() == Some("") { set_query(None) }` 필요, **오라클 커버리지 0**;
  `searchParams.set/delete`는 **위치와 다른 파라미터를 보존**한다(clear+append 금지);
  `encodeURIComponent`(공백→`%20`)와 `URLSearchParams`(공백→`+`)가 **같은 함수 안에 공존**;
  `redactKagiSessionToken`은 **infallible passthrough**(`-> String`), 실패 시 입력을 **토큰째 그대로** 반환;
  `.hash`는 **건드리지 않는다** → `?token=A#token=B`는 리댁션 "성공" 후에도 토큰이 살아남는다(축자 보존 + 문서화).
- **M3 — 네비게이션 결정 트리 + 경로→file URL + 외부 게이트**(`O:11-13, 130-140, 242-333`).
  핵심 함정: `searchEngine`이 **3-state**(`undefined`=검색 끔 / `null`=기본 엔진으로 켬) — `Option`으로
  뭉개면 네비게이션 검증기가 **오픈 리다이렉트**가 된다; `javascript:`/`data:`는 `new URL`을 **통과**하고
  **allow-list에서** 거부되므로 파스 실패를 검색 폴백으로 돌리는 재구성 금지;
  `absolutePathToFileUrl`/`windowsUncPathToFileUrl`은 **파서를 안 거친다**(UNC 호스트가 소문자화되지 않음) →
  `Url::from_file_path` 금지.

## 5. 순서
M1 → M2 → M3, 각각 단일 PR + mutation 스윕.
불변식: 신규 leaf·`regex` 없음(§0), `port_or_known_default` 금지(B1), **두 폴딩 메커니즘 분리**(B2),
ASCII `\d`(B3), `js_trim`(B4), 정규화 순서·1회 스트립(B5), 재직렬화 선행0 거부·후행 빈 조각(B6),
UTF-16 캡·라벨 대소문자 구분(B7), `.local` 미처리(B8), **헐거운 정규식+파서 위임과 원문 경로의 공존**(B9),
`set_scheme` 결과 확인(B10), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[js-lowercase-two-mechanisms]], [[suaegi-impl-model-sonnet]]
