# Plan — work-naming (work/branch/workspace 이름 생성 클러스터) 확정

조사: `docs/superpowers/research/2026-07-25-work-naming.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS**(5모듈·toLowerCase 트랩·no-`/\s+/`·552 CONFIRMED, 정정 6 +
답변). 이 문서가 구현 계약이며 조사를 supersede한다.

## 0. 결정 (조사 + Codex 확정)

Orca의 work/branch/workspace **자동 이름·슬러그 생성** 클러스터(마지막 순수-자율 타겟). **5 모듈**:
`marine-creatures`(552-entry 데이터), `branch-name-from-work`, `display-name-from-work`, `workspace-name`,
`workspace-name-text-scanner`(숨은 5번째, 129L 무테스트). **`suaegi-workref` 의존**(`extract_work_identifier`+
`format_identifier_first`; `strip_work_identifier_echo` 미사용). 슬러그 문자열 처리 트랩 다수.

**크레이트: 새 leaf `suaegi-workname`** (deps: `suaegi-workref` + `regex`[워크스페이스]). `is_js_whitespace`는
workref 내부 private이므로 **이 크레이트에 로컬 복제**(15줄, 기존 4크레이트 동일 패턴 — workref pub API 변경 회피).

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — lowercase는 full `char::to_lowercase()`(NOT `to_ascii_lowercase`) — lowercase-then-ASCII-whitelist 순서.**
  `sanitizeBranchSlug`(`branch-name-from-work.ts:37-38`: `.toLowerCase().replace(/[^a-z0-9]+/g,'-')`),
  `slugifyForWorkspaceName`(`workspace-name.ts:24-30`), prefix 정규화가 **소문자 먼저→ASCII 화이트리스트**.
  JS `"İ".toLowerCase()`=`i̇`(i+U+0307)→ASCII `i` **생존**; Rust `to_ascii_lowercase`는 `İ` 그대로→whitelist가 제거
  =**틀림, 오라클 미커버**(ASCII 케이스만 테스트). → **scalar `char::to_lowercase()` flat_map 후 ASCII 필터.**
  **핀: `sanitize_branch_slug("İ")=="i"`, `sanitize_branch_slug("K")=="k"`(U+212A Kelvin), `slugify_for_workspace_name("İ K")=="i-k"`.**
  (ẞ/Σ/ﬁ는 ASCII survivor 無 = 차별 핀 아님.) creature 멤버십 셋은 전부 ASCII라 ascii-lower 무방하나 full이 소스-근접.
- **C2 — 아포스트로피 2함수/3 lookahead 치환은 hand-scan(Rust regex lookaround 없음), `\p{L}\p{N}`은 exact GC.**
  `removeIntraWordApostrophes`(`:13-15`: `/([\p{L}\p{N}])'(?=[\p{L}\p{N}])/gu`→`$1`),
  `stripDanglingDisplayApostrophes`(`:17-21`: 2 lookahead). U+2018/U+2019→ASCII `'` 정규화 후 scalar iterate(prev/cur/next).
  **`is_ln(c)` = General Category `L*|N*`** — `char::is_alphabetic()` **금지**(Other_Alphabetic 포함) → 컴파일된
  `Regex::new(r"^[\p{L}\p{N}]$")`로 char 분류(regex `\p{L}`/`\p{N}`은 exact GC). 규칙: intra-word=prev&next 둘 다
  is_ln이면 `'` 제거; dangling=(prev 없음/non-LN AND next LN) OR (prev LN AND next 없음/non-LN)이면 제거. 두 소스
  치환의 순차 결과를 재현. **핀: `'Hello`·`Hello'`·`('Hello')`·`rock'n'roll`·`''`·비-Latin L/N 이웃.**
- **C3 — no-`/\s+/` 계약(스파이): slug의 `replace(/\s+/)` + compaction의 `split(/\s+/)`만 금지(모든 `\s` 아님).**
  `workspace-name.test.ts:31-41`(replace 스파이)·`:235-251`(split 스파이). whitespace fold/scan은 hand-roll —
  scanner 집합(`workspace-name-text-scanner.ts:115-129`, ECMAScript ws: U+FEFF포함/U+0085제외)= `is_js_whitespace`.
  `foldWorkspaceNameWhitespaceToHyphen`/`collectCompactWorkspaceWords` verbatim hand-scan.
- **C4 — `\d`→`[0-9]`(12사이트), `\b`→`(?-u:\b)`(2사이트/4토큰). 유니코드 전역 비활성 금지(아포스트로피가 `\p{L}\p{N}` 요구).**
  사이트: branch `:27`; display `:35,:53`; workspace `:63,64,65,66,113×2,116,147,148`(9). `\b`: static `:66`, dynamic `:152`.
  dynamic regex `new RegExp(\`\\b[#!]?${item.number}\\b\`,'g')`(`:152`)는 **`regex::escape(number)`** + `(?-u:\b)`. 프로덕션
  `\w` 0건. ASCII-only action 패턴은 `(?i-u:...)`(유니코드 case-fold 서프라이즈 회피). 정적 화이트리스트 `[^a-z0-9._-]` 리터럴.
- **C5 — `MARINE_CREATURES` 552-entry verbatim**(ASCII 단일토큰, 무중복, 멤버십 recognition 셋 `branch-name:8`
  `Set(map(toLowerCase))`, 랜덤픽 소비자 없음). Rust `const [&str; 552]` 또는 `&[&str]`. 무테스트=데이터.
- **C6 — UTF-16 divergence는 좁게(lone-surrogate/`.length===1`/`[^:]{1,32}` astral만).** `humanizeBranchSlug`의
  `charAt(0)+slice(1)`은 surrogate 재조합(정상 문자열 divergence 없음). char-boundary-safe(panic 금지) 이식, 좁은
  divergence 문서화. `{1,32}` 등 반복 상한은 UTF-16 code-unit — char-scalar로 이식+문서화(입력 cap, 경계 아님).

## 2. 마일스톤

### M1 — work-naming 5모듈 (`suaegi-workname` 신규, 단일 마일스톤)
- `marine_creatures.rs`: 552 const(C5).
- `js_ws.rs`: `is_js_whitespace`/`js_trim` 로컬 복제(C3).
- `branch_name.rs`: `MAX_BRANCH_NAME_WORDS`, `is_auto_generated_creature_branch_name`(멤버십), `sanitize_branch_slug`
  (**C1 full lowercase**), `strip_configured_branch_prefix`, `humanize_branch_slug`(**C6 charAt+slice**), `build_branch_name_prompt`.
- `text_scanner.rs`: `fold_workspace_name_whitespace_to_hyphen`, `collect_compact_workspace_words`(**C3 hand-scan**).
- `workspace_name.rs`: `slugify_for_workspace_name`(**C1**), apostrophe helpers(**C2 hand-scan**),
  `get_linked_work_item_suggested_name`, `get_linked_work_item_workspace_name`, `get_workspace_intent_name`,
  `get_linear_issue_workspace_name`, `resolve_workspace_create_name`(**C4 dynamic regex escape**). workref 소비.
- `display_name.rs`: `derive_workspace_display_name`(workref extract+format + branch humanize).

**오라클(branch/display/workspace 3 테스트 전부 이식):** 4-word cap·creature collision suffix·prefix 정규화·custom
프롬프트(branch); identifier-first·collision·resolved-leaf fallback(display); slug 48-cap·apostrophe·replace/split 스파이·
work-item cleanup·intent·Linear dedup·**Japanese preserve**(workspace).

**추가 핀(Codex 미커버):** C1 `İ`/`K` 트랩(3핀), C2 dangling/intra 아포스트로피 엣지, C5 552 전량 동등, `collisionSuffixFromLeaf("fix","fix-007")→7`, C4 dynamic regex escape·`[0-9]` lock, no-`/\s+/` 스파이 동등(hand-roll 증명).

*mutation:* `to_ascii_lowercase`로(İ 트랩), `char::is_alphabetic`로(GC 부정확), `\d`/`\b` 유니코드, 552 누락, apostrophe
lookahead 규칙 반전, `/\s+/` 사용(스파이 위반), dynamic escape 제거, humanize charAt 경계.

## 3. Deferred (명시)
- **이름 생성 소비자 배선**(워크스페이스/브랜치 자동명명 UI) = 사람눈.
- UTF-16 정확재현(C6) — lone-surrogate/astral-repeat 좁은 divergence 수용(문서화).

## 4. 순서 (확정)
M1 단일 마일스톤(5모듈 + 552 데이터 + 오라클 3 + C1-C6 핀). 불변식: full to_lowercase(C1), GC-exact 아포스트로피
hand-scan(C2), no-`/\s+/` slug/compaction 계약(C3), `\d`/`\b` ASCII lock(C4), 552 verbatim(C5), char-boundary-safe(C6),
매 회귀 mutation 검증. 관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]], [[subagent-output-untrusted]]
