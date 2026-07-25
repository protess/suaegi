# Plan — gen-prompt (AI 커밋/PR 프롬프트+파싱 헬퍼) 확정

조사: `docs/superpowers/research/2026-07-25-gen-prompt.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **NEEDS-REWORK → 정정 반영**(핵심 CONFIRMED, 계약 4정정 + 7질문 답변). 이 문서가 구현
계약이며 조사를 supersede한다. 인용은 별도 명시 없으면 각 §의 모듈 파일.

## 0. 결정 (조사 + Codex 확정)

Orca coding-agent 루프의 **AI 커밋/PR 생성 프롬프트 빌드 + 응답 파싱** 순수 헬퍼. `vi.spyOn` 스파이는
**정규식 회피 강제**(impure mock 아님) — 모듈은 pure-in/pure-out. **4 모듈**(commit-message-agent-output
재export+오라클 커버라 제외 불가). fs/child_process/Date/crypto 0. `TuiAgent` 타입은 순수 함수 미소비 → drop.

**크레이트: 새 leaf `suaegi-gen-prompt`** (deps 0 — **regex 크레이트 미도입**, 스파이-가드 사이트뿐 아니라 전부
hand-roll해 Rust regex Unicode 발산 회피). 4 모듈 상호 결합(commit-message-generation·pull-request-generation이
commit-message-prompt를 import)이라 한 크레이트가 자연스러움.

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — truncateDiffForPrompt 단위 = 문서화된 divergence(Unicode scalar/char), 정확 UTF-16 재현 안 함.**
  Orca는 `String.length`/`.slice`/`.lastIndexOf`로 **UTF-16 code-unit** 측정(`commit-message-prompt.ts:60,68,78,
  118,126`, cut `:49,53,70,76-80`). 정확 UTF-16 재현은 (a) u16 인덱스 공간 유지, (b) surrogate 중간 cut, (c) lone
  surrogate 직렬화(U+FFFD)까지 필요 — **LLM 프롬프트 크기 휴리스틱(보안경계 아님)에 과함**. → **char-scalar 기반
  truncation**(알고리즘 구조는 동일: line-boundary cut `:76`, long-line hard cut, water-fill 공정배분 `:88-101`,
  marker). ASCII는 오라클과 identical; **non-ASCII는 문서화된 divergence + 핀**(한글/BMP/emoji/would-split-surrogate).
  **마커 문구 `"...bytes omitted"` 그대로 보존**(Orca의 역사적 misnomer — code-unit인데 "bytes"; 바꾸지 말 것).
  Rust 슬라이스는 char-boundary-safe(panic 금지).
- **C2 — 마커 계약(edge case 정확, "항상 append" 아님).** (a) 미절단(`section.length <= limit` `:60`)→마커 無;
  (b) 절단+`limit <= 0`(`:63-65`)→**빈 문자열, 마커 無**; (c) water-fill에서 `share === 0`(`:92-95`)→해당 섹션
  0-budget→**무마커 생략**; (d) positive지만 `marker.length >= limit`(`:69-70`)→**마커 prefix만**(`marker.slice(0,limit)`);
  (e) 그 외→완전 마커 + marker-length 재-clip(`:80` `min(cut, max(0, limit - marker.length))`). 전부 verbatim.
- **C3 — `.replace('{{DIFF}}', diff)` `$` quirk **verbatim 재현**(preserved Orca 결함, task-query C3 선례).**
  JS `String.replace(searchString, replaceString)`는 replacement(`diff`)의 `$$`→`$`, `$&`→매치(`{{DIFF}}`),
  `` $` ``→prefix, `$'`→suffix를 특수치환(`:28`). **`$n`은 캡처 그룹 없어 리터럴**(Codex 정정). → single literal-search
  replace를 위한 JS 치환-패턴 시맨틱 hand-roll(`$$`/`$&`/`` $` ``/`$'` 4종만). 핀: `$&` 포함 diff가 `{{DIFF}}`로
  치환됨 + 평범 diff. 주석: "의도적 보존 Orca quirk — literal이 아마 의도였으나 unilateral divergence 금지".
- **C4 — parseGeneratedPullRequestFields → `Result<Fields, ParseError>`(배열은 에러 아님).**
  (`pull-request-generation.ts:155-173`) malformed JSON→`Err(InvalidJson)`(JS SyntaxError), `!parsed || typeof
  !== 'object'`(null/number/string/bool)→`Err(NotObject)`(`:156-158`), **배열 `[]`→`typeof==='object'`이라 throw
  안 함 → `Ok(fallback)`**(Codex 정정, `[]` 핀은 fallback으로). valid-but-missing→fallback(base/currentTitle.trim/
  currentBody/currentDraft), title 최종 default `'Update project files'`(`:169-173`). **throw 경로 2종 오라클 미커버
  → 추가 핀.**
- **C5 — whitespace/case-fold JS-faithful.** `.trim()`/`/\s+$/`/`/[.]+$/` 등은 JS 집합 — 재사용 `is_js_whitespace`
  술어(suaegi-search/taskquery 선례, U+FEFF포함/U+0085제외)로 hand-roll. `startsWithAsciiIgnoreCase`
  (`pull-request-generation.ts:137`)는 **의도적 ASCII-only** → `to_ascii_lowercase`/`eq_ignore_ascii_case`. subject
  `.slice(0,72)`는 **char-boundary-safe**(surrogate/멀티바이트 cut panic 금지). non-스파이-가드 정규식
  (list-marker `cmao:27`, preamble `.test` `:15`, trailing-dot/ws)도 전부 hand-roll(regex 크레이트 미도입).
- **C6 — 정규식 회피(스파이 강제 사이트).** CRLF 정규화·fence-body 추출·`split('\n')` 금지는 오라클 스파이가
  강제(`commit-message-prompt.test.ts:117-124`, `pull-request-generation.test.ts:73-83`,
  `commit-message-generation.test.ts:78-81`) → 반드시 hand-roll(char 스캔). 나머지도 C5대로 hand-roll.

## 2. 마일스톤

### M1 — gen-prompt 4모듈 (`suaegi-gen-prompt` 신규, 단일 마일스톤)
- `commit-message-agent-output.rs`: `clean_generated_commit_message`(CRLF hand-roll 정규화·fence-body 추출·
  list-marker strip·trailing-dot), `excerpt_agent_failure_output`(양끝 excerpt·ANSI strip). 하드 dep이므로 먼저.
- `commit_message_prompt.rs`: `build_commit_prompt`(**$ quirk C3**), 상수 `STAGED_DIFF_BYTE_BUDGET=200_000`,
  `truncate_diff_for_prompt`(C1 char-scalar + C2 마커 + water-fill 공정배분), `tokenize_custom_command_template`
  /`plan_custom_command`(`{prompt}` 플레이스홀더 상태기계), 재export.
- `commit_message_generation.rs`: `split_generated_commit_message`(clean 후 첫 개행 분리, `split('\n')` 금지 C6),
  `build_commit_message_prompt`(truncate 사용).
- `pull_request_generation.rs`: `build_pull_request_fields_prompt`(truncate 사용), `parse_generated_pull_request_fields`
  (**Result C4**), `starts_with_ascii_ignore_case`(C5).

**오라클(53 = 43+5+5, 전부 이식):** commit-message-prompt.test.ts(build 3·truncate 5·clean 8·excerpt 14·tokenize 7·
plan 6), pull-request-generation.test.ts 5, commit-message-generation.test.ts 5.

**추가 핀(Codex, 오라클 미커버):**
- **C1 non-ASCII truncation divergence:** 한글/emoji 대용량 diff에서 char-scalar cut(문서화된 Orca 대비 divergence)
  + would-split-surrogate에서 panic 없음.
- **C2 마커 edge:** budget≤0→빈; positive<marker→마커prefix; 0-budget 섹션 생략; 완전마커 재-clip.
- **C3 `$` quirk:** `$&` 포함 diff → `{{DIFF}}` 치환 재현.
- **C4 parse throw:** malformed JSON→`Err(InvalidJson)`; `null`/`42`/`"s"`/`true`→`Err(NotObject)`; **`[]`→`Ok(fallback)`**;
  missing-field→fallback + `'Update project files'` default.
- **C5:** `startsWithAsciiIgnoreCase` ASCII-only(비-ASCII case 미폴드); subject 72-cut char-boundary-safe.

*mutation:* truncate 단위/water-fill 배분, 마커 edge 각각, `$` quirk 제거(literal), parse throw→Ok(empty) (대죄:
garbage를 성공으로), ASCII-fold→Unicode, split('\n') 사용(스파이 위반), char-boundary cut 제거(panic).

## 3. Deferred (명시)
- **AI 생성 UI 배선**(커밋/PR 생성 트리거·프리뷰) = 사람눈.
- **truncate UTF-16 정확재현**(C1) — LLM 프롬프트 휴리스틱이라 char-scalar divergence 수용(문서화). 후일 정확
  필요 시 u16-버퍼 + U+FFFD 직렬화 별건.
- **`$` quirk literal 수정**(C3) — Orca+포트 동시 승인 behavior-change로만(지금은 verbatim 보존).

## 4. 순서 (확정)
M1 단일 마일스톤(4모듈 + 오라클 53 + C1-C5 추가 핀). 불변식: char-scalar truncate 문서화 divergence(C1), 마커
edge 정확(C2), `$` quirk verbatim(C3), parse throw→Result·배열≠에러(C4, transient≠garbage), ASCII-fold 의도적(C5),
정규식 회피 hand-roll(C6), regex 크레이트 금지, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]], [[subagent-output-untrusted]]
