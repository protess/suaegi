> **⚠️ Codex 교차검증 정정 (VALIDATED-WITH-CORRECTIONS):** 6-stage precedence·`\d` ASCII lock·verbatim-string
> 숫자캡처·URL anchoring 전부 CONFIRMED. 정정: (1) 데니리스트 **24개**(26 아님); (2) 정규식 **14 static + 1
> dynamic**(8+1 아님); (3) `stripWorkIdentifierEcho` dynamic regex는 **"safe by construction" 틀림** — export
> 시그니처가 무제약 `tokens: string[]`이라 metachar 토큰 주입 위험 → **`regex::escape()` 방어적 적용**(추출기
> 산출 토큰엔 무영향, 임의 metachar 토큰에서만 divergence — Orca의 ReDoS/throw footgun 회피, 보안 하드닝);
> (4) Azure `?`-branch 테스트는 실제로 `?`/`#` 미행사(매칭이 `url.pathname` 대상=query/fragment 제외, `$` 대안이
> 매치); (5) 테스트 **15 extract + 2 strip + 0 formatIdentifierFirst**. 결정: regex 크레이트(`[0-9]`/`(?-u:\b)` ASCII
> lock), `url` 크레이트(WHATWG parity, divergence 핀: trailing-slash pathname·protocol 소문자·invalid→try/catch),
> `to_ascii_lowercase`. **최종 계약=플랜 supersede.**

# Research: `work-item-reference` — Rust 포팅 계약서

**대상**: Orca `src/shared/work-item-reference.ts` (190L) + `.test.ts` (127L) @ v1.4.150-rc.0
**성격**: **PURE + DETERMINISTIC**. 시각·난수·I/O·전역상태 0. 유일한 런타임 외부의존은 WHATWG `URL` 생성자(`new URL()` :73) 하나뿐 — 나머지는 정규식/문자열 연산. `.test.ts` 가 오라클, `extractWorkIdentifier`/`stripWorkIdentifierEcho` 는 verbatim 포팅 대상.

## 결론 요약 (먼저 읽을 것)

1. **정규식 8개 + 동적 1개 = 최대위험**. 모든 숫자 매치가 JS `\d`(=ASCII `[0-9]`) 인데 Rust `regex` 의 `\d` 는 기본 **Unicode Nd**(아라비아-인도 숫자·전각숫자 포함). **포트는 반드시 `[0-9]` 또는 `(?-u:\d)`** 로 못박아야 함. 동일하게 `\b`/`\w`(JS ASCII) vs Rust Unicode, `\s`(JS 특정집합 ≠ Rust `\p{White_Space}`) 도 전부 발산. §4 표.
2. **숫자 파싱이 아예 없다**. 캡처된 번호는 **문자열 그대로** `label`/`tokens` 에 들어감(:67, :157, :164). `parseInt`/`Number` 0건. → **선행 0 보존**(`pull/007`→`"PR 007"`), 오버플로우 무관, 자릿수 제한만 존재(티켓 `\d{1,7}`, 나머지 `\d+` 무제한). Rust 도 `String` 으로 취급, `u64` 파싱 금지.
3. **식별자 문법 = 정확히 6-단계 precedence** (:130-167): ① 제공자 URL(§2) → ② `merge request` → ③ `pull request`/`pr` → ④ `issue` → ⑤ 네임스페이스 티켓 `[A-Z]{2,10}-\d{1,7}`(denylist 스킵) → ⑥ bare `#\d+` → ⑦ null. 상위 매치가 이기며 **하위는 시도조차 안 함**.
4. **URL 검증은 host 가 아니라 path 구조**(:1-6 주석). GitLab `/-/`, GitHub `^/owner/repo/(issues|pull)/N`, Bitbucket Cloud/Server, Azure `/_git/`. GitHub·Bitbucket-Cloud 만 `^` 앵커(경로 루트 강제), 나머지는 비앵커(경로 어디서나). 이 앵커 유무가 CDN URL 거부(:56-63)의 핵심.
5. **`URL` 전역 의존이 유일한 self-contained 예외**. `url.protocol`(:77, 소문자+콜론)·`url.pathname`(:80) 시맨틱에 의존 → Rust `url` 크레이트(동일 WHATWG 스펙)로 대체. 값/타입 import 0건(§0). `formatIdentifierFirst` 는 **오라클 미커버**(§5-미커버) — 핀 필수.

---

## 0. 파일 구조 / import

- 모듈 `work-item-reference.ts` — **import 문 0건**(값·타입 모두). 1-6행 주석, 8행부터 코드. `require` 0건. 스캔의 "zero value-imports" 확인 + **type-only import 도 없음**.
- **유일한 외부 런타임 의존 = `URL` 전역 생성자**(:73 `new URL(raw)`). WHATWG URL 파서. `url.protocol`(:77)·`url.pathname`(:80) 만 사용. → Rust: `url::Url::parse` + `.scheme()` + `.path()`. self-contained 아님, 이 한 점만 예외.
- 오라클 `.test.ts:1-2` — `vitest` + 대상에서 `extractWorkIdentifier`, `stripWorkIdentifierEcho` 만 import. **`formatIdentifierFirst` 는 import 안 함 = 테스트 0건.** `WorkIdentifier` 타입은 `.toEqual({...})` 리터럴로 간접 검증.

---

## 1. Public surface

### 1.1 exports

- **`type WorkIdentifier`** (:8-14) — export. `{ label: string; tokens: string[] }`. `label` = 사람용 식별자-우선 라벨(`PR 1033`/`MR 42`/`ENG-456`/`#321`); `tokens` = **소문자화된 식별자 토큰들**(consumer 가 슬러그/설명에서 제거용). Rust: `struct WorkIdentifier { label: String, tokens: Vec<String> }`.
- **`extractWorkIdentifier(text: string): WorkIdentifier | null`** (:130-168) — export, pure(+URL전역). §3.
- **`formatIdentifierFirst(label: string, detail: string): string`** (:175-177) — export, pure. §3.4. **오라클 미커버.**
- **`stripWorkIdentifierEcho(text: string, identifier: WorkIdentifier): string`** (:184-190) — export, pure. §3.5.

### 1.2 비-export 내부

- **`IDENTIFIER_SCAN_LIMIT = 4096`** (:18) — 스캔 상한(문자, 실제로는 UTF-16 code unit — §4-T-slice).
- **`NON_TICKET_PREFIXES`** (:24-49) — `Set<string>` 26개: `UTF SHA MD ISO RFC AES RSA EC ES RS HS PS GPT MPEG UTC GMT IPV IEEE ANSI ASCII TLS SSL HTTP HTTPS`. 전부 대문자. 티켓 오탐 방지. Rust: `HashSet<&str>` 또는 `matches!`. **주의: 단일문자 prefix(`P-256`)는 `{2,10}` 에 안 걸려 엔트리 불필요**(:22-23 주석).
- 정규식 상수 6개 (§2.0).
- **`taggedIdentifier(type, num)`** (:66-68) — `{ label: `${type} ${num}`, tokens: [type.toLowerCase(), num] }`. `type` ∈ `'PR'|'MR'|'Issue'`.
- **`urlToIdentifier(raw)`** (:70-106) — §2.1.
- **`findUrlIdentifier(text)`** (:108-123) — §2.2.

---

## 2. URL 경로 (최우선 precedence)

### 2.0 정규식 상수 6개 (전부 file:line, flags, `\d` 주의)

| # | 이름 | 소스 (:line) | flags | 앵커 | 캡처 | 비고 |
|---|---|---|---|---|---|---|
| R1 | `URL_IN_TEXT` | `/https?:\/\/[^\s<>()[\]"']+/gi` (:51) | `g`,`i` | — | 0 | URL 토큰 추출. `\s` 부정클래스가 URL 종료경계. `_*~` 등은 **포함**됨(→trailing strip 필요). |
| R2 | `GITLAB_ITEM_PATH` | `/\/-\/(issues\|work_items\|merge_requests)\/(\d+)(?:[/?#]\|$)/i` (:54) | `i` | 비앵커 | 2 | `/-/` 마커. `\d+`. |
| R3 | `GITHUB_ITEM_PATH` | `/^\/[^/]+\/[^/]+\/(issues\|pull)\/(\d+)(?:[/?#]\|$)/i` (:55) | `i` | **`^`** | 2 | owner/repo 루트 강제. |
| R4 | `BITBUCKET_CLOUD_ITEM_PATH` | `/^\/[^/]+\/[^/]+\/pull-requests\/(\d+)(?:[/?#]\|$)/i` (:57) | `i` | **`^`** | 1 | |
| R5 | `BITBUCKET_SERVER_ITEM_PATH` | `/\/(?:projects\|users)\/[^/]+\/repos\/[^/]+\/pull-requests\/(\d+)(?:[/?#]\|$)/i` (:60-61) | `i` | 비앵커 | 1 | project/user 중첩. |
| R6 | `AZURE_DEVOPS_ITEM_PATH` | `/\/_git\/[^/]+\/pullrequests?\/(\d+)(?:[/?#]\|$)/i` (:64) | `i` | 비앵커 | 1 | `pullrequests?` = s 옵션. |

- **모든 R2-R6 의 `(\d+)` 는 JS ASCII `[0-9]`**. Rust `\d` = Unicode Nd → **반드시 `[0-9]`/`(?-u:\d)`**. 최상위 위험.
- **`(?:[/?#]|$)`** = 번호 뒤가 `/`·`?`·`#`·문자열끝 이어야 함. → `pull/1033x` 거부, `pull/1033/files`·`pullrequest/4521?_a=files`(:38) 허용. 정확히 캡처만.
- 앵커: **R3·R4 만 `^`**(경로 루트에 owner/repo). R2·R5·R6 은 비앵커(경로 어디서든 `/-/`,`/projects|users/…/repos`,`/_git/`). 이 차이가 CDN 거부(:56)의 근거.
- `i` 플래그: Rust `(?i)`. R1 의 `i` 는 `https?`·`HTTPS` 매칭용(하지만 §2.1 에서 protocol 재검증).

### 2.1 `urlToIdentifier(raw)` (:70-106)

1. :72-75 `try { url = new URL(raw) } catch { return null }` — **WHATWG 파싱 실패 = null**. Rust `Url::parse(raw).ok()?`.
2. :77-79 `if (url.protocol !== 'https:' && url.protocol !== 'http:') return null`. `url.protocol` 는 **소문자+콜론**(`HTTPS://`→`https:`). Rust `url.scheme()`(소문자, 콜론無) 로 `"http"|"https"` 검사.
3. :80 `path = url.pathname` — WHATWG 직렬화 경로(dot-segment 정규화, percent-encoding 유지). Rust `url.path()`.
4. :81-105 **순서대로 첫 매치 승**: GitLab(R2) → GitHub(R3) → Bitbucket-Cloud(R4) → Bitbucket-Server(R5) → Azure(R6). 하나도 없으면 null.
   - GitLab(:83-85): `gitlab[1].toLowerCase() === 'merge_requests'` → `MR`, 아니면(`issues`/`work_items`) → `Issue`.
   - GitHub(:89-91): `github[1].toLowerCase() === 'pull'` → `PR`, 아니면(`issues`) → `Issue`.
   - R4/R5/R6 → 무조건 `PR`.
   - **`.toLowerCase()` 비교(:83,89)**: 입력이 정규식 캡처(`issues|pull|merge_requests`, `i` 플래그라 `Pull`/`PULL` 도 캡처 가능) → 소문자화 후 비교. 전부 ASCII → Rust `eq_ignore_ascii_case` 또는 `to_ascii_lowercase`. `to_lowercase`(Unicode) 불필요.

### 2.2 `findUrlIdentifier(text)` (:108-123)

1. :109 `urls = text.match(URL_IN_TEXT)` — **global 매치 = 매칭된 URL 문자열 배열**(없으면 null → :110-112 return null). 등장순서 유지.
2. :113-121 **각 URL 순회, 첫 성공 반환**. :117 각 raw 에 **trailing 문장부호/마크다운 강조 스트립**: `raw.replace(/[.,;:!?*_~]+$/, '')` (R7). `$` 앵커라 **끝에서만** 제거 — 내부 `_`(`merge_requests`)는 보존(:114-116 주석). 그 뒤 `urlToIdentifier`.
   - **R7 = `/[.,;:!?*_~]+$/`** (:117) — flags 없음, 리터럴 클래스. `.`,`,`,`;`,`:`,`!`,`?`,`*`,`_`,`~` 1개↑ 문자열끝. Rust: `[.,;:!?*_~]+$`(유니코드 무관, ASCII 리터럴). `_`/`*` 포함이 마크다운(`_…_`, `**…**`) 언랩의 이유(§5-T9).
   - **loop 는 "첫 URL 실패 → 다음 URL 시도" 경로가 오라클 미커버**(§5-미커버).

---

## 3. `extractWorkIdentifier` 및 나머지 EXACTLY

### 3.0 진입 (:130-131)

- :131 `const scanned = text.slice(0, IDENTIFIER_SCAN_LIMIT)` — **JS `.slice` = UTF-16 code unit 기준**, 앞 4096 code unit 절단. Rust `String`(UTF-8/char) 과 발산: 서로게이트(emoji) 가 4096 경계 근처면 절단점 상이. §4-T-slice. 이후 모든 매칭은 `scanned` 위에서.

### 3.1 precedence 사슬 (:133-167) — 첫 매치 반환, 하위 스킵

1. **URL**(:133-136): `findUrlIdentifier(scanned)` 성공 시 반환. (§2)
2. **merge request**(:139-142): `scanned.match(/\bmerge\s+request\s*[#!]?\s*(\d+)/i)` (R8). 성공 → `taggedIdentifier('MR', m[1])`. `[#!]?` = `#` 또는 GitLab `!` 허용(`!9`→MR 9, :80).
3. **pull request / pr**(:143-146): `scanned.match(/\bpull\s+request\s*#?\s*(\d+)/i) ?? scanned.match(/\bpr\s*#?\s*(\d+)/i)` (R9, R10). 앞 실패 시 뒤 시도(`??`). → `PR`. `\bpr\s*#?\s*(\d+)` 는 `pr123`(공백·# 없음)도 매치.
4. **issue**(:147-150): `scanned.match(/\bissue\s*#?\s*(\d+)/i)` (R11) → `Issue`.
5. **네임스페이스 티켓**(:155-159): `for (const ticket of scanned.matchAll(/\b([A-Z]{2,10})-(\d{1,7})\b/g))` (R12, **global**). **`i` 플래그 없음 = 대문자만**(:154 주석: `gpt-4` 배제). `!NON_TICKET_PREFIXES.has(ticket[1])` 인 **첫 티켓** 반환: `{ label: `${p}-${n}`, tokens: [p.toLowerCase(), n] }`. denylist 는 스킵하고 **계속 순회**(`SHA-256 … ENG-456`→ENG-456, :96).
6. **bare `#`**(:162-165): `scanned.match(/(?:^|\s)#(\d+)\b/)` (R13, flags 없음). → `{ label: `#${n}`, tokens: [n] }`. **tokens 는 번호 1개뿐**(type 토큰 없음).
7. :167 `return null`.

**정규식 R8-R13 세부**:

| # | 소스 (:line) | flags | 트랩 |
|---|---|---|---|
| R8 | `/\bmerge\s+request\s*[#!]?\s*(\d+)/i` (:139) | `i` | `\b` `\s` `\d` 전부 Unicode 발산; `\s+`=1↑, `\s*`=0↑ |
| R9 | `/\bpull\s+request\s*#?\s*(\d+)/i` (:143) | `i` | 동일 |
| R10 | `/\bpr\s*#?\s*(\d+)/i` (:143) | `i` | `\bpr` 경계; `pr123` 매치 |
| R11 | `/\bissue\s*#?\s*(\d+)/i` (:147) | `i` | 동일 |
| R12 | `/\b([A-Z]{2,10})-(\d{1,7})\b/g` (:155) | `g` (**no `i`**) | 대문자전용; `\d{1,7}` 7자리 상한; 뒤 `\b` — 8자리(`ENG-12345678`)는 **매치 실패**(§4-T-boundary) |
| R13 | `/(?:^|\s)#(\d+)\b/` (:162) | none | `\s`·`\b`·`\d` Unicode; 라인시작/공백 뒤 `#` |

- **R8-R13 의 `\b`/`\s`/`\w`(암묵)/`\d` 는 전부 JS ASCII 의미**. Rust `regex` 기본 Unicode → 각각 `(?-u:\b)`, 명시 공백클래스, `[0-9]` 로 다운그레이드 필요. §4.
- **R12 대문자전용 + denylist `.has` 는 exact 대문자매치**. MEMORY 의 "path denylist case-insensitive" 함정과 **다름**: 여기 prefix 는 정규식이 이미 `[A-Z]` 로 대문자 강제 → lowercase(`sha-256`)는 R12 자체에 안 걸려 denylist 도달 불가. exact `.has` 가 정답.

### 3.4 `formatIdentifierFirst(label, detail)` (:175-177) — **오라클 0건**

- :176 `return detail ? `${label} - ${detail}` : label`. **JS falsy 단축**: `detail === ""` → `label` 만. `"  "`(공백) 은 truthy → `${label} -   `. → Rust **`if detail.is_empty()`**, `trim().is_empty()` 아님. 구분자 정확히 `" - "`(스페이스-하이픈-스페이스).
- 테스트 전무 → 두 분기 모두 **핀 필수**(§6).

### 3.5 `stripWorkIdentifierEcho(text, identifier)` (:184-190)

1. :186-188 `for (const token of identifier.tokens) stripped = stripped.replace(new RegExp(`\\b${token}\\b`, 'gi'), ' ')` — **동적 정규식 R14**. 각 토큰을 `\b<token>\b`(gi, 경계+대소무시)로 찾아 **스페이스로** 치환.
   - **토큰 **미이스케이프** 삽입**: 원본은 이스케이프 안 함. 하지만 tokens 는 항상 `[a-z]+`(소문자 prefix) 또는 `\d+`(숫자문자열) — 정규식 메타문자 불가 → 실질 안전. Rust 포트는 known-safe 로 두거나 `regex::escape` 로 방어(동작 동일).
   - **`\b` Unicode 발산**: 토큰 `pr` 가 `PRÉ` 같은 유니코드 인접 시 JS(ASCII `\b`)는 경계 성립→매치, Rust(Unicode `\b`)는 불성립→미매치. §4-T-boundary. `(?-u:\b)` 로 못박아야 verbatim.
   - `gi` = Rust `(?i)`. 토큰이 ASCII 라 `(?i)` 와 `(?i-u)` 사실상 동일하나 명시 권장.
2. :189 `.replace(/\s+/g, ' ')` (R15, `g`) — 연속 공백 → 단일 스페이스. `\s+` Unicode 발산(§4).
3. :189 `.trim()` — 양끝 공백 제거. **JS trim** ≠ Rust `str::trim`(§4-T-ws): JS 는 U+FEFF 제거·U+0085 미제거, Rust 는 반대.

---

## 4. 트랩 클래스 × Rust 발산 (핵심)

- **T-digit (`\d`, 최상위)**: R2-R6(URL), R8-R13(텍스트) 전부 `\d`. **JS `\d`=`[0-9]`, Rust `regex \d`=`\p{Nd}`**(아라비아-인도 `٠١٢`, 전각 `０１２`, 데바나가리 등 포함). 미교정 시 `pull/٤٢` 같은 입력이 Rust 에서만 매치 → 발산. **결정: 전 `\d`→`[0-9]` (또는 크레이트 상단 `(?-u:\d)`)**. 캡처 후 그 문자열이 label 에 그대로 들어가므로(§결론2) 비-ASCII 숫자를 매치하면 label 도 오염.
- **T-word (`\b`/`\w`)**: R8-R13, R14 의 `\b`. **JS `\w`=`[A-Za-z0-9_]` ASCII, Rust `\w`/`\b` Unicode**. 유니코드 문자 인접 시 경계 판정 반대(예: `stripWorkIdentifierEcho` 토큰이 유니코드 옆). **결정: `(?-u:\b)`**.
- **T-space (`\s`)**: R1(`[^\s…]`), R8-R11(`\s+`/`\s*`), R13(`(?:^|\s)`), R15(`\s+`). **JS `\s`** = `[\t\n\v\f\r    -     　﻿]`. **Rust `\s`** = `\p{White_Space}`. 차이: **U+FEFF 는 JS만**(BOM/ZWNBSP), **U+0085(NEL) 는 Rust만**. 나머지(U+00A0,U+1680,U+2000-200A,U+2028/9,U+202F,U+205F,U+3000)는 공통. **결정: 명시 문자클래스로 JS 집합 복제**(특히 U+FEFF 를 공백취급, U+0085 를 비공백취급).
- **T-ws (`.trim()`)**: :189 `.trim()`. **JS trim** = WhiteSpace+LineTerminator(≈`\s`, U+FEFF 포함, U+0085 미포함). **Rust `str::trim`** = `char::is_whitespace`(U+0085 포함, U+FEFF 미포함). 발산. verbatim 하려면 JS 집합 기준 커스텀 trim 또는 U+FEFF/U+0085 보정.
- **T-lower (`.toLowerCase()`)**: :67(`type.toLowerCase()`), :83, :89, :157(`ticket[1].toLowerCase()`). 입력 전부 ASCII(`PR`/`MR`/`Issue`, `pull`/`issues`/`merge_requests`, `[A-Z]{2,10}`) → **Rust `to_ascii_lowercase()`** 가 정답. `to_lowercase()`(Unicode 풀 케이스폴드, Turkish-I/ß) 불필요 & 위험. 비교(:83,89)는 `eq_ignore_ascii_case` 가능.
- **T-slice (UTF-16)**: :131 `text.slice(0, 4096)`. **JS = UTF-16 code unit**. Rust byte/char 절단과 다름 — BMP밖 문자(emoji=2 code unit)가 경계 근처면 상이, 서로게이트 중간 절단도 JS 는 고아 서로게이트 허용. **결정: UTF-16 단위 4096 절단 재현**(`encode_utf16().take(4096)`+lossy 복원) vs "스캔 바운드일 뿐, char/byte 근사 허용" 판단 — Codex 확인.
- **T-boundary (`\d{1,7}\b`, R12)**: 8자리 `ENG-12345678` → `\d{1,7}` 가 7자리 잡아도 뒤 `\b` 불성립(숫자↔숫자 비경계), 백트랙 전부 실패 → **매치 없음**. Rust 동일 문법이면 자동 재현되나, `[0-9]{1,7}` 로 바꿔도 `\b` 를 반드시 유지해야 함. 오라클 미커버.
- **T-number (파싱 없음)**: **`parseInt`/`Number` 0건**. 캡처 숫자문자열이 label/token 에 verbatim. 선행0 보존, 오버플로우/부호/기수 무관. Rust: `String` 유지, 정수변환 금지.
- **T-url (WHATWG 의존)**: `new URL`(:73), `.protocol`/`.pathname`. self-contained 예외. Rust `url` 크레이트(WHATWG 스펙 구현)로 대체 — dot-segment 정규화·percent-encoding 유지·scheme 소문자화 동작이 일치하는지 확인 필요(§6).
- **T-regex-escape (동적)**: R14(:187) 토큰 미이스케이프 — 실입력 안전하나 포트 결정 필요(escape vs known-safe).

---

## 5. 오라클 케이스별 (`.test.ts`, `extractWorkIdentifier` 13 + `stripWorkIdentifierEcho` 2)

- **T1 (:5-10) GitHub PR URL** — `Review https://github.com/EveryInc/plugin/pull/1033` → `{label:'PR 1033', tokens:['pr','1033']}`. Crux: R3 앵커+`pull`→PR, tokens 소문자.
- **T2 (:12-16) Bitbucket Cloud** — `…/team/repo/pull-requests/77` → `PR 77`. Crux: R4.
- **T3 (:18-30) Bitbucket Server** — `…/projects/ENG/repos/orca/pull-requests/1288` → `{label:'PR 1288',tokens:['pr','1288']}`; **`/users/jane/repos/orca/pull-requests/9/overview`** → `PR 9`. Crux: R5 `projects|users` + 뒤 세그먼트(`/overview`)를 `(?:[/?#]|$)` 의 `/` 로 허용.
- **T4 (:32-41) Azure DevOps** — `dev.azure.com/contoso/Orca/_git/orca/pullrequest/4521` → `PR 4521`; `contoso.visualstudio.com/…/_git/orca/pullrequest/4521?_a=files` → `PR 4521`. Crux: R6 비앵커 `/_git/`, `pullrequest`(s 없음), `?_a=files` 를 `?` 브랜치로 허용.
- **T5 (:43-50) GitLab MR + work_items** — `…/-/merge_requests/42` → `{MR 42,['mr','42']}`; `…/-/work_items/9` → `Issue 9`. Crux: R2 `merge_requests`→MR, `work_items`→else→Issue.
- **T6 (:52-54) issue URL** — `…/o/r/issues/88` → `Issue 88`. Crux: R3 `issues`→Issue.
- **T7 (:56-63) 유사경로 거부** — `cdn.vendor.com/assets/pull/2023/data.json` → **null**(R3 앵커: `/assets/pull/…` 에서 3번째 세그먼트가 `2023`≠`issues|pull`); `github.com/o/r/pull/notanumber` → **null**(`\d+` 뒤 없음). Crux: 앵커+숫자강제.
- **T8 (:65-67) URL 뒤 문장부호** — `(see …/pull/5).` → `PR 5`. Crux: R1 이 `)` 를 클래스에서 배제해 `…/pull/5` 까지만 매치(`.`·`)` 미포함).
- **T9 (:69-74) 마크다운 강조 언랩** — `_…/pull/5_ now` → `PR 5`; `**…/pull/1094**` → `PR 1094`. Crux: R1 이 `_`·`*` 를 **포함**해 매치 후 R7 이 trailing `_`/`**` 제거. 내부 `_` 보존.
- **T10 (:76-81) 텍스트 참조** — `PR #1094`→`PR 1094`(R10); `pull request 500`→`PR 500`(R9); `issue 12`→`Issue 12`(R11); `merge request !9`→`MR 9`(R8 `!`). Crux: precedence ②③④, `[#!]?`/`#?` 선택.
- **T11 (:83-88) 네임스페이스 티켓** — `implement ENG-456 login flow` → `{ENG-456,['eng','456']}`. Crux: R12 대문자, tokens 소문자 prefix + 숫자.
- **T12 (:90-94) 표준/암호 배제** — `SHA-256`,`UTF-8`,`ISO-8601` → 전부 **null**. Crux: denylist `.has`.
- **T13 (:96-98) denylist 스킵 후 실키** — `AES-256 for ticket ENG-99` → `ENG-99`. Crux: R12 `matchAll` 순회, `AES` 스킵→`ENG` 반환.
- **T14 (:100-104) URL > 티켓 precedence** — `per RFC-2616 notes, review …/pull/7` → `PR 7`. Crux: URL 단계(①)가 티켓 단계(⑤)보다 먼저 → RFC-2616(denylist라 어차피 스킵) 무시하고 URL 승.
- **T15 (:106-109) bare 번호 fallback + null** — `look at #321 when free` → `#321`(R13); `add a dark mode toggle to settings` → **null**. Crux: ⑥ bare, tokens `['321']`, 그 외 null.
- **T16 (:113-119) stripEcho 라벨토큰 제거** — `stripWorkIdentifierEcho('Review this community PR', {tokens:['pr','1094']})` → `'Review this community'`. Crux: R14 `\bpr\b`gi 제거→공백, `1094` 부재 no-op, `\s+`→' ' + trim.
- **T17 (:122-125) stripEcho 티켓 에코** — `('Fix ENG 456 crash', {tokens:['eng','456']})` → `'Fix crash'`. Crux: `\beng\b`gi + `\b456\b` 제거 → `'Fix   crash'` → collapse+trim.

### 미커버 엣지 (핀 추가 필수 — mutation-verify 대상, §6)

- **E1 `formatIdentifierFirst`** — **테스트 0건**. `("PR 5","Review")`→`"PR 5 - Review"`, `("PR 5","")`→`"PR 5"`, `("PR 5","  ")`→`"PR 5 -   "`(공백 truthy). 두 분기 + falsy 규칙 핀.
- **E2 비-ASCII 숫자** — `\d` Unicode 발산 잠금용. `pull/٤٢`(아라비아) / `#４２`(전각) → JS **null**(ASCII만). Rust 미교정 시 매치 → **회귀 핀**으로 null 고정.
- **E3 findUrlIdentifier 다중 URL loop** — 첫 URL 실패(예 CDN) + 둘째 URL 성공 → 둘째 반환. 오라클은 단일 URL 만 — loop "다음 시도" 경로 미커버.
- **E4 `\d{1,7}\b` 8자리 경계** — `ENG-12345678` → JS **null**. Rust 동일 문법이면 자동이나 핀으로 고정.
- **E5 whitespace 엣지** — U+FEFF/U+0085/U+00A0 가 `\s`·`.trim()` 에서 JS 집합대로 처리되는지(§4 T-space/T-ws). 특히 stripEcho `.trim()` 양끝 U+FEFF.
- **E6 UTF-16 slice 경계** — 4096 근처 emoji/서로게이트로 절단점 상이(§4 T-slice). 프롬프트 앞 4096 안에 식별자 있으면 무관하나, 경계 걸치는 URL 은 발산 가능.
- **E7 `http://`(비-TLS)** — `:77` 이 `http:` 허용. 오라클 전부 https. 핀으로 http 허용 고정.
- **E8 `pr123`(구분자 없음)** — R10 `\bpr\s*#?\s*(\d+)` 가 매치. 미커버.
- **E9 잘못된 scheme / `new URL` 실패** — `ftp://…/pull/5`(R1 미매치라 도달 안 함)·상대경로. `urlToIdentifier` try/catch·protocol 게이트 미커버.

---

## 6. Codex 교차검증 open questions

1. **정규식 vs 핸드롤 + `\d` Unicode 발산**: 정규식 8개(R1-R6,R8-R13) + 동적 R14/R15 를 Rust `regex` 크레이트로 갈지, path 매칭만 핸드롤할지. 확정 시 **전 `\d`→`[0-9]`(또는 `(?-u:\d)`), 전 `\b`→`(?-u:\b)`** 를 크레이트 정책으로 못박기. 특히 R12(대문자전용, `i` 없음)와 R14(동적, gi) 를 `regex` 로 갈 때 토큰 이스케이프(`regex::escape`) 여부. `\d{1,7}\b` 8자리 거부(E4) 재현 확인.
2. **case-fold / whitespace 술어**: (a) `.toLowerCase()`(:67,83,89,157) 은 입력 전부 ASCII → **`to_ascii_lowercase`/`eq_ignore_ascii_case`** 로 충분한지(Unicode `to_lowercase` 배제). (b) `\s`(§4 T-space) 를 JS 집합(**U+FEFF 포함, U+0085 배제**)으로 복제할지 Rust `\p{White_Space}` 근사 허용할지. (c) `.trim()`(:189) 을 JS trim 시맨틱(U+FEFF strip / U+0085 keep)으로 맞출지.
3. **숫자 파싱 = 없음 확정**: 캡처 숫자를 **문자열 verbatim** 유지(선행0 보존, `u64` 변환 금지) 재확인. `\d{1,7}` vs `\d+` 자릿수 정책 그대로.
4. **UTF-16 slicing**: :131 `slice(0,4096)` 을 UTF-16 code unit 정확 재현(`encode_utf16`)할지, char/byte 근사(스캔 바운드일 뿐)로 갈지. 경계-걸침 URL(E6) 발산 허용 범위.
5. **self-contained 여부 + `URL` 의존**: 모듈 import 0건이나 **`new URL`(:73)·`.protocol`·`.pathname` = WHATWG 파서 의존**. Rust `url` 크레이트가 dot-segment 정규화·percent-encoding 유지·scheme 소문자화에서 동일 결과인지, 아니면 http/https + path 만 필요하니 경량 핸드롤 파서로 대체할지. `url` 크레이트가 오라클 URL(GHE/self-hosted GitLab/Azure on-prem)을 전부 동일 pathname 으로 직렬화하는지 스팟체크.
6. **미커버 핀 E1-E9**: 특히 **E1(`formatIdentifierFirst` 0-테스트), E2(비-ASCII 숫자 null 고정), E4(8자리), E7(http 허용), E8(`pr123`)** 를 포팅 회귀테스트로 명시 추가 — mutation-verify 필수. E3(다중 URL loop)·E5(whitespace 엣지)·E6(slice 경계)도 핀 대상.
