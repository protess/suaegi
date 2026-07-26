# Plan — github-links (M1 of 2; 신규 `suaegi-ghlink` 크레이트)

조사: Explore 정찰(`github-links.ts` 102L + `terminal-github-pr-link-detector.ts` 174L 통독,
오라클 3개 위치 확인). 출처 `reference/orca/` = **v1.4.146-rc.0**.

## 0. 분할 — 한 크레이트, 두 PR
- **M1(이번)**: `github_links.rs` (102L). 자체 오라클이 있다 — `renderer/src/lib/github-links.test.ts`가
  `export *` 재수출을 통해 **공유 함수 그대로** 검증한다(`:2-7`). 130L 분량이 이 모듈 몫.
- **M2(다음)**: `pr_link_detector.rs` (174L) + 오라클 17케이스 + parity 픽스처.
근거: 두 모듈은 한 크레이트가 맞지만(디텍터의 유일한 런타임 import가 `parseGitHubIssueOrPRLink`),
`github-links`는 **디텍터가 건드리지 않는 export가 둘**(`buildGitHubRepoUrl`,
`parseGitHubIssueOrPRNumber`) 있고 자체 오라클도 있어 독립 diff로 충분히 두껍다.

## 1. 배치 — 신규 leaf `suaegi-ghlink`
```toml
[dependencies]
url = { workspace = true }
suaegi-misc = { path = "../suaegi-misc" }   # js_trim
```
- **`suaegi-term`에 두지 않는다**: 이 모듈은 `url`이 필요하고 forge 도메인 어휘를 끌고 온다.
- **`suaegi-forge`에도 두지 않는다**: 그러면 M2의 디텍터가 `suaegi-term → suaegi-forge` 간선을 만들어
  tokio·reqwest·chrono를 터미널 크레이트 트리에 끌어들인다. leaf로 두면 나중에 forge가 **이걸** 의존하면 된다.
- 선례: `suaegi-workref`가 거의 같은 모양(URL/work-item 참조 파서, suaegi 의존 0, `url` 사용).
- **`regex` 금지**(P1/P2). 패턴이 `/`로 쪼개면 끝나는 4-토큰 경로라 손코딩이 더 짧고,
  정규식을 쓰면 아래 두 함정을 **정확히** 밟는다.

## 2. 계약 결정

- **P1 — ⚠⚠ `GH_ITEM_PATH_RE`의 `/i`에는 `/u`가 **없다**. 그리고 오라클이 그 플래그를 **전혀** 검증하지 않는다.**
  JS 비-유니코드 모드의 `Canonicalize`는 코드포인트 ≥128을 ASCII로 접지 **않는다** →
  `ſ`(U+017F)는 `s`와 매치되지 **않는다**. Rust `(?i)`는 유니코드 단순 폴딩이라 **매치한다**.
  즉 `https://github.com/o/r/iſſueſ/42`가 JS에선 `null`, 순진한 Rust 포트에선 **유효한 이슈 링크**가 된다.
  ⚠ 어떤 오라클도 라우트 세그먼트의 대소문자 변형(`PULL`/`Issues`)을 쓰지 **않는다** →
  **플래그의 존재 이유 자체가 미고정**이라 이 발산은 조용히 출하된다.
  → **`eq_ignore_ascii_case`로 손코딩**한다. `(?i)` 금지.
- **P2 — `\d`는 JS에서 **항상 `[0-9]`**, Rust에선 유니코드 `Nd`다.**
  `٤٢`·`４２`가 통과하면 안 된다. `is_ascii_digit`으로.
- **P3 — ⚠ 호스트를 **한 번도 검사하지 않는다**. 이건 GitHub 매처가 아니라 **경로 모양 매처**다.**
  `https://git.corp.com/MyOrg/my_repo/pull/395`가 **수락**된다(오라클이 못박음).
  모듈 이름이 오해를 부른다 — 호스트 검사를 "보강"하지 말 것.
- **P4 — 두 함수의 차이는 **bare-number 빠른 경로의 유무**가 전부다.**
  `parseGitHubIssueOrPRNumber`는 `42`·`#42`를 받고, `parseGitHubIssueOrPRLink`는 **받지 않는다**
  (`'42'` → `null`). 나머지 파이프라인(trim → `new URL` try/catch → http/https 게이트 → 경로 매치)은 동일.
- **P5 — `owner/repo#123` 형태는 **지원하지 않는다**.** `#`은 **선두 1개만** 벗겨지고
  (`##42`는 실패), 그 뒤는 `^[0-9]+$`가 아니면 URL 파싱으로 간다 → 스킴이 없어 throw → `null`.
- **P6 — ⚠ `parseInt`는 **실패하지 않는다**.** `…/pull/99999999999999999999` → `1e20`(> 0이라 통과),
  309자 이상 → `Infinity`도 통과해 `number` 필드에 들어간다.
  Rust `u64::from_str`은 **`Err`** → `None` → 링크 소실. **오라클 커버리지 0.**
  → `f64`(정확) 또는 saturating `u64`(실용) 중 **택일해 핀으로 고정**하고 근거를 주석에.
- **P7 — 숫자 게이트는 `parsed > 0`**이다(`:30`). `0`·`#0`·`/pull/0` → **`null`**.
  기수는 명시 10이라 8진수 없음 → `007` → **`7`**(URL 문자열은 `007` 그대로).
- **P8 — 후행 슬래시는 **전부** 벗긴다**(`/\/+$/`, 앵커됨). pathname이 `/`면 `''`이 되어
  `^/`가 실패 → `null`. `/923///` → `923`.
- **P9 — `url.pathname`을 쓴다**(원문이 아니라) → 쿼리·프래그먼트가 **구조적으로 제외**되고
  퍼센트 인코딩은 WHATWG 파서가 이미 적용한 상태다. `?diff=split`·`#issuecomment-1` 무해.
- **P10 — `owner`/`repo`는 **원문 그대로**(대소문자 보존) 반환된다.** `.toLowerCase()`는
  **라우트 세그먼트 판정에만** 쓰인다(`match[3].toLowerCase() === 'pull'`).
  `MyOrg`/`my_repo`가 그대로 나온다(오라클이 못박음).
- **P11 — trim 2곳은 ECMAScript 의미론** → `suaegi_misc::js_trim`(U+FEFF 포함, U+0085 제외).
- **P12 — `buildGitHubRepoUrl`은 **`https://github.com/`을 하드코딩**한다** — GHE를 **무시**한다.
  두 세그먼트에 `encodeURIComponent` 적용(`stably ai` → `stably%20ai`, `orca/tools` → `orca%2Ftools`).
  가드는 **falsy** 검사라 `owner: ''`/`repo: ''`도 `null`이다.
  ⚠ 인코더는 **모듈 로컬로 복사**한다(`suaegi-forge/src/repo_icon.rs:388` 선례 — 크로스 크레이트 재사용 금지).
- **P13 — `url` 크레이트 매핑**: `new URL(x)` → `Url::parse(x).ok()?`,
  `url.protocol !== 'https:'` → `scheme() != "https"`(크레이트가 이미 소문자화),
  `url.pathname` → `url.path()`.

## 3. 오라클 & 핀
**오라클**: `reference/orca/src/renderer/src/lib/github-links.test.ts` **의 `:10-130`만**
(`:132-183`의 `normalizeGitHubLinkQuery`는 **렌더러 전용 래퍼**라 이 포트 범위 밖 — 이식하지 말 것).

**추가 핀(오라클 침묵 — 정찰이 열거한 무커버 분기 전부):**
**P1 라우트 대소문자 4종**(`PULL`/`Pull`/`ISSUES`/`Issues` 수락) **+ `iſſueſ`/`ISSUEſ` 거부**(유니코드 폴딩 금지 증명);
P2 아랍-인도/전각 숫자 거부; P6 오버플로 2종(20자리·309자리)과 선택한 정책; P7 `007` → `7`이면서 URL은 `007`;
`try/catch` 무효 URL(스킴 없음·`::::`); 프로토콜 게이트(`ftp:`·`file:`·`javascript:`);
trim 후 빈 문자열; `buildGitHubRepoUrl`의 `None`/`Some{owner:""}`/`Some{repo:""}`;
P11 U+FEFF/U+0085 양방향; P12 인코딩 2종; P4 `parse_link("42") == None` vs `parse_number("42") == Some(42)`;
P5 `acme/orca#42` → `None`·`##42` → `None`.

*mutation:* P1 `(?i)`/유니코드 폴딩·라우트 소문자화 제거, P2 유니코드 숫자 허용, P3 호스트 검사 추가,
P4 링크 쪽에 bare-number 경로 추가, P6 `u64::from_str`로 무조건 `None`, P7 `>= 0`으로·기수 8,
P8 후행 슬래시 1개만 제거, P9 `pathname` 대신 원문, P10 owner/repo 소문자화, P11 `str::trim`,
P12 GHE 호스트 사용·인코딩 생략·`is_some()` 가드.

## 4. 순서
M1 단일 PR. M2(디텍터)는 별건 — 그쪽은 carry/dedupe/strip 순서가 위험 예산이고
정찰이 **`test:116-121`이 이름과 달리 아무것도 고정하지 못한다**는 것과
`endsWithHttpSchemePrefixFragment`가 **양성 커버리지 0**이라는 것을 이미 짚어 뒀다.
불변식: 신규 leaf·`regex` 금지(§1), **ASCII 폴딩**(P1), `[0-9]`(P2), 호스트 미검사(P3),
두 함수 비대칭(P4/P5), `parseInt` 정책 명시(P6), `> 0` 게이트(P7), 후행 슬래시 전부(P8),
`pathname` 사용(P9), 대소문자 보존(P10), `js_trim`(P11), 하드코딩 호스트 + 로컬 인코더(P12),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
