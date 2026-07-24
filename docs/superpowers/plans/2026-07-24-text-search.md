# Plan — text-search (repo content-search 백엔드: rg + git-grep) 확정

조사: `docs/superpowers/research/2026-07-24-text-search.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **NEEDS-REWORK → 정정 반영**(인용 전부 정확, 결정-임계 정정 2건 + 4개 결정질문
답변). 이 문서가 구현 계약이며 조사 문서를 supersede한다. 인용은 별도 명시 없으면
`src/shared/text-search.ts`.

## 0. 결정 (조사 + Codex 확정)

Orca 콘텐츠-검색(파일 내용 grep) 백엔드. 이미 포팅된 Quick Open(파일명 퍼지 스코어)의 **콘텐츠-검색
짝**. 순수 모듈(argv 빌더 + 스트림 파서)은 IO 미접촉 — spawn/kill/timeout은 드라이버 소유. rg(`--json`)
주경로 + git-grep(`--null`) 폴백(rg 미설치 시). 보안·정확도 stakes 높음(argv-injection, silent
truncation = 저장소 대죄).

**크레이트: 새 `suaegi-search` (Codex Q8).** 두 peer 백엔드(rg/git-grep)라 본질이 git 기능 아님.
deps: `serde`+`serde_json`(rg JSON), `tokio`(드라이버 spawn), `regex`(submatch 위치 재스캔).
suaegi-git엔 `serde_json` 없음 → 거기 두면 잘못된 의존 경계. `suaegi-git`은 generic git 실행 facility만
노출(git-grep 드라이버가 재사용). rg 드라이버는 `quick_open.rs`의 tokio Command·transient≠empty 규율 재사용.

## 1. Codex 반영 정정 (구현자 필독)

- **C1 — byte-offset은 canonical 소스 좌표로 보존, `&str` 슬라이싱은 char-boundary 방어(대죄+panic).**
  rg `--json` submatch `start`/`end`는 **UTF-8 byte offset**(Codex 실측: `é`가 2바이트라 `start:2`).
  Orca는 이를 UTF-16 string index로 오용(`:83-88`,`:98`) = latent 버그(오라클 ASCII-only라 미노출).
  Rust 계약: (a) `column = start_byte + 1`, `match_length = end_byte - start_byte` **보존**(관측 가능한
  숫자 계약 유지, char index 변환 **금지** — 계약 변경됨); (b) `start <= end <= line.len()` 검증;
  (c) 슬라이싱 전 `is_char_boundary` 확인, 불량이면 인접 valid boundary로 clamp 또는 whole-line 폴백
  — **절대 panic·매치 드롭 금지**; (d) 500-code-unit 윈도가 멀티바이트 스칼라 안에 떨어지면 문서화된
  정책으로 start 뒤로/end 앞으로 이동; (e) display 좌표는 안전한 렌더 스니펫 기준 별도 계산.
  **Orca의 완전한 Unicode 윈도잉 충실도는 재현 안 함**(원본이 결함) — byte 소스 좌표 보존 + 렌더 경계만
  안전 수리. **non-ASCII 픽스처(`éx`, CJK, emoji) 추가 후 freeze.**
- **C2 — `null`은 "JS regex 컴파일 실패"일 뿐(git 수용 여부 무관).** 케이스 24는 git-grep이 `(foo`/`[abc`를
  수용하는데 JS가 거부함을 **증명 안 함**(테스트는 git 미실행, 실 Git 2.50.1도 exit 128 거부). 정확한 계약:
  `buildSubmatchRegex`가 `None` 반환 = **JS/Rust regex 컴파일 실패** → git-confirmed 라인이 `None`으로
  인제스트되면 whole-line 하이라이트(`:417-421`,`:438-445`). git 수용성과 결부시키지 말 것.
- **C3 — `regex` 크레이트는 best-effort submatch locator(JS/ERE 검증기 아님).** 추가하되: 컴파일 실패→
  `None`/whole-line; case-insensitive; 전체 매치 안전 순회; **zero-length 매치 무한루프 방지**(`lastIndex++`
  대응 = 매 반복 최소 1 byte/char 전진); `\b` 지원. **JS RegExp 정확 재현 불가**(backref/lookaround 없음,
  거부 집합 다름, `\b` 유니코드 시맨틱 다름) — 오라클 케이스 24는 Rust `regex`에서도 우연히 `None`이나
  그건 divergence 증거 아님. fixed-string 모드는 **수동 리터럴 스캔**(regex 우회), regex 모드만 `regex`.
  regex 모드 전체를 whole-line 폴백하면 Orca와 크게 divergence — regex 모드엔 엔진 필수.
- **C4 — per-file 캡 비대칭 verbatim.** rg만 `--max-count 100`(`:189-190`), git-grep은 per-file 캡 **없음**
  (§3.1, `:297-342`에 플래그 부재). 통일하면 result count·file 분포가 관측 divergence → **rg-only 유지**
  (오라클 충실). 후일 통일은 별도 승인 behavior-change + 교차-백엔드 테스트로.
- **C5 — truncated 불변식(대죄 핵심), early-stop 브랜치도 세팅.** `pushMatch`가 `truncated=true`+`return 'stop'`
  연속(`:130-131`). **단 ingest 진입의 `totalMatches>=maxResults` early-`stop`(`:232-234`,`:373-375`)은
  truncated 세팅 안 함** — 이건 정정: Rust는 (i) accumulator 불변식으로 `total>=max ⇒ truncated` 강제
  하거나 (ii) **early-stop 브랜치에도 `truncated=true`** 세팅(문서화된 correction). 드라이버: Stop 수신→
  mutated accumulator 유지→child kill/reap→finalize. **타임아웃(15s)도 kill 직전 truncated=true**
  (`filesystem.ts:1018`,`filesystem-search-git.ts:92`). transient/timeout을 완전 empty로 위장 금지
  (quick_open transient≠empty 계열).
- **C6 — empty-submatch 폴백은 byte-safe하게.** `submatches.length===0 → [{start:0, end: len>0?1:0}]`
  (`:265-268`)에서 **`end=1` 리터럴은 non-ASCII 첫 문자에서 byte-boundary로 취급 시 불안전** → Rust:
  `end = if line.is_empty() {0} else { 첫 UTF-8 스칼라의 byte 길이 }`. 의도(navigable 첫 문자 1개) 보존 +
  valid UTF-8 boundary. **의도적 safety correction — 호환 테스트로 문서화.** emptiness 판정은 byte-len/
  scalar-count/UTF-16-len 모두 일치하므로 `line.is_empty()`로 충분(케이스 11·12).

## 2. 마일스톤

### M1 — 순수 기반 (`suaegi-search` 신규, 전부 unit/mutation)
types(`SearchMatch`/`SearchFileResult`/`SearchResult`/`SearchOptions` — `types.ts:3543-3574`), 상수
(`MAX_MATCHES_PER_FILE=100`,`DEFAULT_SEARCH_MAX_RESULTS=2000`,`SEARCH_TIMEOUT_MS=15000`,
`MAX_LINE_CONTENT_LENGTH=500`,`SEARCH_MAX_FILE_SIZE=5MB`,`TRUNCATION_MARKER='…'`), `normalize_relative_path`
(`:33-35`, 세퍼레이터 런→단일 `/`, 선행 `/` 제거, **수동 문자열 연산 — `std::path` 금지**[실행-플랫폼 종속]),
`split_search_glob_patterns`(`:143-175`, **escape 상태기계 C-정정 없음, verbatim**: `\`+다음문자 원형보존,
미이스케이프 `,`만 분할, 후행 단독 `\` 보존, 빈 조각 drop, `chars()` 순회), `escape_regex`
(`string-utils.ts:10-12`, 메타 **14종** — `[.*+?^${}()|[\]\\]` 전개), `normalize_search_result`/`normalize_search_file_match_count`
(`search-match-count.ts:3-30`, `max(matchCount, matches.len)` 하한 보정). *오라클:* 케이스 1(경로 정규화),
6·7(glob escape: escaped-comma·trailing-backslash), 37·38(matchCount 보정·빈파일 필터). *mutation:* 세퍼레이터
접기, escape 상태 전이, 빈조각 drop, matchCount 하한.

### M2 — argv 빌더 + submatch regex (`suaegi-search`)
`build_rg_args`(`:183-215`, 고정 순서 §2 표 + 조건부, **`--`,query,target 종결자 C-대죄가드**),
`build_git_grep_args`(`:297-342`, 프리앰블 + `-e query --` 종결자, **per-file 캡 없음 C4**),
`to_git_glob_pathspec`(`:291-295`, `/`없으면 `**/` 재귀, exclude prefix), `build_submatch_regex`
(`:351-364`, **C2/C3: `regex` 크레이트 best-effort, 컴파일 실패→None, fixed-string은 수동 리터럴 스캔
경로라 여기선 regex 모드용, `\b` wholeWord, `g`/`i` 대응**). *오라클:* 2·3·4·5(rg argv + **`slice(-3)==['--',q,target]`
injection 핀**), 16·17·18·19·20(git argv + `at(-1)=='.'` 기본 pathspec + pathspec), 21·22·23·24(submatch regex:
escapeRegex·wholeWord·useRegex·**None**[C2: JS/Rust 컴파일 실패, git 수용 무관]). **추가 핀(Codex Q1):** 쿼리
`-e`/`--help`/`--`, 타깃 `-`시작 → argv 벡터가 여전히 안전. *mutation:* `--` 종결자 제거/위치, 재귀 prefix,
None 폴백, regex 플래그.

### M3 — 스트림 파서 + accumulator (`suaegi-search`, byte-safe 대죄 표면)
`SearchAccumulator`+`create_accumulator`(`:18-26`), `clamp_line_context`(`:65-103`, **C1: byte 소스 좌표
보존 + char-boundary 방어 슬라이싱 + 별도 display 좌표, 500=UTF-16-code-unit 상당이나 Rust는 측정 단위
명시**), `push_match`(`:106-134`, **C5 truncated 불변식**), `ingest_rg_json_line`(`:225-282`, **serde_json
tolerant 역직렬화 Q6**: unknown 무시, `type=='match'`+textual path 요구, lines 기본 `""`, line_number 기본 0,
submatches 기본 empty→**C6 byte-safe 폴백**, malformed JSON 스킵 stream 유지 `:247-250`), `ingest_git_grep_line`
(`:366-447`, `filename\0lineno\0content` + **구버전 콜론 폴백**[Q7a: proven-necessary 단정 금지, 둘 다 유지],
submatch regex 재스캔 + zero-length 전진 C3 + **git-confirmed-무매치 whole-line 폴백 `:438-445` 드롭금지**),
`finalize`(`:451-457`). *오라클:* 8~15(rg ingest: 기본매핑·non-match·malformed·**empty-submatch 폴백**·
**총량캡+truncated**·**라인클램프 참값vs표시값**·WSL transform), 26~38(git ingest: 다중위치·콜론폴백·
파일명콜론·malformed·zero-length·캡·null폴백·git-confirmed폴백·finalize). **케이스 25(impure git shell-out
`test.ts:268-306`)는 녹화 픽스처로 대체**(git 버전 함께 기록 Q7a). **추가 non-ASCII 픽스처(C1):** `éx`/CJK/emoji로
byte-boundary 방어·컬럼 값 검증. *mutation:* 폴백 분기 각각, 캡 `>=` 경계, truncated 세팅, char-boundary 방어,
clamp 윈도 산술.

### M4 — 드라이버 (rg tokio spawn + git-grep runner, transient≠empty)
rg 드라이버: `quick_open.rs` 패턴 재사용(tokio `Command`, streamed stdout 라인, `SEARCH_TIMEOUT_MS` timeout,
명시 kill/reap). git-grep 드라이버: `suaegi-git` generic git 실행 facility 경유(**streaming 노출 필요 —
whole-output 버퍼링이면 좁게 확장하거나 suaegi-search가 process 프리미티브 직접 사용**, streaming/truncation
희생 금지). 백엔드 선택(rg 우선, 미설치→git-grep). **C5 truncated/timeout 순서 + transient≠empty**(spawn 실패·
timeout·kill을 완전 empty·성공으로 위장 금지 — 저장소 대죄). *AV:* 실 rg/git tempdir repo 왕복(스테이지→검색→
확인) + 케이스 25 녹화 픽스처. *crux:* rg-미설치→git 폴백이 절대 empty 위장 안 함, timeout=truncated, 캡→kill.
*리스크:* 드라이버 impure라 순수 core(M1-M3)보다 mutation 표면 작음 — transient≠empty·truncated 순서에 집중.

## 3. Deferred (명시)
- **검색 UI**(검색 패널 위젯·결과 트리·하이라이트·네비) = 사람눈.
- **완전한 Unicode 윈도잉 충실도**(C1) — Orca 원본이 byte-as-UTF16-index 결함이라 재현 안 함. Rust는 byte
  소스 좌표 보존 + 렌더 안전만. non-ASCII에서 Orca와 display가 미세 divergence 가능(문서화 수용).
- **per-file 캡 통일**(C4) — 별도 승인 behavior-change.
- WSL 경로 번역은 `transformAbsPath` 훅 시그니처만(로컬 None, 릴레이 사람눈).

## 4. 순서 (확정)
M1 순수 기반 → M2 argv+submatch regex → M3 스트림 파서(byte-safe) → M4 드라이버(transient≠empty).
불변식: argv-injection `--` 종결자(대죄 가드), silent truncation 금지(truncated 불변식 C5), transient≠empty,
byte-boundary panic 금지(C1), 매 회귀 mutation 검증. 관련: [[mutation-verify-regression-tests]],
[[suaegi-workflow]], [[subagent-output-untrusted]]
