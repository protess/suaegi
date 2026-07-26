# Plan — browser-url M2: 검색 엔진 + Kagi 세션/리댁션 (비밀이 사는 계층)

조사: Explore 정찰 + 리드 실측(소스 `O:6-35, 142-240`과 오라클 `T:215-270` 직접 통독,
`url`/`form_urlencoded` 크레이트 동작 확인). M1은 PR #88로 머지 완료(15 테스트).

**이 PR을 따로 떼는 이유: 세션 토큰(베어러 자격증명)이 여기 산다.** 독립 리뷰 표면을 준다.

## 1. 범위
`browser-url.ts:10, 15-35, 142-240` → `crates/suaegi-browser-url/src/search.rs`(신규).
`LOOKS_LIKE_URL_PATTERN`(손코딩), `SearchEngine`(4), `SearchUrlOptions`, `SEARCH_ENGINE_LABELS`,
`SEARCH_ENGINE_URLS`(private), `DEFAULT_SEARCH_ENGINE`, `normalize_kagi_session_link`,
`redact_kagi_session_token`, `build_kagi_session_search_url`(private), `build_search_url`,
`looks_like_search_query`, private `encode_uri_component`.
**의존 추가 없음.** M3으로 미룸: `O:11-13, 130-140, 242-333`(네비게이션 트리, 경로→file URL, 외부 게이트).

## 2. 계약 결정

- **C1 — ⚠ `query_pairs_mut()`는 쿼리가 비면 **맨 `?`를 남긴다**. 오라클 커버리지 0.**
  `url` 크레이트는 진입 시 `'?'`를 무조건 밀어 넣고 `Drop`은 fragment만 복원한다 → 마지막 파라미터를
  지우면 `https://kagi.com/search?`가 남는다. WHATWG/JS는 직렬화가 비면 query를 **null**로 만들어
  `https://kagi.com/search`를 준다.
  도달 경로: `redact_kagi_session_token("https://kagi.com/search?token=secret")` — **token이 유일한
  파라미터일 때**. 오라클 3케이스는 전부 `q`가 함께 있어 **이 버그를 못 잡는다**.
  → 변경 후 반드시 `if url.query() == Some("") { url.set_query(None); }`. **핀 필수.**
- **C2 — `searchParams.set`/`delete`는 **위치와 다른 파라미터를 보존**한다.**
  `O:167-170`: `delete('q')` 후 `set('token', token)`. JS `set`은 **첫 번째 출현을 제자리에서 교체하고
  나머지 동명 항목을 제거**한다(오라클 `T:255-257` `?token=A&token=B` → `token=A`가 이걸 못박는다).
  `delete(name)`는 **모든** 동명 항목을 제거한다.
  ⚠ `clear()` + `append_pair` 는 **무관한 파라미터를 전부 파괴**하고, filter-then-append는 **token을
  맨 뒤로 옮긴다**. `?a=1&token=X&b=2` → `?a=1&token=X&b=2`가 유지되어야 한다. **오라클 커버리지 0**
  (유일한 다중 파라미터 케이스 `T:226`은 token이 맨 앞이라 순서 차이가 안 보인다). **핀 필수.**
- **C3 — 인코더가 **두 개** 있고 **같은 함수 안에서 갈린다**.**
  - `O:226` `encodeURIComponent(query)` — 공백 → **`%20`**, 비이스케이프 집합
    `A-Za-z0-9 - _ . ! ~ * ' ( )`. `new URL`을 **다시 안 거친다**(문자열 연결).
  - `O:211` `searchParams.set('q', query)` → `toString()` — 공백 → **`+`**, 비이스케이프 집합이
    `* - . 0-9 A-Z _ a-z`로 **줄어든다**(`! ' ( ) ~`가 퍼센트 인코딩됨).
  오라클이 **양쪽 다** 못박는다: `T:216` `hello%20world` vs `T:232` `hello+world`.
  → `%20` 경로는 **손코딩**(Path A), `+` 경로는 `url`의 `form_urlencoded` 직렬화(Path B).
  ⚠ `suaegi-forge`의 `repo_icon.rs` 인코더는 **다른 크레이트라 접근 불가이고, 그 파일 헤더가 교차 재사용을
  명시적으로 금지**한다 → **복사**하되 공유하지 말 것.
- **C4 — `redact_kagi_session_token`은 **infallible passthrough**: 시그니처는 `(&str) -> String`.**
  `O:196`이 실패 시 **입력을 그대로** 돌려준다. `Option`/`Result`로 만들면 호출자의 `unwrap_or_default()`가
  URL을 **빈 문자열로 날리거나** `?`가 표시 문자열을 **삭제**한다.
  ⚠ **그래서 토큰이 살아남는 경로가 넷 있다**(전부 오라클 침묵, 전부 축자 보존):
  `http:` 스킴, 다른 호스트, 다른 경로, 파스 실패 — 넷 다 `?token=SECRET`째로 반환된다.
- **C5 — `redact`는 `.hash`를 **절대 건드리지 않는다**. `normalize`는 **비운다**.**
  `O:190-191`은 쿼리 파라미터만 지운다 → `https://kagi.com/search?token=A#token=B`는
  **"성공적으로" 리댁션된 뒤에도 fragment에 토큰이 남는다**. `O:171`의 `parsed.hash = ''`는
  `normalize`에만 있다(오라클 `T:226`의 `#ignored`가 사라지는 것으로 확인).
  ⚠ **축자 보존하고 doc 주석에 명시**한다 — "하드닝"하면 오라클과 어긋난다. 상류 결함이지 우리 결정이 아니다.
- **C6 — `normalize_kagi_session_link`의 거부 조건 7개는 AND이고, `port !== ''`가 함정이다.**
  `O:156-166`: `protocol === 'https:'`, hostname ∈ {`kagi.com`, `www.kagi.com`}(**`to_lowercase()` 후**),
  pathname ∈ {`/search`, `/search/`}, `username === ''`, `password === ''`, `port === ''`, `token` truthy.
  ⚠ WHATWG가 기본 포트를 지우므로 **`https://kagi.com:443/search?token=x`는 통과한다**.
  Rust는 `port().is_some()` — **`port_or_known_default()`는 `Some(443)`을 돌려줘 잘못 거부한다**(B1과 동일 함정).
  오라클은 `:8443`만 테스트해 이 회귀가 **안 보인다**. **핀 필수.**
  ⚠ `password`: JS `.password`는 부재 시 `''`. Rust `password()`는 `Option<&str>` →
  `username().is_empty()`와 `password().is_none_or(str::is_empty)`로 맞춰 `http://user:@host/`를 정렬.
- **C7 — `looks_like_search_query`의 세 검사와 그 함정.**
  `O:229-240`: ① `input.includes(' ')` → true. **리터럴 U+0020만**이다 — 탭·NBSP는 해당 없음
  (`contains(char::is_whitespace)`로 쓰면 발산). ② `LOOKS_LIKE_URL_PATTERN` 매치 → false.
  ③ `.` 또는 `:` 포함 → false. ④ 그 외 true.
  `LOOKS_LIKE_URL_PATTERN` = `/^[^\s]+\.[a-z]{2,}(\/.*)?$/i` — ⚠ **세 겹 함정**:
  `/i`는 **`/u`가 없어 ASCII만 접는다**(U+017F·U+212A 불일치, Rust `(?i)`는 매치해버림, [[js-lowercase-two-mechanisms]]);
  `\s`는 **ECMAScript 집합**(U+FEFF 포함, U+0085 **제외** — Rust는 정반대) → `suaegi_misc::is_js_whitespace`;
  `.*`에 `s` 플래그가 없어 **개행을 안 넘는다**. → 손코딩.
  ⚠ 이 함수는 **export되어 있지만 테스트가 import조차 안 한다**. `:` 가지는 직·간접 커버리지가 **0**이다.
- **C8 — trim 2곳은 `js_trim`.** `O:143`(`rawLink`), `O:150`(`searchParams.get('token')?.trim()`).
  후자가 특히 중요하다: U+FEFF로 감싼 토큰은 JS에선 정상 토큰으로 다듬어지지만 Rust `str::trim`은
  BOM을 남겨 **링크 수락 여부와 Kagi에 보내는 값이 둘 다 달라진다**.
  ⚠ `searchParams.get`은 **퍼센트/`+` 디코딩된 값**을 준다(`+` → 공백). Rust도 `query_pairs()`가 디코딩한다.
- **C9 — `O:210`의 `new URL`은 **try 밖의 유일한 파스**다. 재파스 자체를 없앤다.**
  `build_kagi_session_search_url`이 `normalize`의 문자열 결과를 다시 파싱한다.
  Rust에서 `Url::parse(...).unwrap()`은 **패닉**이 되어 JS 예외와 실패 양상이 다르다.
  → 내부 변형 `normalize_kagi_session_url(&str) -> Option<Url>`을 만들고 공개 함수는 그걸
  `.map(|u| u.to_string())` 하도록 배선한다. 재파스가 사라져 문제가 소멸한다.
- **C10 — `!sessionLink`는 `''`도 잡는다.** `O:203`. `Option<&str>` + 빈 문자열 둘 다 → `None` 폴백.
  그리고 `build_search_url`의 kagi 분기는 **정규화 실패 시 평범한 템플릿으로 폴백**한다(`O:220-225`) —
  에러가 아니다.
- **C11 — `SEARCH_ENGINE_LABELS`는 **오라클 커버리지 0****(테스트가 참조조차 안 한다).
  `Google`/`DuckDuckGo`/`Bing`/`Kagi` — 대소문자까지 핀으로 직접 쓴다. `DEFAULT_SEARCH_ENGINE`도 직접 핀.

## 3. 오라클 & 핀

**오라클(M2 해당 = 블록 18·19·20·21·22·23):**
`T:215-223` `buildSearchUrl` google/duckduckgo/kagi **`%20`**(bing은 여기 **빠져 있다**);
`T:225-238` 세션 링크 정규화 + `%20`이 아닌 **`+`** 인코딩(**세 번째 assertion `T:234-237`은
`normalizeBrowserNavigationUrl`을 호출하므로 M3로 이월**);
`T:240-246` 거부 5종(token 없음 / `http:` / 다른 호스트 / userinfo / `:8443`);
`T:248-252` `/search/` 후행 슬래시 수락; `T:254-258` 중복 token → 첫 값; `T:260-270` 리댁션 3종.

**추가 핀(오라클 침묵 — 이 계층은 구멍이 크다):**
C1 **token이 유일 파라미터일 때 맨 `?`가 안 남는다**(양쪽 함수);
C2 `?a=1&token=X&b=2` 순서·타 파라미터 보존;
C3 `encode_uri_component`의 `!~*'()` 미이스케이프 및 `+`경로에서의 `!'()~` 이스케이프 대비;
C4 `http:`/다른 호스트/다른 경로/파스 실패 4종이 **토큰째 verbatim 반환**;
C5 `?token=A#token=B` → fragment의 토큰이 **살아남는다**(축자 보존 핀) / `normalize`는 fragment를 **버린다**;
C6 **`https://kagi.com:443/search?token=x` 수락**(기본 포트) 및 `www.kagi.com` 수락·대문자 호스트 수락;
C7 `includes(' ')`가 **탭에는 반응 안 함**·U+017F/U+212A가 `[a-z]{2,}`에 **불일치**·U+FEFF가 `[^\s]`에 **불일치**·
개행 tail 거부·**`:` 가지**(`localhost:3000` → false);
C8 U+FEFF 패딩 토큰이 다듬어짐·U+0085는 **안** 다듬어짐·`+`가 공백으로 디코딩됨;
C10 빈 문자열 세션 링크 → 평범한 템플릿 폴백·정규화 실패 시 폴백;
C11 라벨 4개·기본 엔진.

*mutation:* C1 `set_query(None)` 제거, C2 `clear()`+append·filter-then-append, C3 두 인코더 교환·
unreserved 집합을 RFC3986으로, C4 `-> Option<String>`으로·실패 시 빈 문자열, C5 `redact`에 hash 클리어 추가·
`normalize`에서 hash 클리어 제거, C6 `port_or_known_default`·조건 7개 중 하나 제거, C7 `/i`를 유니코드로·
`\s`를 Rust로·`includes(' ')`를 `is_whitespace`로·`:` 검사 제거, C8 `str::trim`으로, C10 폴백 제거.

## 4. 순서
M2 단일 PR. 불변식: **빈 쿼리에 `?` 미잔류**(C1), 파라미터 위치·이웃 보존(C2), **두 인코더 분리**(C3),
infallible passthrough(C4), **fragment 비대칭 축자 보존**(C5), 기본 포트 수락(C6), 손코딩 3중 함정 패턴(C7),
`js_trim`(C8), 재파스 제거(C9), 빈 문자열·실패 폴백(C10), 라벨 직접 핀(C11), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[js-lowercase-two-mechanisms]],
[[suaegi-impl-model-sonnet]]
