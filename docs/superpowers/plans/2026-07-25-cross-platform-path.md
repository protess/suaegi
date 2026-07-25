# Plan — cross-platform-path (크로스플랫폼 경로 프리미티브) 확정

조사: `docs/superpowers/research/2026-07-25-cross-platform-path.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS**(8체크·two-predicate·컨테인먼트·`..` escape 전부 CONFIRMED,
정정 2 + 8질문 답변). 이 문서가 구현 계약이며 조사를 supersede한다. 인용은 별도 명시 없으면
`src/shared/cross-platform-path.ts`.

## 0. 결정 (조사 + Codex 확정)

Orca의 **보안-load-bearing 크로스플랫폼 경로 프리미티브**(컨테인먼트 검사 = path-escape 방어, Orca 렌더러
10+곳). 자기완결 hand-rolled(**node:path·import 0**, `.trim()` 0). 8 export. suaegi에 동종 프리미티브 부재
(기존 path 코드는 worktree/search 전용) = **net-new, 비-중복**.

**크레이트: 새 leaf `suaegi-path`** (deps 0, **regex 크레이트 미도입** — 전부 hand-roll char 검사, Codex Q5).
**Rust `std::path` 사용 금지**(플랫폼 고정 — posix/win32 동시 재현 불가; 결정 후 opaque 저장 타입으로만 허용).

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — posix+win32 hand-roll, `std::path` 금지, regex 크레이트 금지.** resolve/split-root/dots(`:39-49`,
  `:105-128`, `:130-155`) verbatim 이식. 모든 패턴(ASCII 드라이브 prefix, 선행 `//`/백슬래시, repeated-slash
  collapse, trailing-slash trim, WSL server match)은 byte/char 검사로 hand-roll.
- **C2 — Windows 판정 predicate **2개 별도**(병합 금지 = 즉시 escape).**
  - `is_windows_absolute_path_like`(`:1-3`, **start-anchored**): `/^[A-Za-z]:[\\/]/` **또는** 선행 `\\`(백슬래시
    **2개**) **또는** 선행 `//`. 쓰임: 비교 정규화(`:14`)·case-preserving candidate 정규화(`:72`).
  - `is_windows_path_flavor`(`:101-103`, **contains-`\`-anywhere**): `/^[A-Za-z]:[\\/]/` **또는** `\` **어디든**
    포함 **또는** 선행 `//`. 쓰임: `is_runtime_path_absolute` 기본 flavor(`:31`)·`resolve_runtime_path` flavor(`:40-41`).
  둘의 차이(2백슬래시 vs 1백슬래시-anywhere)가 핵심 — POSIX `/srv/team\repo`가 `abs_like`엔 false지만 `flavor`엔
  true. **정확히 분리 이식.**
- **C3 — 컨테인먼트 = 정규화-문자열 prefix match(relative 계산 아님).** `is_path_inside_or_equal`(`:59-68`):
  root·candidate를 `normalize_runtime_path_for_comparison`; `candidate == root` → true(OrEqual); 아니면
  `rootWithBoundary = (root == '/' || /^[a-z]:\/$/i) ? root : root.replace(/\/+$/,'') + '/'` 계산 후
  `candidate.starts_with(rootWithBoundary)`. **sibling-prefix 방어 = 이 `/`-boundary append**(`/repo/app` vs
  `/repo/application`), 드라이브/FS-root(`c:/`) 특수케이스. `relative_path_inside_root`(`:70-92`): 밖이면 **`null`**
  (Node `path.relative`의 `..` **아님**), 같으면 `''`.
- **C4 — `..`는 컨테인먼트에서 **미해결**(escape 벡터) → verbatim 보존 + caller 계약 문서화 + 회귀 핀.**
  두 컨테인먼트 함수 모두 `normalize_runtime_path_dots` **미호출** → 리터럴 `..` segment가 prefix-match됨. 즉
  `root=/safe/root`, `candidate=/safe/root/../outside`가 **inside로 판정**(오라클 미커버). **Orca 동작 그대로 보존**
  (legacy predicate). **호출부 계약(문서+doc comment): 컨테인먼트 호출 전 root·candidate 둘 다 pre-resolve
  필수.** 회귀 핀: (a) legacy predicate가 `/safe/root/../outside`를 여전히 accept함을 단언(동작 고정),
  (b) `resolve_runtime_path`로 pre-resolve하면 `/outside`가 되어 reject됨을 단언(안전 wrapper 패턴 증명).
- **C5 — case-fold = Unicode `to_lowercase()`; 비교-키와 case-preserving candidate **분리 유지**; 반환-suffix는
  JS UTF-16 code-unit 슬라이스 재현(byte-len-from-folded 금지, char-boundary-safe, panic-free).**
  - `normalize_runtime_path_for_comparison`(`:13-27`): separator 정규화 후 WSL은 **distro만** fold(`:20-24`),
    그 밖 Windows-abs-like는 **전체** `.to_lowercase()`(`:26`), POSIX는 fold 없음. **`to_ascii_lowercase` 금지**
    (JS `toLowerCase`는 Unicode — `İ→i̇` 길이변화). 정정: `ß→ss`는 틀림(JS는 `ß` 유지).
  - **`relative_path_inside_root` 반환-suffix(`:89-91`)의 함정**: WSL 분기는 `comparison_candidate.slice(prefix.len)`
    (둘 다 folded), 비-WSL 분기는 `normalized_candidate.slice(prefix.len)`(**folded prefix 길이로 case-preserving
    candidate를 슬라이스**). JS `.slice(N)`의 N = **UTF-16 code-unit** 수. Rust: `normalized_candidate.encode_utf16()`
    로 변환→앞 N 유닛 skip→재디코드. **N이 surrogate pair 중간이면**(비현실적) panic·silent-corruption 금지 →
    명시적 에러/lossy 처리 문서화. ASCII 경로(대다수)는 fold 길이-보존이라 N-유닛 슬라이스가 정확. **구조적 대안
    권장**: 정규화 중 root prefix 경계를 case-preserving 문자열에서 **직접 추적**해 folded-길이 역산 회피(Codex Q6).
- **C6 — `.trim()` 절대 추가 금지**(`line\nbreak` 경로가 리터럴 round-trip, 테스트 `:65-70`). UTF-16 `.length`
  슬라이스는 §C5 외엔 없음. FS canonicalize/symlink 해석 없음(순수 lexical — doc에 명시).

## 2. 마일스톤

### M1 — cross-platform-path 전체 (`suaegi-path` 신규, 단일 마일스톤)
8 export verbatim: `is_windows_absolute_path_like`(C2), `normalize_runtime_path_separators`(`:5-11`),
`normalize_runtime_path_for_comparison`(`:13-27`, C5 fold), `is_runtime_path_absolute`(`:29-37`),
`resolve_runtime_path`(`:39-49`, dots 해결), `get_runtime_path_basename`(`:51-57`, **flavor-agnostic 백슬래시
split** — 비교경로와 반대, 주의), `is_path_inside_or_equal`(`:59-68`, C3), `relative_path_inside_root`
(`:70-92`, C3+C5 suffix). 비-export helper: `is_windows_path_flavor`(C2), `normalize_runtime_path_dots`
(`:105-128`), `split_runtime_path_root`(`:130-155`, 드라이브/UNC/slash/relative root), `trim_runtime_path_trailing_slash`.

**오라클(7 `it`, 전부 이식):** 컨테인먼트(inside/equal/sibling-prefix/outside), Windows(드라이브/UNC/case), WSL,
`line\nbreak` round-trip, resolve dots.

**추가 핀(Codex, 오라클 미커버 — 전부 mutation 검증):**
- **C2 two-predicate:** `/srv/team\repo`가 `abs_like`=false·`flavor`=true(병합하면 발산); 판정 병합 mutation이 죽음.
- **C3 sibling-prefix:** `is_path_inside_or_equal('/repo/app','/repo/application')` → **false**(경계 `/` 없으면 escape).
- **C4 `..` gap:** `is_path_inside_or_equal('/safe/root','/safe/root/../outside')` → **true**(verbatim 보존 단언) +
  pre-resolve 후 `resolve_runtime_path` 통과시키면 `/outside`로 reject(안전 wrapper 증명).
- **C5 case-fold:** Windows 드라이브 경로 대문자 fold(`C:\Foo` vs `c:\foo` 동일 컨테인먼트); 비-ASCII fold(`İ`)에서
  suffix 슬라이스 panic 없음·UTF-16 재현; WSL distro-only fold(리눅스 하위 case 보존).
- **getRuntimePathBasename / normalizeRuntimePathSeparators / isWindowsAbsolutePathLike / normalizeRuntimePathDots**
  전부 오라클 미커버 → 직접 핀.
- **empty-root/relative-root**(Codex 추가): `is_path_inside_or_equal('', x)`·비-절대 root 동작 핀 또는 caller-금지 문서화.

*mutation:* two-predicate 병합, 경계-`/` 제거(sibling escape), `..` 해결 추가(C4 위반), `to_ascii_lowercase`로 교체,
suffix를 byte-len 슬라이스로(panic/발산), `std::path` 사용(플랫폼 발산), null↔`..` 반환 뒤바꿈.

## 3. Deferred (명시)
- **경로 소비자 배선**(렌더러 UI 10+곳) = 사람눈.
- **FS canonicalize/symlink 해석 없음** — 순수 lexical containment. 실제 심링크 escape 방어는 호출부(예: suaegi-git
  `resolve_in_worktree`)가 담당; 이 모듈은 lexical 계약만.
- **`..` pre-resolve는 caller 책임**(C4) — 이 모듈은 legacy lexical 동작 보존.

## 4. 순서 (확정)
M1 단일 마일스톤(8 export + helper + 오라클 + C2-C5 추가 핀 + C4 안전-wrapper 핀). 불변식: two-predicate 분리
(C2), 경계-`/` sibling 방어(C3), `..` verbatim+caller계약(C4), Unicode to_lowercase + UTF-16-safe suffix(C5),
std::path·regex 크레이트 금지(C1), 매 회귀 mutation 검증. 관련: [[mutation-verify-regression-tests]],
[[path-denylist-case-insensitive]], [[suaegi-workflow]], [[subagent-output-untrusted]]
