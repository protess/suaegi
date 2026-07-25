# Plan — work-item-reference (작업항목 식별자 파싱/포맷) 확정

조사: `docs/superpowers/research/2026-07-25-work-item-reference.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS**(6-stage·`\d` lock·URL anchoring CONFIRMED, 정정 5 +
6질문 답변). 이 문서가 구현 계약이며 조사를 supersede한다. 인용은 별도 명시 없으면 `src/shared/work-item-reference.ts`.

## 0. 결정 (조사 + Codex 확정)

Orca의 **작업항목 식별자 추출/포맷** 순수 모듈(이슈/PR/티켓 참조 파싱 — `JIRA-123`·`#456`·URL 등). tracker/forge
통합 인접. **import 0**(타입도 없음) — 유일 외부 의존은 WHATWG `URL` 전역(`:73,77,80`) → Rust `url` 크레이트.

**크레이트: 새 leaf `suaegi-workref`** (deps: **`regex`**[워크스페이스 기존] + **`url`**[신규, WHATWG 파서]).
순수·자기완결(URL/regex 외 표준 라이브러리만). 4 export + 3 helper + 14 static regex + 1 dynamic + 24-entry 데니리스트.

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — regex 크레이트, 단 ASCII 강제(`\d` Unicode lock).** 모든 숫자 캡처는 JS ASCII `\d`=`[0-9]`인데 Rust
  `regex \d`는 Unicode Nd(아라비아/전각 숫자 매치) → **`[0-9]`로 명시**(또는 `(?-u:\d)`). `\b`/`\w`도 JS는 ASCII
  → **`(?-u:\b)`/`(?-u:\w)`**. **캡처된 숫자는 label에 verbatim 문자열로 흐름**(parseInt/Number 0건 — 선행0 보존,
  overflow 없음, `String` 유지). 14 static regex 전부 이 ASCII lock 적용. 6-stage precedence 순서 verbatim(`:130-168`):
  URL → `merge request` → `pull request`/`pr` → `issue` → `[A-Z]{2,10}-[0-9]{1,7}` 티켓(24-데니리스트 필터) →
  bare `#[0-9]+` → null.
- **C2 — `url` 크레이트로 WHATWG `new URL()` 재현, divergence 핀.** URL 검증은 **path 구조**(host 아님):
  GitHub/Bitbucket-Cloud `^`-anchored, GitLab/Bitbucket-Server/Azure non-anchored(`:77,80` 매칭은 `url.pathname`
  대상 — query/fragment 제외). Rust `url::Url::parse`: (a) invalid URL → JS `try/catch`처럼 **skip(에러 아님)**,
  (b) `.pathname`/`.protocol` 매핑(protocol 소문자, trailing-slash pathname). **JS-vs-url-crate divergence 핀**:
  trailing-slash pathname·protocol casing·invalid-URL skip. (Azure 테스트가 `?`/`#` 미행사 — pathname이 제외하므로
  `$` 대안 매치, 정정 반영.)
- **C3 — case-fold `to_ascii_lowercase`, whitespace JS-술어.** `.toLowerCase()`는 ASCII 입력만 → `to_ascii_lowercase`/
  `eq_ignore_ascii_case`. `.trim()`/`\s`는 JS 집합(U+FEFF포함/U+0085제외) → 재사용 `is_js_whitespace` 술어
  (suaegi-taskquery 선례). `.slice(0,4096)` scan cap은 JS UTF-16 code-unit — **char-scalar로 이식+문서화 divergence**
  (입력 길이 cap일 뿐 경계 아님, gen-prompt C1 선례), char-boundary-safe(panic 금지).
- **C4 — `stripWorkIdentifierEcho` dynamic regex는 `regex::escape()` 방어적 적용(보안 하드닝, 문서화 divergence).**
  `:184-190`이 각 token으로 RegExp 빌드. export 시그니처가 **무제약 `tokens: string[]`** → metachar 토큰이 정규식
  주입/ReDoS/throw footgun. Codex: **각 token을 `regex::escape()`** 후 빌드. **추출기 산출 토큰(`[A-Z]{2,10}-[0-9]{1,7}`/
  `#[0-9]+`, metachar 無)엔 무영향** — 오라클 2케이스 동일 통과. **임의 metachar 토큰에서만 Orca와 divergence**(Orca는
  literal replace 대신 정규식 해석 = injection). 저장소 보안 규율([[path-denylist-case-insensitive]] RCE 교훈)상
  injection footgun 재현 금지. **핀**: metachar 토큰(`a.b`)이 리터럴 취급됨(문서화 divergence).
- **C5 — 데니리스트 24-entry verbatim**(`:` 인용 표 그대로), scan limit `.slice(0,4096)` C3.

## 2. 마일스톤

### M1 — work-item-reference 전체 (`suaegi-workref` 신규, 단일 마일스톤)
4 export: `WorkIdentifier` 타입(`:8-14`), `extract_work_identifier`(`:130-168`, 6-stage + helper `tagged_identifier`/
`url_to_identifier`/`find_url_identifier`), `format_identifier_first`(`:175-177`, **오라클 0 테스트 → 양분기 핀 필수**),
`strip_work_identifier_echo`(`:184-190`, **C4 escape**). 14 static regex(C1 ASCII lock), 24-데니리스트(C5).

**오라클(17 = 15 extract + 2 strip, 전부 이식):** extract 15케이스(URL 각 provider·MR·PR·issue·티켓·denylist·bare-#·
null), strip 2케이스.

**추가 핀(Codex, 미커버):** (E-fmt) **formatIdentifierFirst 양분기**(0 테스트); (E-digit) **비-ASCII 숫자 거부**
(`JIRA-١٢٣`·`#٤٥` → 매치 안 됨, C1 `\d` lock — Rust `\d` 포트면 통과해버림); (E-url) multi-URL fallback loop, `http://`,
trailing-slash/protocol-casing divergence; (E-8digit) `[0-9]{1,7}` 경계(8자리 거부); (E-strip) C4 metachar 토큰 리터럴;
(E-ws) whitespace/trim edge; (E-slice) 4096 scan cap char-boundary-safe.

*mutation:* `\d` Unicode(비-ASCII 숫자 통과), 6-stage 순서 뒤섞기, denylist 우회, URL anchoring(`^` 제거), invalid-URL를
에러로(skip 대신), escape 제거(injection), to_ascii→Unicode fold, verbatim 숫자→parseInt(선행0 손실), scan-cap 제거.

## 3. Deferred (명시)
- **네이밍 클러스터**(branch-name-from-work[+marine-creatures 데이터]·display-name-from-work·workspace-name) —
  work-item-reference에 의존, **UI-cosmetic**(사람눈/저가치). 이 플랜은 substantive한 reference 파싱만.
- **URL/regex 크레이트 도입** — 표준·WHATWG 충실. hand-roll WHATWG는 거대 오류표면이라 회피.

## 4. 순서 (확정)
M1 단일 마일스톤(4 export + 오라클 17 + C1-C5 추가 핀). 불변식: `\d` ASCII lock(C1, transient≠garbage 아님 — 비-ASCII
숫자 오매치 방지), URL WHATWG 충실+skip(C2), ASCII fold·JS-ws(C3), dynamic regex escape 하드닝(C4), verbatim 숫자
문자열, 매 회귀 mutation 검증. 관련: [[mutation-verify-regression-tests]], [[path-denylist-case-insensitive]],
[[suaegi-workflow]], [[subagent-output-untrusted]]
