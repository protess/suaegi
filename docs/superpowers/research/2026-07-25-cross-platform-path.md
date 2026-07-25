> **⚠️ Codex 교차검증 정정 (VALIDATED-WITH-CORRECTIONS):** 인용·two-predicate split·컨테인먼트·`..`-미해결
> escape 전부 CONFIRMED. 정정: (1) case-fold 예시 `ß → ss`는 **틀림**(JS `toLowerCase`는 `ß` 유지); 길이변화
> 예는 `İ → i̇`. (2) `relativePathInsideRoot`(:89-91) 반환-suffix는 folded-키 길이로 case-preserving를 슬라이스
> 금지 → **JS UTF-16 code-unit 슬라이스를 char-boundary-safe·panic-free 재현**. case-fold=Unicode `to_lowercase()`.
> **최종 계약은 플랜 `docs/superpowers/plans/2026-07-25-cross-platform-path.md`가 supersede.**

# Research: `cross-platform-path` — Rust 포팅 계약서

**대상**: Orca `src/shared/cross-platform-path.ts` (155L) + `.test.ts` (88L) @ v1.4.150-rc.0
**성격**: **보안 하중 부담(security-load-bearing)**. `isPathInsideOrEqual` / `relativePathInsideRoot` 는 **컨테인먼트(containment) 판정** — 버그 = path-escape = 이 저장소에서 `.git` denylist RCE 를 냈던 바로 그 클래스.
**결론 요약(먼저 읽을 것)**:
1. 이 모듈은 **`node:path` 를 import 하지 않는다** — 전부 정규식/문자열로 hand-roll. (`grep import|require` = 0건, `.trim()` = 0건.) 그래서 "Node path 의미 재현" 부담은 없지만, **Rust `std::path` 는 플랫폼 고정이라 여기에 절대 쓰면 안 된다** — posix/win32 의미를 우리가 직접 hand-roll 해야 함. 소스가 이미 hand-roll 이므로 그 로직을 **verbatim** 옮기는 것이 계약.
2. 컨테인먼트는 **상대경로 계산이 아니라 정규화된 문자열 prefix 매칭**으로 구현된다. sibling-prefix 방어(`/repo/app` vs `/repo/application`)는 오직 root 끝에 `/` 경계를 붙이는 것으로만 이뤄진다(:65-67). 이 경계 로직이 보안의 전부다.
3. **두 개의 서로 다른 "Windows 판정" 술어**가 존재하고 의미가 다르다 — 이걸 하나로 합치면 즉시 취약점. `isWindowsAbsolutePathLike`(:1, **시작 앵커**)와 `isWindowsPathFlavor`(:101, **중간 포함**). 아래 트랩 클래스 §T1 참조.
4. Windows 만 case-fold(`toLowerCase`), POSIX 는 case-sensitive 유지. POSIX 에서 backslash 는 **정규 파일명 문자**로 보존(:15-18). WSL UNC 는 distro 부분만 fold, 나머지 Linux 경로는 case 보존(:20-26).

---

## 0. 파일 구조 / import

- 모듈: `cross-platform-path.ts` — top-level import 없음. 순수 함수 8개 export + 비-export 헬퍼 4개. 전부 pure(부수효과 없음, `process.cwd()` 등 미참조 — 테스트 이름이 "without using the process cwd" :73).
- 오라클: `cross-platform-path.test.ts:1` `import { describe, expect, it } from 'vitest'`, `:2-8` 대상 모듈에서 5개 함수 import(`isPathInsideOrEqual`, `isRuntimePathAbsolute`, `normalizeRuntimePathForComparison`, `relativePathInsideRoot`, `resolveRuntimePath`). **`getRuntimePathBasename`, `normalizeRuntimePathSeparators`, `isWindowsAbsolutePathLike` 는 오라클이 직접 호출하지 않음** → 별도 pin 필요(§오라클/미커버).

---

## 1. Public surface (export 8개)

### 1.1 `isWindowsAbsolutePathLike(value: string): boolean` — :1-3
- **signature/return/purity**: `(string) -> bool`, pure.
- **알고리즘**(:2, 단일 return):
  `/^[A-Za-z]:[\\/]/.test(value)` — 드라이브 문자 + **직후 구분자**(`\` 또는 `/`). `C:\` / `C:/` 매치. **`C:`(구분자 없음)은 매치 안 함**.
  `|| value.startsWith('\\\\')` — JS 리터럴 `'\\\\'` = 실제 문자 두 개 `\\` (UNC 시작).
  `|| value.startsWith('//')` — 슬래시 두 개.
- **용도**: `normalizeRuntimePathForComparison`(:14), `relativePathInsideRoot` 후보 정규화(:72). 즉 **"이 경로가 Windows 절대경로/UNC 처럼 보이는가"** — **시작 앵커** 판정. 중간 backslash 는 무시(POSIX backslash 보존의 근거).

### 1.2 `normalizeRuntimePathSeparators(value: string): string` — :5-11
- `(string) -> string`, pure. Windows 구분자 통일 + UNC 이중 슬래시 보존.
- **알고리즘**:
  - :6 `normalized = value.replace(/\\/g, '/').replace(/\/+/g, '/')` — 모든 backslash→`/`, 그 다음 `/` 연속을 하나로 collapse.
  - :7 원본(`value`, normalized 아님)이 `'\\\\'`(두 backslash) 또는 `'//'` 로 시작하면 → :8 `` `//${normalized.replace(/^\/+/, '')}` `` (선행 슬래시 제거 후 `//` 재부착 = collapse 로 사라진 UNC 이중 슬래시 복원).
  - :10 그 외 `normalized` 반환.
- **주의**: UNC 판정은 **원본** startsWith 로 함. collapse 는 `//foo` → `/foo` 로 만들지만 UNC branch 가 `//` 를 되살림.

### 1.3 `normalizeRuntimePathForComparison(value: string): string` — :13-27 (컨테인먼트의 정규화 코어)
- `(string) -> string`(비교 키), pure. **컨테인먼트 판정의 정규화 규칙 전부가 여기 있음.**
- **알고리즘**:
  - :14 `isWindowsPath = isWindowsAbsolutePathLike(value)` — **시작-앵커** 술어(§1.1).
  - :17-19 `normalized = trimRuntimePathTrailingSlash( isWindowsPath ? normalizeRuntimePathSeparators(value) : value.replace(/\/+/g, '/') )`
    - Windows: 전체 구분자 정규화(backslash→`/`, collapse, UNC 보존).
    - POSIX: **`/` 연속 collapse 만**. backslash 는 **손대지 않음 → 리터럴 파일명 문자로 보존**(:15-16 주석 근거). 그다음 trailing slash trim.
  - :20 `wslUnc = normalized.match(/^\/\/(?:wsl\.localhost|wsl\$)\/([^/]+)(\/[\s\S]*)?$/i)` — **`i` 플래그(case-insensitive)**. `//wsl.localhost/<distro>` 또는 `//wsl$/<distro>` (+ 선택적 `/rest`). group1 = distro, group2 = `/rest`(선행 슬래시 포함) 또는 `undefined`. `[\s\S]` = **개행 포함 모든 문자**.
  - :24 WSL 매치 시 `` return `//wsl/${wslUnc[1].toLowerCase()}${wslUnc[2] ?? ''}` `` — 두 UNC alias 를 canonical `//wsl/` 로 fold, **distro 만 lowercase**, **나머지(rest)는 verbatim 보존(case-sensitive Linux 경로)**.
  - :26 그 외 `return isWindowsPath ? normalized.toLowerCase() : normalized` — Windows → **전체 lowercase**(case-insensitive), POSIX → **원형 유지**(case-sensitive).
- **트랩**: `.toLowerCase()`(§T3). POSIX/Windows 분기(§T1). WSL fold(§T4).

### 1.4 `isRuntimePathAbsolute(value, pathFlavor?): boolean` — :29-37
- `(string, 'posix'|'windows'=auto) -> bool`, pure.
- :31 **기본 인자** `pathFlavor = isWindowsPathFlavor(value) ? 'windows' : 'posix'` — **`isWindowsPathFlavor`(중간 포함 술어, §1.11/§T1)** 로 자동 감지. 호출자가 명시하면 그 값 사용.
- :33-34 windows: `/^[A-Za-z]:[\\/]/.test(value) || value.startsWith('\\') || value.startsWith('/')` — 드라이브+구분자, 또는 **단일 `\` 시작**, 또는 **단일 `/` 시작**.
- :36 posix: `value.startsWith('/')`.
- **주의**: windows 는 `//`/`\\` 뿐 아니라 **단일** backslash/slash 시작도 절대로 취급. 드라이브는 콜론 뒤 구분자 필수.

### 1.5 `resolveRuntimePath(basePath, targetPath): string` — :39-49
- `(string, string) -> string`, pure. `process.cwd()` 미사용(테스트 :73 근거).
- :40-41 `pathFlavor = isWindowsPathFlavor(basePath) || isWindowsPathFlavor(targetPath) ? 'windows' : 'posix'` — **둘 중 하나라도** Windows-flavor 면 windows. (**포함 술어**, §T1.)
- :42-43 `isRuntimePathAbsolute(targetPath, pathFlavor)` 참이면 → `normalizeRuntimePathDots(targetPath, pathFlavor)` (**절대 target 우선, base 무시**).
- :45-48 아니면 `` normalizeRuntimePathDots(`${trimRuntimePathTrailingSlash(normalizeRuntimePathSeparators(basePath))}/${targetPath}`, pathFlavor) `` — base(구분자 정규화+trailing trim) + `/` + target 문자열 결합 후 dot 해석.
- **주의**: 결과는 **case 보존**(resolve 는 lowercase 안 함 — 오직 comparison 만 fold). 출력 구분자는 `/`. 테스트 :82 `..\worktrees\feature` → `C:/Repos/app/worktrees/feature`(`Repos` 대문자 유지).

### 1.6 `getRuntimePathBasename(value: string): string` — :51-57
- `(string) -> string`, pure.
- :52 `trimmed = value.replace(/[\\/]+$/g, '')` — 후행 slash/backslash 제거.
- :53-55 빈 문자열이면 `''`.
- :56 `trimmed.split(/[\\/]/).findLast(Boolean) ?? ''` — **slash/backslash 양쪽으로 split**, 마지막 truthy(비어있지 않은) 세그먼트. `findLast(Boolean)` 이 `//` 로 생긴 빈 세그먼트 skip.
- **분기 주의**: 이 함수는 **flavor 무관하게 항상 backslash 를 구분자로 split** — comparison 계열이 POSIX backslash 를 보존하는 것과 **정반대**. POSIX `team\repo` 를 여기 넣으면 basename 은 `repo` 가 됨. 포팅 시 이 비대칭을 그대로 옮길 것. **오라클 미커버 → pin 필요.**

### 1.7 `isPathInsideOrEqual(rootPath, candidatePath): boolean` — :59-68 ★보안 핵심★
- `(string, string) -> bool`, pure. "candidate 가 root 안(또는 같음)인가".
- **알고리즘**:
  - :60-61 `root = normalizeRuntimePathForComparison(rootPath)`, `candidate = normalizeRuntimePathForComparison(candidatePath)` — 양쪽 동일 정규화(case-fold/구분자/WSL 처리 포함).
  - :62-64 `candidate === root` → `true` (**"OrEqual" = equal 은 inside**).
  - :65-66 `rootWithBoundary = root === '/' || /^[a-z]:\/$/i.test(root) ? root : `${root.replace(/\/+$/, '')}/`` — root 가 파일시스템 루트(`/`) 또는 **드라이브 루트(`c:/`, `i` 플래그)**면 경계 = root 그대로(이미 `/` 로 끝남); 아니면 후행 슬래시 제거 후 **단일 `/` 부착**.
  - :67 `return candidate.startsWith(rootWithBoundary)`.
- **보안 크럭스**: sibling-prefix 방어(`/repo/app` → 경계 `/repo/app/`)는 **오직 이 `/` 부착**으로만 성립. `/repo/application` 은 `/repo/app/` 로 startsWith 안 함 → false(테스트 :14). 드라이브 루트 특례가 없으면 `c:/` 가 `c://` 가 되어 후속 매칭이 깨짐. **트랩: `.startsWith` = UTF-16 코드유닛 prefix(§T5).**

### 1.8 `relativePathInsideRoot(rootPath, candidatePath): string | null` — :70-92 ★보안 핵심★
- `(string, string) -> string | null`, pure. inside 면 root 기준 상대 경로 문자열, 아니면 `null`(equal 이면 `''`).
- **알고리즘**:
  - :71-75 `normalizedCandidate = trimRuntimePathTrailingSlash( isWindowsAbsolutePathLike(candidatePath) ? normalizeRuntimePathSeparators(candidatePath) : candidatePath.replace(/\/+/g, '/') )` — **case 보존** 후보(반환 suffix 원본). Windows→구분자 정규화(**lowercase 안 함**), POSIX→`/` collapse 만. **(§1.1 시작-앵커 술어 사용.)**
  - :76-77 `comparisonRoot = normalizeRuntimePathForComparison(rootPath)`, `comparisonCandidate = normalizeRuntimePathForComparison(candidatePath)` — case-fold 비교 키.
  - :79-81 `comparisonCandidate === comparisonRoot` → `''` (equal → 빈 상대경로).
  - :82 `isRoot = comparisonRoot === '/' || /^[a-z]:\/$/i.test(comparisonRoot)`.
  - :83 `comparisonPrefix = isRoot ? comparisonRoot : `${comparisonRoot}/`` (**§1.7 과 동일한 `/` 경계 로직**).
  - :84-86 `!comparisonCandidate.startsWith(comparisonPrefix)` → `null`.
  - :89-91 반환: `comparisonRoot.startsWith('//wsl/')` 면 `comparisonCandidate.slice(comparisonPrefix.length)`(WSL: fold 된 comparison 에서 잘라야 정렬됨), 아니면 `normalizedCandidate.slice(comparisonPrefix.length)`(**case 보존 원본에서 잘라 원래 대소문자 유지**).
- **보안 크럭스**: `comparisonPrefix.length`(비교형 길이)로 계산한 인덱스를 `normalizedCandidate`(원본형)에 slice. **길이 정렬은 comparison 과 normalized 가 prefix 까지 동일 길이일 때만 성립.** POSIX: comparison === normalized(둘 다 `/` collapse, case 변화 없음) → 정렬 OK. Windows 비-WSL: comparison = normalized.toLowerCase() → ASCII 는 길이 보존, **비-ASCII case-fold(예: `ß`→`ss`)는 길이 변동 → slice 어긋남**(§T3/§T5, 오라클 미커버). WSL 분기는 `comparisonCandidate` 에서 잘라 회피. **Rust 포팅 시 UTF-16 length 가 아니라 실제 매치된 prefix 의 바이트 길이로 slice 할 것(§T5).**

---

## 1.9~1.12 비-export 헬퍼 / 상수

### 1.9 `trimRuntimePathTrailingSlash(value): string` — :94-99
- :95 `value === '/'` 또는 `/^[A-Za-z]:\/$/.test(value)`(드라이브 루트, **`i` 플래그 없음** — 단 `[A-Za-z]` 문자클래스로 사실상 양대소문자 커버) → 그대로.
- :98 그 외 `value.replace(/\/+$/, '')` — 후행 `/` 연속 제거.
- **주의**: 여기 드라이브-루트 정규식은 :66/:82 의 `/^[a-z]:\/$/i` 와 **표기는 다르나 의미 동일**(둘 다 대소문자 드라이브 매치). 포팅 시 세 곳 모두 "드라이브 문자 + `:/` + 끝" 로 통일 가능하나, **원본 그대로 세 곳 유지가 안전**.

### 1.10 `isWindowsPathFlavor(value): boolean` — :101-103  ★§1.1 과 다름★
- :102 `/^[A-Za-z]:[\\/]/.test(value) || value.includes('\\') || value.startsWith('//')` — 드라이브+구분자, 또는 **문자열 어디든 backslash 포함(`.includes('\\')`)**, 또는 `//` 시작.
- **용도**: `isRuntimePathAbsolute` 기본 flavor(:31), `resolveRuntimePath` flavor(:40-41).
- **§1.1 대비 결정적 차이**: `isWindowsAbsolutePathLike` 는 backslash 가 **시작(`\\`)** 이어야 하고 comparison 계열이 씀. `isWindowsPathFlavor` 는 **중간 backslash 하나라도** 있으면 windows 이고 resolve/absolute 가 씀. 즉 `..\worktrees`(상대, 중간 backslash) 는 flavor=windows(resolve 대상)지만 abs-like=false. `/srv/team\repo`(POSIX, 중간 backslash)는 comparison 에서 abs-like=false → backslash 보존. **두 술어를 절대 병합하지 말 것(§T1).**

### 1.11 `normalizeRuntimePathDots(value, pathFlavor): string` — :105-128 (`.`/`..` 해석)
- :106 `normalized = normalizeRuntimePathSeparators(value)`.
- :107 `{ root, rest } = splitRuntimePathRoot(normalized, pathFlavor)`.
- :109-122 `rest.split('/')` 순회:
  - :110-112 빈 세그먼트 또는 `.` → skip.
  - :113-120 `..`:
    - :114-115 `segments` 비어있지 않고 마지막이 `..` 아니면 → `pop()`.
    - :116-117 아니고 `!root`(상대경로) → `..` push (**선행 `..` 보존**).
    - :118 아니면(root 있어 위로 못감) → 버림(no-op).
  - :121 그 외 push.
- :123 `suffix = segments.join('/')`.
- :124-125 `!root` → `suffix || '.'` (빈 상대 → `.`).
- :127 root 있음 → `suffix ? `${root}${suffix}` : trimRuntimePathTrailingSlash(root)` (root 는 이미 `/` 로 끝 → 직접 결합; 빈 suffix 면 root 후행 슬래시 trim, 단 `/`·`C:/` 는 trim 헬퍼가 보존).

### 1.12 `splitRuntimePathRoot(value, pathFlavor): { root, rest }` — :130-155
- :134-150 **windows flavor**:
  - :135-137 드라이브 `/^([A-Za-z]:)(?:\/|$)/` → `root = `${drive[1]}/`` (예 `C:/`), `rest = value.slice(drive[0].length)`.
  - :139-146 `//` 시작(UNC): `parts = value.slice(2).split('/')`; :141-143 `parts.length >= 2 && parts[0] && parts[1]` 면 `root = `//${parts[0]}/${parts[1]}/``(server/share), `rest = parts.slice(2).join('/')`; :145 아니면 `root='//', rest=value.slice(2)`.
  - :147-148 `/` 시작 → `root='/', rest=value.slice(1)`.
- :151-153 (fall-through/posix) `/` 시작 → `root='/', rest=value.slice(1)`.
- :154 그 외 → `root='', rest=value` (상대경로).
- **주의**: windows 분기가 드라이브/UNC/슬래시 어느 것도 아니면 **early-return 없이 :151 로 fall through**. `C:foo`(콜론 뒤 구분자 없음)는 드라이브 정규식 미스 → 결국 `root='', rest='C:foo'`. 엣지.

---

## §T. 트랩 클래스 열거 (file:line + Rust divergence + 포팅 결정)

### §T0. `node:path` 부재 — `std::path` 사용 금지
- 소스에 `node:path` 없음. 하지만 **Rust `std::path::Path` 는 컴파일 타깃 플랫폼(Unix=`/`, Windows=`\`)에 고정** → 두 flavor 를 동시에 다뤄야 하는 이 로직과 **근본적으로 발산**. **결정: `std::path`/`PathBuf` 를 이 모듈 어디에도 쓰지 말 것.** posix/win32 구분자·드라이브·UNC 를 전부 문자열/정규식으로 hand-roll. 소스가 이미 그렇게 함 → verbatim 이식.
- 재현해야 할 "Node-like" 연산(전부 hand-rolled, node:path 아님): `resolve`(=`resolveRuntimePath`+`normalizeRuntimePathDots`), `relative`(=`relativePathInsideRoot`, prefix-slice 방식이지 `..` 생성 방식 아님 — 아래 §T6), `basename`(=`getRuntimePathBasename`), `isAbsolute`(=`isRuntimePathAbsolute`). **주의: 이 모듈의 `relative` 는 candidate 가 root 밖이면 `..` 를 만들지 않고 `null` 을 반환** — Node `path.relative` 와 다름.

### §T1. posix vs win32 flavor 선택 — **두 개의 다른 술어** ★최우선★
- `isWindowsAbsolutePathLike` :1-3 (시작 앵커): drive+sep / `\\` / `//`. 사용처: comparison :14, relativePathInsideRoot 후보 :72.
- `isWindowsPathFlavor` :101-103 (중간 포함): drive+sep / **`includes('\\')`** / `//`. 사용처: isRuntimePathAbsolute 기본 :31, resolveRuntimePath :40-41.
- **Rust divergence**: 하나로 합치면 (a) POSIX `/srv/team\repo` 가 windows 로 오인 → backslash 가 구분자로 접혀 컨테인먼트 우회, 또는 (b) `..\worktrees` 가 posix 로 오인 → resolve 실패. **결정: 두 개의 별도 함수로 포팅. 절대 통합 금지.** `char == '\\'`, `starts_with("\\\\")`(Rust 리터럴 `"\\\\"` = 두 backslash), `starts_with("//")`, drive 정규식 `^[A-Za-z]:[\\/]` 를 각각 정확히.

### §T2. 구분자 정규화
- `\\`→`/` 전량 치환, 그 후 `/+`→`/` collapse(:6). UNC 이중 슬래시는 **원본 startsWith 판정 후** `//` 재부착으로 보존(:7-8). POSIX 는 backslash 미치환·`/+` collapse 만(:18, :74).
- **Rust**: `str::replace("\\", "/")` 후 정규식 `/+`→`/`(또는 수동 collapse). Rust `regex` crate 사용 여부는 Codex 질문(정규식 다수 — `regex` 의존 vs 수동 파서). 반드시 **소스와 동일 순서**(replace 먼저, collapse 다음).

### §T3. Case-folding — `toLowerCase()` (:24 distro, :26 windows 전체)
- JS `.toLowerCase()` = **로케일 독립 전체 유니코드** 소문자화(Turkish-i 예외 없음, `ß`→`ß` 유지하나 일부 유니코드는 길이 변동).
- **Rust divergence**: `to_ascii_lowercase()`(A–Z 만) vs `to_lowercase()`(전체 유니코드). 
  - Windows 파일시스템 case-insensitivity 는 실무상 ASCII 드라이브/영문 위주지만 **소스는 전체 문자열에 `.toLowerCase()` 적용**. 충실 이식 = `to_lowercase()`.
  - 그러나 `to_lowercase()` 는 §T5 의 slice 길이 정렬을 깰 수 있음(`İ`→`i̇` 등 길이 증가). **결정 필요(Codex): `to_ascii_lowercase` 로 divergence 를 감수할지, `to_lowercase` + slice 를 바이트-매칭으로 안전화할지.** 오라클은 ASCII 만 테스트하므로 어느 쪽도 통과 → **추가 pin 으로 비-ASCII 케이스 고정 권장**.

### §T4. WSL UNC alias fold — :20-26
- 정규식 `/^\/\/(?:wsl\.localhost|wsl\$)\/([^/]+)(\/[\s\S]*)?$/i`. 두 alias(`wsl.localhost`, `wsl$`)를 `//wsl/` 로, **distro 만 lowercase**, 나머지 case 보존. `[\s\S]` 로 개행 포함(테스트 :66-70 `line\nbreak`).
- **Rust**: `regex` crate 로 case-insensitive(`(?i)`) + `[\s\S]` = `(?s).` 또는 `[\s\S]`. `wsl\$` = 리터럴 `$` 이스케이프. group2 는 optional(`None` → 빈 문자열). **distro 만 `to_lowercase`, rest 는 원형.**

### §T5. String/index — `.startsWith` / `.slice` / `.length` (UTF-16 vs UTF-8)
- JS `.length`/`.slice(n)` 은 **UTF-16 코드유닛** 인덱스. `relativePathInsideRoot` 반환(:90-91)이 `slice(comparisonPrefix.length)`.
- **Rust divergence**: Rust `&str[..]` 는 **UTF-8 바이트** 인덱스이며 char 경계 아니면 panic. **결정: `comparisonPrefix` 의 (UTF-16) 길이를 재현하지 말고, "매치된 prefix 이후"를 잘라라** — POSIX/ASCII 에선 comparison·normalized 가 동일 prefix 이므로 `normalizedCandidate` 에서 `comparisonPrefix` 바이트 길이만큼 skip 하면 정확. 비-ASCII case-fold(§T3)로 길이 어긋날 수 있으니 **WSL 처럼 slice 대상(comparison vs normalized)을 소스대로 분기**하고, slice 는 `char_indices`/`strip_prefix` 로 안전화. `.startsWith`(:67,:84) → `str::starts_with`(바이트 prefix, 의미 동일).

### §T6. 컨테인먼트 predicate 충실도 (equal / sibling-prefix / `..` / 절대·상대 혼합)
- **equal**: :62-64(`isPathInsideOrEqual` true), :79-81(`relativePathInsideRoot` `''`). 정규화 **후** 비교 — 즉 `/repo/app/`(후행 슬래시) vs `/repo/app` 는 trim 후 equal.
- **sibling-prefix**: :65-67 / :82-83 의 `/` 경계 부착이 유일 방어. `/repo/application`(테스트 :14), UNC `repo2`(테스트 :37), 드라이브 `D:`(테스트 :28) 모두 이걸로 걸러짐. **이 `/` 부착을 빼먹으면 즉시 escape.**
- **드라이브/FS 루트 특례**: :66/:82 의 `root === '/' || /^[a-z]:\/$/i` — 루트는 이미 `/` 로 끝나므로 경계 이중부착 방지. 빠지면 `c:/` 아래가 전부 매치 실패.
- **`..` traversal**: 이 두 함수는 **정규화에서 `..` 를 resolve 하지 않음**(`normalizeRuntimePathForComparison` 는 dot 해석 안 함 — `normalizeRuntimePathDots` 는 resolve 전용). 즉 candidate 에 `..` 가 있으면 리터럴 `..` 세그먼트로 prefix 매칭됨. **트랩: 호출자가 candidate 를 `resolveRuntimePath` 로 먼저 정규화하지 않으면 `..` escape 가능** — 오라클 미커버. Codex 질문(§OQ).
- **절대·상대 혼합**: 오라클 미커버. 상대 root 와 절대 candidate 등은 prefix 매칭이 그냥 실패/오작동할 수 있음 — 호출 규약 확인 필요.

### §T7. Whitespace / `.trim()` — **부재**
- 모듈 전체에 `.trim()` 없음(grep 0건). **경로 내 공백/개행은 리터럴 보존**(테스트 :66-70 `line\nbreak` 이 그대로 반환). **Rust 포팅에서 `.trim()`/`trim_matches` 를 추가하지 말 것** — JS 와 발산. 이건 "하지 말아야 할 것" 트랩.

### §T8. `getRuntimePathBasename` 의 flavor-무관 backslash split — :56
- comparison 계열이 POSIX backslash 를 보존하는 것과 반대로, basename 은 항상 `[\\/]` 로 split. **의도적 비대칭** → verbatim 이식. 오라클 미커버 → pin.

---

## §O. 오라클 (테스트별, input → expected → 크럭스)

`describe('cross-platform path containment')` — 총 7 `it` 블록.

**it :11-16 "keeps POSIX sibling prefixes outside the root"**
- :12 `isPathInsideOrEqual('/repo/app','/repo/app')` → `true`. 크럭스: **equal = inside(OrEqual)**.
- :13 `('/repo/app','/repo/app/src/index.ts')` → `true`. inside 정상.
- :14 `('/repo/app','/repo/application/src/index.ts')` → `false`. ★**sibling-prefix escape 방어**★ — 경계 `/repo/app/`.
- :15 `relativePathInsideRoot('/repo/app/','/repo/app/src/index.ts')` → `'src/index.ts'`. root 후행 슬래시 trim + 상대 suffix.

**it :18-23 "keeps literal POSIX backslashes distinct from separators"**
- :19 `normalizeRuntimePathForComparison('/srv/team\\repo')` → `'/srv/team\\repo'`(리터럴 `/srv/team\repo`). 크럭스: **POSIX backslash = 파일명 문자, 미변환**.
- :20 `('/srv/team/repo')` → `'/srv/team/repo'`.
- :21 `isPathInsideOrEqual('/srv/team\\repo','/srv/team/repo/file.ts')` → `false`. ★**backslash 를 slash 로 접었으면 escape 됐을 케이스**★ — POSIX 보존이 보안.
- :22 `relativePathInsideRoot('/srv/repo','/srv/repo/a\\b.txt')` → `'a\\b.txt'`(리터럴 `a\b.txt`). backslash 보존 suffix.

**it :25-30 "handles Windows drive roots and sibling drives case-insensitively"**
- :26 `isPathInsideOrEqual('C:\\Repo','c:\\repo\\src\\index.ts')` → `true`. Windows **case-insensitive**(양쪽 `c:/repo`).
- :27 `relativePathInsideRoot('C:\\Repo','c:\\repo\\src\\index.ts')` → `'src/index.ts'`. case-insensitive 매칭, **case 보존 suffix**(입력이 소문자).
- :28 `isPathInsideOrEqual('C:\\Repo','D:\\Repo\\src\\index.ts')` → `false`. ★**sibling drive**★.
- :29 `relativePathInsideRoot('C:\\','c:\\repo\\src\\index.ts')` → `'repo/src/index.ts'`. ★**드라이브 루트 특례**(prefix `c:/`, 추가 슬래시 없음)★.

**it :32-38 "handles UNC roots, trailing slashes, mixed separators, and case"**
- :33 `isPathInsideOrEqual('\\\\Server\\Share\\Repo\\','//server/share/repo/src')` → `true`. UNC + 후행 슬래시 + 혼합 구분자 + case-fold 동시.
- :34-36 `relativePathInsideRoot(같은 root, '//server/share/repo/src')` → `'src'`.
- :37 `isPathInsideOrEqual('\\\\Server\\Share\\Repo','\\\\server\\share\\repo2')` → `false`. ★**UNC sibling-prefix `repo2`**★ vs 경계 `//server/share/repo/`.

**it :40-71 "treats WSL UNC aliases as the same case-sensitive filesystem"** (§T4 핵심)
- :41-46 `isPathInsideOrEqual('\\\\wsl$\\Ubuntu\\home\\Alice\\repo','\\\\wsl.localhost\\ubuntu\\home\\Alice\\repo\\src')` → `true`. **두 alias fold(`wsl$`≡`wsl.localhost`), distro `Ubuntu`≡`ubuntu`, 하지만 `Alice` case 보존 일치**.
- :47-52 `relativePathInsideRoot(..., '...Alice\\repo\\Src')` → `'Src'`. ★**case 보존 suffix**, WSL 분기는 `comparisonCandidate` 에서 slice★.
- :53-58 `isPathInsideOrEqual(..., '...home\\alice\\repo\\src')` → `false`. ★**distro 아래 Linux 경로는 case-sensitive** — `alice`≠`Alice`★.
- :59-64 `relativePathInsideRoot(..., '...home\\alice\\repo\\src')` → `null`. 위와 동일 outside.
- :65-70 `relativePathInsideRoot(..., '...Alice\\repo\\line\nbreak')` → `'line\nbreak'`. ★**경로 내 개행 리터럴 보존**(`[\s\S]`), trim 없음(§T7)★.

**it :73-79 "resolves POSIX relative paths without using the process cwd"**
- :74-76 `resolveRuntimePath('/repos/app/repo','../worktrees/feature')` → `'/repos/app/worktrees/feature'`. `..` 가 `repo` pop.
- :77 `('/repos/app/repo','/custom/worktrees')` → `'/custom/worktrees'`. ★**절대 target 우선**★.
- :78 `isRuntimePathAbsolute('../worktrees')` → `false`. POSIX 자동 flavor.

**it :81-87 "resolves Windows relative paths with Windows semantics"**
- :82-84 `resolveRuntimePath('C:\\Repos\\app\\repo','..\\worktrees\\feature')` → `'C:/Repos/app/worktrees/feature'`. ★**중간 backslash → windows flavor**(§T1), `..` pop, **case 보존**, 출력 `/`★.
- :85 `('C:\\Repos\\app\\repo','D:\\worktrees')` → `'D:/worktrees'`. 절대 windows target 우선.
- :86 `isRuntimePathAbsolute('/remote/worktrees','windows')` → `true`. ★**명시 windows flavor 에선 단일 `/` 시작도 절대**★.

### §O.미커버 (추가 pin 필요) — 특히 sibling-prefix·case-folding 계열
- **`getRuntimePathBasename` 전체 미테스트**(§T8) — trailing sep, `//` 빈 세그먼트, POSIX `team\repo`→`repo` 비대칭, 빈 입력→`''`. **pin 필수**.
- **`normalizeRuntimePathSeparators` / `isWindowsAbsolutePathLike` 직접 미테스트** — UNC `//` 보존, `C:`(구분자 없음) 미매치 등.
- **비-ASCII case-fold(§T3)** — `toLowerCase` vs `to_ascii_lowercase` 발산 + slice 길이(§T5). 오라클은 ASCII 만 → **비-ASCII pin 권장**.
- **`normalizeRuntimePathDots` 단독**: `.` 세그먼트 제거, 상대경로 선행 `..` 보존, root 하에서 `..` 초과분 drop, 빈 상대→`.`, `C:/`·`/` 빈 suffix 보존 — resolve 테스트로 일부만 커버.
- **`..` traversal 이 컨테인먼트에서 resolve 되지 않는 점(§T6)** — candidate 에 리터럴 `..` 가 prefix 매칭되는 위험. 호출 규약/추가 pin.
- **절대·상대 혼합, 빈 문자열, `.`/`..` 단독 입력** — 미커버.
- **sibling-prefix 의 non-ASCII / 후행 다중 슬래시 변형** — 기본 케이스만 커버.

---

## §OQ. Codex 교차검증 오픈 퀘스천

1. **posix/win32 hand-roll 표면**: `std::path` 는 플랫폼 고정이라 **사용 불가**(§T0) — 확정 동의? 재현 대상은 hand-rolled `resolveRuntimePath`/`normalizeRuntimePathDots`/`splitRuntimePathRoot`(드라이브 `C:/` 루트, UNC `//server/share/` 루트, `//`→`/` 특례) — 이 드라이브/UNC 분해 규칙(:130-155)을 정규식으로 옮길지, 수동 파서로 옮길지. **주의: 이 모듈의 `relative`(=`relativePathInsideRoot`)는 밖이면 `..` 대신 `null`** — Node `path.relative` 와 다름을 이식자에게 명시.
2. **컨테인먼트 predicate 충실도**: sibling-prefix 방어가 오직 `/` 경계 부착(:65-67/:82-83)에 달림 + 드라이브/FS 루트 특례(:66/:82). equal 처리(:62/:79), `..` 미-resolve(§T6) — candidate 를 호출 전에 `resolveRuntimePath` 로 정규화해야 안전한지(호출 규약)?
3. **case-folding 선택**(§T3): `toLowerCase`(전체 유니코드) vs Rust `to_ascii_lowercase` vs `to_lowercase`. Windows 전체 fold(:26)와 WSL distro fold(:24) 에 어느 것? `to_lowercase` 시 §T5 slice 길이 안전화 방안(strip_prefix/char_indices).
4. **whitespace/trim**(§T7): 모듈에 `.trim()` 없음 — 포팅에서도 절대 추가 금지 확인(개행 보존 테스트 :66-70 이 오라클).
5. **두 Windows 술어 분리**(§T1): `isWindowsAbsolutePathLike`(시작 앵커, comparison 용) vs `isWindowsPathFlavor`(중간 포함, resolve/absolute 용) — 병합 금지 확정. 병합 시 POSIX backslash escape(:21) 회귀.
6. **String 인덱싱**(§T5): UTF-16 length 기반 slice → Rust UTF-8 바이트/char 경계 안전 slice 로 재작성 필요. WSL vs 비-WSL slice 대상 분기(:89-91) 유지.
7. **정규식 엔진**: 다수 정규식(`^[A-Za-z]:[\\/]`, WSL alias, 드라이브 루트) — Rust `regex` crate 의존 도입 vs 수동 매칭. 성능/의존성 트레이드오프.
8. **미커버 export 의 mutation-verified pin**: `getRuntimePathBasename`(§T8), `normalizeRuntimePathSeparators`, `isWindowsAbsolutePathLike`, `normalizeRuntimePathDots` 단독 + 비-ASCII case-fold — 이식 전 오라클 보강 목록 확정.
