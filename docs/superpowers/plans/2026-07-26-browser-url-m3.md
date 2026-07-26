# Plan — browser-url M3: 네비게이션 결정 트리 + 경로→file URL + 외부 게이트 (모듈 완성)

조사: Explore 정찰 + 리드 실측(`O:11-13, 126-140, 242-333`과 오라클 잔여 블록 직접 통독).
M1 = PR #88(15 테스트), M2 = PR #89(40 테스트). **이 PR로 `browser-url.ts` 전체가 포팅 완료된다.**

**이 계층은 보안 경계다** — 무엇이 "허용"으로 떨어지는지가 전부다.

## 1. 범위
`O:11-13, 126-140, 242-333` → `crates/suaegi-browser-url/src/navigation.rs`(신규). **의존 추가 없음.**
`WINDOWS_ABSOLUTE_PATH_PATTERN`/`WINDOWS_UNC_PATH_PATTERN`/`UNIX_ABSOLUTE_PATH_PATTERN`(손코딩),
`absolute_path_to_file_url`(private), `windows_unc_path_to_file_url`(private),
`normalize_browser_navigation_url`, `normalize_external_browser_url`, `resolve_remote_failure_external_url`.
`ORCA_BROWSER_BLANK_URL` 상수도 여기서 노출(`constants.ts:64` = `"data:text/html,"`).

## 2. 계약 결정

- **D1 — ⚠ `searchEngine`은 **3-state**다. `Option`으로 뭉개면 보안 결함이 된다.**
  `O:303` `searchEngine !== undefined`가 검색 활성화를 정하고, `O:316` `searchEngine ?? DEFAULT`가
  `null`을 기본 엔진으로 채운다. 즉 **`undefined` = 검색 끔**(메인 프로세스 URL 검증 경로),
  **`null` = 검색 켬 + 기본 엔진**(주소창), **`Some(engine)` = 검색 켬 + 지정 엔진**.
  `Option<SearchEngine>` 하나로 접으면 `normalize_browser_navigation_url("not a url")`이 `None` 대신
  구글 검색 URL을 반환한다 → **네비게이션 검증기가 검색엔진으로 가는 오픈 리다이렉트가 된다**.
  → 전용 3-variant enum(예: `SearchFallback::Disabled | DefaultEngine | Engine(SearchEngine)`).
  오라클이 `T:177`(생략 → `null`)과 `T:191`(명시적 `null` → 구글 검색)로 **정확히 이 구분만** 못박는다.
- **D2 — ⚠ `javascript:`·`data:`는 `new URL`을 **성공적으로 통과**하고 **allow-list에서** 거부된다.**
  `O:293-297`은 **성공 경로에서** `null`을 반환한다. 허용 스킴은 정확히 `http:`/`https:`/`file:` 셋.
  ⚠ **`Url::parse(t).map(...).unwrap_or_else(|_| search_fallback(t))` 식으로 재구성 금지** —
  "http/https/file이 아님"을 "검색 폴백 시도"로 바꾸면 `javascript:alert(1)`이
  `https://javascript:alert(1)` 승격 경로나 검색 URL로 흘러간다. 현재 코드는 비-웹 스킴이
  **`O:298`의 catch에 절대 도달하지 못함**을 보장한다. 이 구조를 축자 보존한다.
- **D3 — 경로→file URL 두 함수는 **URL 파서를 절대 거치지 않는다**(문자열 연결).**
  결과: UNC 호스트가 **소문자화되지 않고 퍼센트 인코딩되지도 않는다** —
  `\\SERVER\share\x` → `file://SERVER/share/x`. 파서로 왕복시키면 `file://server/...`가 되어 발산한다.
  ⚠ **`Url::from_file_path` 금지**(정규화·상대경로 거부·플랫폼 의존). 반환 타입은 `String`이지 `Url`이 아니다.
  - `absolute_path_to_file_url`(`O:242-253`): `\`→`/` 전체 치환 → `/`로 split →
    **인덱스 0이 `^[A-Za-z]:$`면 그대로**, 아니면 각 세그먼트 `encodeURIComponent` →
    원본이 `/`로 시작하면 `file://` + join, 아니면 `file:///` + join.
    (`/a/b` → split이 선두 빈 세그먼트를 남기고 `encodeURIComponent("")==""` → `file:///a/b`.)
  - `windows_unc_path_to_file_url`(`O:255-259`): `\`→`/` 치환 후 **선두 `/`들을 정규식으로 제거** →
    첫 세그먼트가 host(**인코딩 안 함**), 나머지만 `encodeURIComponent` → `file://{host}/{...}`.
- **D4 — 여기 쓰이는 `encodeURIComponent`는 M2의 그것과 **같은 함수**다**(비이스케이프
  `A-Za-z0-9 - _ . ! ~ * ' ( )`). `search.rs`의 private 헬퍼를 `pub(crate)`로 올려 **재사용**한다
  (M2에서 이미 mutation 검증됨). 오라클 `T:156-166`이 공백·`#`·`&`·`%`·`!`·`^`를 정확히 못박는다
  — `!`는 **살아남고** `^`는 `%5E`가 된다.
- **D5 — `normalize_external_browser_url`은 스킴이 아니라 **문자열 접두사** `"file:"`를 본다**(`O:329`).
  오늘 안전한 이유는 모든 생산자가 소문자 `file://`를 내기 때문이다. `Url::parse` 후 `scheme()`으로
  "개선"하면 D3의 미파싱 UNC 문자열을 재파싱하게 되어 호스트가 소문자화된다 → **축자 보존**.
  게이트 순서: `null`이거나 blank sentinel → `null`; `file:` 접두 → `null`; 그 외 통과.
  순 허용: **`http:`/`https:`만**(+ D7 승격분).
- **D6 — blank sentinel은 **파스 이전에** 정확한 문자열 동등성으로 걸린다**(`O:267-269`).
  `trimmed`가 빈 문자열 / `"about:blank"` / `ORCA_BROWSER_BLANK_URL`(`"data:text/html,"`)이면
  즉시 sentinel 반환. **대소문자 구분** — `"ABOUT:BLANK"`는 해당 없음.
  ⚠ 이 단락 덕분에 `data:text/html,`만이 유일하게 허용되는 `data:` URI다(D2의 allow-list는 `data:`를 거부한다).
- **D7 — `O:305`의 `https://{trimmed}` 승격이 경계에서 가장 넓은 부분이다.**
  `new URL` 실패 후, `https://` 접두가 파싱되고 **검색이 꺼져 있거나** `looks_like_search_query`가
  false면 그 URL을 **그대로 반환**한다. 검색이 꺼진 경로에서는 `looks_like_search_query`가 **아예 호출되지 않는다**
  → `normalize_browser_navigation_url("singleword", Disabled)` → `"https://singleword/"`(오라클 `T:181`).
  축자 보존하되 doc 주석에 경계 폭을 명시.
- **D8 — 검사 순서가 계약이다.** ① js_trim + sentinel ② `classify_scheme_less_local_dev_address`
  ③ UNC 패턴 ④ UNIX-절대 **또는** Windows-드라이브 절대 ⑤ `new URL` + allow-list ⑥ catch 폴백.
  ⚠ ③④가 ⑤보다 **먼저**라 `C:\...`·`\\srv\sh\..`·`/a/b`는 파서에 **도달하지 않는다**.
  ⑤에서 `new URL`이 성공하면 폴백은 **절대 안 돈다**(D2).
- **D9 — `resolve_remote_failure_external_url`은 `parsed`가 아니라 **원문을 다시** 넘긴다**(`O:139`).
  그리고 그 호출은 **try 밖**이다. 파스 실패 → `null`(`O:136-138`); 호스트가 wildcard이거나
  loopback-적격이면 `null`; 그 외에는 `normalize_external_browser_url(raw_url)`.
  `file:///etc/passwd`는 hostname이 `""`라 두 술어를 다 통과해 `O:139`로 내려가고, 거기서 D5의 `file:` 규칙에
  걸려 `null`이 된다(오라클이 못박음). 술어 두 개는 M1에 이미 있다.
- **D10 — 패턴 3개는 손코딩이고 `\s`가 또 함정이다.**
  `O:12` UNC `^\\\\[^\s\\/]+[\\/][^\\/]+(?:[\\/].*)?$` — `[^\s...]`가 **ECMAScript 공백 집합**이다
  → `suaegi_misc::is_js_whitespace`. `O:11` `^[A-Za-z]:[\\/].*$`, `O:13` `^\/.*$` —
  둘 다 `s` 플래그가 없어 **`.*`가 개행을 안 넘는다**.
- **D11 — trim은 `js_trim`**(`O:266`). sentinel 동등성 판정이 여기 달려 있다.

## 3. 오라클 & 핀

**오라클(M3 해당 = 잔여 전량):** `T:17-26` scheme-less 로컬(쿼리·해시 포함); `T:99-118`
`resolveRemoteFailureExternalUrl` 7 loopback/wildcard null + 공개 2 통과 + `file://`·쓰레기 null;
`T:120-124` 후행 슬래시·`''`·`about:blank`; `T:126-129` `javascript:` → null, 외부 `about:blank` → null;
`T:135-139` in-app `file:` 허용; `T:141-154` UNIX·드라이브·UNC 2종 → file URL;
`T:156-166` 공백/`#`/`&`/`%`/`!`/`^` 인코딩; `T:171-174` 외부에서 `file:`·UNC 거부;
`T:176-178` 검색 꺼짐 → null; `T:180-182` `singleword` → `https://singleword/`;
`T:184-194` 명시적 `null` → 구글 `%20`; `T:196-206` 엔진별 템플릿; `T:208-213` `example.com`·
`github.com/org/repo`는 URL 취급; **그리고 M2에서 이월된 `T:234-237`**(Kagi 세션 + 네비게이션, `+` 인코딩).

**추가 핀(오라클 침묵):** D1 3-state 전량(생략 vs `null` vs 명시 엔진, `"not a url"`로 세 답이 갈리게);
D2 `data:text/plain,x`·`mailto:`·`ftp:` → **null**(검색 폴백으로 안 감)·`javascript:`가 검색 켜짐에서도 null;
D3 `\\SERVER\share\x`의 호스트가 **대문자 유지**·UNC 호스트 **미인코딩**·`C:/a`(슬래시)도 드라이브 취급;
D5 `file:` 접두 판정이 문자열 기반임(대문자 `FILE:`는 어떻게 되는지 실측 후 핀);
D6 `"ABOUT:BLANK"`는 sentinel 아님·`"data:text/html,"` 정확 일치만 허용;
D7 `user:pass@evil.com`·`evil.com:8080`이 승격됨(경계 폭 문서화 핀);
D8 순서(`C:\x`가 파서에 안 감, `/a/b`가 UNIX 분기로 감);
D10 UNC 패턴의 U+FEFF·개행 거부; D11 U+FEFF 패딩 `about:blank`가 sentinel이 됨.

*mutation:* D1 3-state를 `Option`으로, D2 파스 실패→검색폴백 재구성·allow-list에서 `file:` 추가/제거,
D3 `Url::from_file_path`·UNC 호스트 소문자화·호스트 인코딩·`file://` vs `file:///` 교환,
D4 unreserved 집합 교체, D5 `scheme()` 기반으로, D6 sentinel 검사 제거·대소문자 무시,
D7 승격 제거·검색 꺼짐에서도 `looks_like_search_query` 호출, D8 검사 순서 뒤바꾸기,
D9 `parsed`를 넘기기·술어 하나 제거, D10 `\s`를 Rust로, D11 `str::trim`.

## 4. 순서
M3 단일 PR. 불변식: **3-state 유지**(D1), **비-웹 스킴은 폴백에 도달 불가**(D2), 파서 미경유 경로 변환(D3),
인코더 재사용(D4), 문자열 접두 게이트(D5), 파스 이전 sentinel(D6), 승격 폭 문서화(D7), 검사 순서(D8),
원문 재전달(D9), 손코딩 패턴 + JS 공백(D10), `js_trim`(D11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[suaegi-impl-model-sonnet]]
