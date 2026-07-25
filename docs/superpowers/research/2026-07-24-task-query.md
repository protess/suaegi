# task-query 조사: 태스크/작업항목 검색 쿼리 DSL 파서

> 2026-07-24. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> 구현하지 않는다 — 이 문서가 포팅 계약의 증거 기반이다. 서브에이전트가 verbatim 포팅한다.
> 인용 경로 표기: 별도 명시 없으면 전부 `src/shared/task-query.ts` (305줄). 오라클은 `src/shared/task-query.test.ts` (216줄, `test.ts:`로 표기).
> 모듈은 **완전 self-contained** — import 0건(파일 상단 import 문 없음, `:1`부터 바로 `export type`). Electron/fs/node 의존 전무. 순수 문자열 → 구조체 → 문자열 변환뿐.
>
> **가장 중요한 발견 세 줄:**
> 1. **round-trip 계약은 raw 문자열이 아니라 파싱 구조체의 idempotency다.** 오라클 `test.ts:151-155`는 `parse(serialize(parse(raw)))`가 `parse(raw)`와 같음을 assert — `serialize(parse(raw)) === raw`가 **아니다**. `serialize`는 필드를 **정규 순서**(scope→state→draft→author→assignee→review-requested→reviewed-by→labels→freeText)로 재배열하므로 원본 순서/토큰 형태는 손실된다. 단 `test.ts:164-167`는 `serialize(parse('is:pr state:all')) === 'is:pr state:all'` 정확 문자열을 핀한다(정규형과 우연히 일치하는 특정 케이스).
> 2. **free-text는 `raw`(따옴표 포함 원형)로 보존된다 — 재직렬화 시 따옴표 스트립 금지.** 알 수 없는 qualifier와 정확 구문(`custom:value`, `"exact phrase"`)은 `raw`가 그대로 `freeTextTokens`에 push되고(`:108`,`:148`), serialize에서 마지막에 `q.freeText`로 append(`:207-209`). 주석(`:146-147`)이 명시: "reserializing stripped quotes changes search semantics."
> 3. **JS `/\s/` · `.trim()` · `.toLowerCase()`가 8군데 — 최대 divergence 리스크.** 특히 tokenizer의 `/\s/.test(char)`(`:34`)와 `quoteIfNeeded`의 `/\s/.test(value)`(`:166`)는 **JS `\s` 집합**(U+FEFF **포함**, U+0085 **제외**)을 쓴다. Rust `char::is_whitespace`는 정반대(U+0085 포함, U+FEFF 제외) — §7에서 전수 열거. `.trim()`도 같은 함정.

---

## 0. 요약 — 이 조사가 확정한 사실

1. **공개 함수 5 + 타입 2 + 상수 0.** export: `tokenizeSearchQuery`(`:53`), `parseTaskQuery`(`:57`), `serializeTaskQuery`(`:173`), `withQualifier`(`:227`), `stripRepoQualifiers`(`:287`); 타입 `ParsedTaskQuery`(`:1-11`), `TaskQueryFilterKey`(`:213-220`). 비-export 헬퍼 3: `tokenizeSearchQueryWithRaw`(`:18-51`), `quoteIfNeeded`(`:165-167`), 비-export 타입 `SearchQueryToken`(`:13-16`). **명명 상수/설정 상수는 0건** — qualifier 키·프리픽스는 전부 코드 안 리터럴(`'is:open'`, `'assignee'` 등).
2. **DSL은 하드코딩된 유한 qualifier 집합.** `is:` 스코프/상태(issue/pr/pull-request/open/closed/merged/draft), `key:value` qualifier 5종(assignee/author/review-requested/reviewed-by/label), `state:` 상태값 4종(open/closed/merged/all). 그 외 전부 free-text. 새 qualifier 추가는 `if` 체인 추가로만.
3. **키는 case-insensitive, 값은 대개 case-preserving.** `is:*` 토큰은 `token.toLowerCase()`로 통째 비교(`:74`). `key:value`의 키는 `rawKey.toLowerCase()`(`:106`)지만 **값은 원본 유지**(`:105` `.trim()`만) — 단 `state:` 값만 `value.toLowerCase()`로 정규화(`:134`).
4. **정규식은 3개, 전부 trivial — regex 크레이트 불요.** `/\s/`(tokenizer `:34`, quoteIfNeeded `:166`, stripRepo `:293`), `/\n$/` 없음(text-search와 달리 개행 스트립 없음), `/^repo:[^\s]+$/i`(stripRepo `:290`). 전부 손수 char-scan으로 대체 가능. **단 `\s` 집합의 JS 시맨틱을 정확히 재현해야 한다**(§7).
5. **serialize 필드 순서는 struct 고정 — HashMap ordering 우려 없음.** `ParsedTaskQuery`는 고정 필드 struct(`:1-11`), `labels`만 배열(push 순서 보존, `:131`·`:204-206`). Rust struct + `Vec<String>`이면 순서 정확 재현. JS 객체 키 순서 의존 0.
6. **is:issue / is:pr 상호작용 = 스코프 확대 로직.** 둘 다 보이면 `scope='all'`로 넓힘(순서 무관, `:75-83`). draft/merged/review-* qualifier는 post-pass에서 `scope='pr'` 강제(`:151-160`).
7. **serialize의 `\"` 이스케이프는 tokenizer가 역해석 못 함 → 값에 `"` 포함 시 round-trip 깨짐(latent).** `quoteIfNeeded`가 `.replaceAll('"','\\"')`(`:166`)로 이스케이프하지만 tokenizer(`:32-48`)에 백슬래시 처리 없음 → `"` 든 값은 재파싱 시 mangle. 오라클 미커버 — §5.3·§8 개방질문.

---

## 1. 공개 표면 (exported surface)

전부 `src/shared/task-query.ts`. 모듈 전체 **순수**(IO/clock/전역 상태 0). 부수효과 없음(전부 값 반환).

| 심볼 | 종류 | 시그니처 | 순수성 | 인용 |
|---|---|---|---|---|
| `ParsedTaskQuery` | type | struct 아래 표 | — | `:1-11` |
| `tokenizeSearchQuery` | fn | `(rawQuery: string) => string[]` | pure | `:53-55` |
| `parseTaskQuery` | fn | `(rawQuery: string) => ParsedTaskQuery` | pure | `:57-163` |
| `serializeTaskQuery` | fn | `(q: ParsedTaskQuery) => string` | pure | `:173-211` |
| `TaskQueryFilterKey` | type | union 7종 | — | `:213-220` |
| `withQualifier` | fn | `(rawQuery: string, key: TaskQueryFilterKey, value: string \| string[] \| null) => string` | pure | `:227-276` |
| `stripRepoQualifiers` | fn | `(rawQuery: string) => string` | pure | `:287-305` |

**`ParsedTaskQuery` 필드(`:1-11`)** — Rust struct 대응 필수:

| 필드 | 타입 | 기본값(`:58-68`) | 인용 |
|---|---|---|---|
| `scope` | `'all' \| 'issue' \| 'pr'` | `'all'` | `:2`,`:59` |
| `state` | `'open' \| 'closed' \| 'all' \| 'merged' \| null` | `null` | `:3`,`:60` |
| `draft` | `boolean` | `false` | `:4`,`:61` |
| `assignee` | `string \| null` | `null` | `:5`,`:62` |
| `author` | `string \| null` | `null` | `:6`,`:63` |
| `reviewRequested` | `string \| null` | `null` | `:7`,`:64` |
| `reviewedBy` | `string \| null` | `null` | `:8`,`:65` |
| `labels` | `string[]` | `[]` | `:9`,`:66` |
| `freeText` | `string` | `''` | `:10`,`:67` |

**`TaskQueryFilterKey`(`:213-220`)** — `withQualifier`가 받는 키 union: `'author' | 'assignee' | 'reviewRequested' | 'reviewedBy' | 'labels' | 'state' | 'draft'` (7종). **주의:** `scope`는 여기 없다 — 스코프는 qualifier로 직접 못 바꾸고 다른 qualifier의 부작용으로만 변한다.

**비-export 내부(반드시 함께 포팅):**
- `SearchQueryToken`(`:13-16`): `{ value: string; raw: string }`. tokenizer 내부 표현 — `value`는 따옴표 벗긴 값, `raw`는 원형(따옴표 포함).
- `tokenizeSearchQueryWithRaw(rawQuery)`(`:18-51`): 진짜 tokenizer. `tokenizeSearchQuery`(`:53-55`)는 이걸 호출해 `.value`만 뽑는 얇은 래퍼(`:54`). `parseTaskQuery`는 `value`+`raw` 둘 다 쓴다(`:73`).
- `quoteIfNeeded(value)`(`:165-167`): 공백 있으면 `"..."` 감싸고 내부 `"`를 `\"`로 치환(`:166`). serialize 전용.

---

## 2. DSL 문법 (정확한 열거)

파서는 각 토큰을 아래 **우선순위 순서로** 검사한다(먼저 매치되면 `continue`). 토큰 분류: (A) `is:*` 고정 토큰, (B) `key:value` qualifier, (C) 그 외 = free-text.

### 2.1 `is:*` 고정 토큰 — `normalized = token.toLowerCase()` 통째 비교(`:74`)

| 토큰(정규화 후) | 효과 | 인용 |
|---|---|---|
| `is:issue` | `sawIssueScope=true`; `scope = sawPRScope ? 'all' : 'issue'` | `:75-79` |
| `is:pr` **또는** `is:pull-request` | `sawPRScope=true`; `scope = sawIssueScope ? 'all' : 'pr'` | `:80-84` |
| `is:open` | `state='open'` | `:85-88` |
| `is:closed` | `state='closed'` | `:89-92` |
| `is:merged` | `state='merged'` | `:93-96` |
| `is:draft` | `scope='pr'; state='open'; draft=true` | `:97-102` |

`is:pull-request`는 `is:pr`의 **별칭**(`:80`, 오라클 `test.ts:54-58`). 스코프 확대 로직: issue와 pr 토큰이 **둘 다** 나타나면(순서 무관) `scope='all'`(`:77`,`:82`; 오라클 `test.ts:60-68`).

### 2.2 `key:value` qualifier — 첫 `:`로 분리(`:104-106`)

```
const [rawKey, ...rest] = token.split(':')   (:104)
const value = rest.join(':').trim()          (:105)   ← 값은 첫 콜론 이후 전부, trim만
const key = rawKey.toLowerCase()             (:106)   ← 키만 소문자화
if (!value) { freeTextTokens.push(raw); continue }   (:107-110)  ← 빈 값 = free-text로
```

| 키(소문자) | 효과 | 값 정규화 | 인용 |
|---|---|---|---|
| `assignee` | `query.assignee = value` | 없음(case 보존) | `:112-115` |
| `author` | `query.author = value` | 없음 | `:116-119` |
| `review-requested` | `scope='pr'; reviewRequested = value` | 없음 | `:120-124` |
| `reviewed-by` | `scope='pr'; reviewedBy = value` | 없음 | `:125-129` |
| `label` | `query.labels.push(value)` — **배열 누적**(중복 허용) | 없음 | `:130-133` |
| `state` | 값이 `open`/`closed`/`merged`/`all`일 때만 `state = value.toLowerCase()` | **소문자화**(`:134`) | `:134-144` |

**미분류 → free-text:** 위 어디에도 안 걸리면 `freeTextTokens.push(raw)`(`:146-148`). 여기 걸리는 것: (a) 알 수 없는 키(`custom:value`), (b) 유효하지 않은 `state:` 값(`state:foo`), (c) 콜론 없는 단어(`hello` → `split(':')`가 `['hello']`, value `''` → 실은 `:107`에서 이미 free-text로 빠짐).

### 2.3 case-sensitivity 요약(포팅 계약)

- **`is:*` 토큰**: 통째 `.toLowerCase()`(`:74`) → `IS:Open` 매치됨.
- **qualifier 키**: `.toLowerCase()`(`:106`) → `Assignee:x` 매치됨.
- **qualifier 값**: 원본 보존(assignee/author/label/review-*), `state:` 값만 소문자화. → `assignee:Alice`는 `'Alice'` 저장, `state:OPEN`은 `'open'` 저장.
- **negation 없음.** `-qualifier`나 `is:not:*` 문법 **부재** — 소스 어디에도 `-` 프리픽스나 `not` 처리 없음. `-is:open` 같은 토큰은 `split(':')` → key `'-is'` → 미분류 → free-text.

### 2.4 quoting

- **standalone 따옴표 토큰**: `"needs review"` → 값 `needs review`(따옴표 벗김, `test.ts:19-21`), `'with spaces'` 동일(`test.ts:23-25`). 단·쌍 따옴표 둘 다.
- **qualifier 값 따옴표**: `label:"needs review"` → 한 토큰, 값 `label:needs review`(`test.ts:27-32`) → 파싱 시 key `label` value `needs review`.
- **이스케이프**: tokenizer는 **백슬래시 이스케이프를 처리하지 않는다**(§3). serialize만 `\"`를 생성하지만 역방향 미지원(§5.3).

---

## 3. tokenizer — `tokenizeSearchQueryWithRaw` (`:18-51`)

**손수 char-scan 상태기계**(regex split 아님). `quote` 상태 하나로 따옴표 안/밖 추적.

```
tokens=[]; value=''; raw=''; quote=null                       (:19-22)
flush = () => { if (value || raw) { push {value,raw}; reset } }  (:24-30)
for (i=0; i<rawQuery.length; i+=1):                            (:32)
    char = rawQuery[i]                                        (:33)
    if /\s/.test(char) && quote===null:                       (:34)  ← 따옴표 밖 공백 = 토큰 경계
        flush(); continue                                     (:35-36)
    raw += char                                               (:38)  ← 공백 아니면 raw엔 항상 추가
    if (char==='"' || char==="'") && quote===null:            (:39)
        quote = char; continue                                (:40-41)  ← 여는 따옴표: value엔 안 넣음
    if char === quote:                                        (:43)
        quote = null; continue                                (:44-45)  ← 닫는 따옴표: value엔 안 넣음
    value += char                                             (:47)
flush()                                                        (:49)
return tokens                                                  (:50)
```

**정확한 규칙(포팅 핀):**
- **분할**: 따옴표 **밖**의 `/\s/` 매치 문자가 토큰 경계(`:34`). 따옴표 안 공백은 토큰에 포함.
- **여는/닫는 따옴표는 `value`에서 제외, `raw`엔 포함**(`:38` 무조건 append vs `:40`/`:44`가 `value+=` 스킵). 그래서 `"a b"` → value `a b`, raw `"a b"`.
- **닫는 따옴표는 여는 것과 같은 문자만**(`char===quote`, `:43`). `"a'b"` 안의 `'`는 value에 들어감(quote가 `"`라 `'!==quote`, 또 `'`는 `quote!==null`이라 `:39` 안 걸림).
- **미종결 따옴표**: 끝까지 `quote!==null`이어도 `flush`는 신경 안 씀(`:24-25`는 `value||raw`만 검사) — 남은 값 그대로 방출. 예: `"abc` → value `abc`, raw `"abc`.
- **빈 토큰 제거**: `flush`는 `value || raw` 둘 다 비면 push 안 함(`:25`) — 연속 공백/선행·후행 공백은 빈 토큰 안 만듦. 빈 입력 → `[]`(`test.ts:34-36`).
- **`value=''`인데 `raw!==''`인 경우도 push된다**: 예 토큰이 `""`(빈 따옴표쌍)면 value `''`, raw `""` → `value||raw`가 `raw`로 truthy → push `{value:'', raw:'""'}`. Rust 포팅 시 이 조건(`value.is_empty() && raw.is_empty()`일 때만 스킵) 정확히.

**⚠️ 순회 단위 — UTF-16 code unit.** `rawQuery.length`/`rawQuery[i]`(`:32-33`)는 **UTF-16 code unit** 순회. BMP 밖 문자(이모지 등)는 surrogate 2개로 쪼개져 각각 `char`가 되지만, `/\s/`·따옴표 비교 모두 false라 `raw`/`value`에 그대로 이어붙어 문자열은 정확히 재구성됨. Rust `chars()`(코드포인트) 순회로 바꿔도 `raw`/`value` 재구성 결과는 동일(문자 경계에서 공백/따옴표 판정만 하므로). **단 divergence 지점은 `/\s/`가 무엇을 공백으로 보느냐**(§7) — surrogate 자체가 아님.

`tokenizeSearchQuery`(`:53-55`)는 `.map(t => t.value)` 래퍼일 뿐.

---

## 4. parseTaskQuery (`:57-163`)

### 4.1 전체 흐름

1. `query` 기본값 초기화(`:58-68`, §1 표).
2. `freeTextTokens=[]`, `sawIssueScope=false`, `sawPRScope=false`(`:70-72`).
3. **입력을 `.trim()`한 뒤 tokenize**: `tokenizeSearchQueryWithRaw(rawQuery.trim())`(`:73`) — `value`와 `raw` 둘 다 순회.
4. 각 토큰: `normalized = token.toLowerCase()`(`:74`) 계산 후 §2.1 `is:*` 체인(`:75-102`) → §2.2 `key:value` 체인(`:104-148`) 순으로 검사, 매치 시 `continue`.
5. **post-pass 스코프 강제**(`:151-160`):
   ```
   if query.draft:                                  (:151)
       query.scope='pr'; query.state='open'          (:152-153)
   else if query.state==='merged' || reviewRequested!==null || reviewedBy!==null:  (:154-158)
       query.scope='pr'                              (:159)
   ```
6. `query.freeText = freeTextTokens.join(' ').trim()`(`:161`).
7. return(`:162`).

### 4.2 branch-by-branch 세부(§2 표 외 미묘점)

- **`is:*` 매치는 `normalized`(전체 소문자) 정확 일치**(`:75`,`:80`,...). `is:openx`는 매치 안 됨 → `split(':')` key `is` value `openx` → 미분류 → free-text.
- **스코프 확대는 idempotent하지 않다? — 아니다, 순서 무관 확정.** issue 먼저(`:77` `sawPRScope`는 아직 false → `issue`), 그다음 pr(`:82` `sawIssueScope` true → `all`). pr 먼저면 대칭(`:82` pr, `:77` all). 오라클 `test.ts:60-68` 양방향 핀.
- **`is:draft` 즉시 3필드 세팅**(`:98-100`) + post-pass가 또 강제(`:152-153`). 그래서 `is:draft is:issue`(`test.ts:77-82`)에서 뒤 `is:issue`가 `scope='issue'`로 덮어도 post-pass가 `pr`로 되돌림. **draft가 이긴다.**
- **빈 값 처리**: `key:` 또는 `key:   `(공백만) → `value=''`(trim 후) → `freeTextTokens.push(raw)`(`:107-109`). **`token`이 아니라 `raw`를 push** — 따옴표 원형 보존.
- **`label`은 배열 누적**(`:131`), 중복·순서 보존. 나머지 단일값 qualifier는 마지막 값이 이김(재대입).
- **review-requested/reviewed-by는 파싱 중 즉시 `scope='pr'`**(`:121`,`:126`) + post-pass 재확인(`:156-159`). `review-requested:@me is:issue`(`test.ts:103-107`)도 `pr` 유지.
- **`state:` 유효성 게이트**(`:135-141`): 값이 4종 아니면 `if` 통과 못 하고 `:148`로 떨어져 free-text. `state:foo` → free-text `state:foo`.
- **미분류 fallthrough**(`:148`)는 `raw` push — 알 수 없는 qualifier·정확 구문 원형 유지(주석 `:146-147`).

### 4.3 정규화 요약(무엇이 손실되나)

- `is:*`·키: 소문자 비교하되 **결과 struct엔 정규 형태 저장 안 함**(bool/enum). 원래 대소문자 손실(serialize가 재생성).
- qualifier 값: **trim만**(`:105`), case 보존. → `assignee: @me `(공백) → `@me`.
- `state:` 값: 소문자화 저장(`:134`,`:142`).
- freeText: 토큰들 `raw`를 공백 하나로 join + trim(`:161`). **원본 공백/토큰 순서는 free-text 안에서만 보존**(qualifier로 분리된 것 제외).

---

## 5. serializeTaskQuery (`:173-211`) + round-trip 계약

### 5.1 필드 순서(고정, `:174-210`)

`parts=[]`에 아래 순서로 push 후 `parts.join(' ')`(`:210`):

| # | 조건 | push하는 것 | 인용 |
|---|---|---|---|
| 1 | `scope==='pr'` | `is:pr` | `:175-176` |
| 1 | `scope==='issue'` | `is:issue` | `:177-179` |
| — | `scope==='all'` | (아무것도 안 함) | — |
| 2 | `state==='open'` | `is:open` | `:180-181` |
| 2 | `state==='closed'` | `is:closed` | `:182-183` |
| 2 | `state==='merged'` | `is:merged` | `:184-185` |
| 2 | `state==='all'` | `state:all` | `:186-188` |
| — | `state===null` | (아무것도 안 함) | — |
| 3 | `draft` | `is:draft` | `:189-191` |
| 4 | `author` truthy | `author:${quoteIfNeeded(author)}` | `:192-194` |
| 5 | `assignee` truthy | `assignee:${quoteIfNeeded(assignee)}` | `:195-197` |
| 6 | `reviewRequested` | `review-requested:${quoteIfNeeded(...)}` | `:198-200` |
| 7 | `reviewedBy` | `reviewed-by:${quoteIfNeeded(...)}` | `:201-203` |
| 8 | 각 `label` | `label:${quoteIfNeeded(label)}` | `:204-206` |
| 9 | `freeText` truthy | `freeText`(그대로) | `:207-209` |

**핵심 비대칭:** `state==='all'`은 `is:all`이 아니라 **`state:all`**로 직렬화(`:187`) — parse에서 `is:all`은 없고 `state:all`만 유효하기 때문(`:135-141`). 오라클 `test.ts:164-167`가 정확 문자열 핀.

### 5.2 `quoteIfNeeded` (`:165-167`)

```
return /\s/.test(value) ? `"${value.replaceAll('"', '\\"')}"` : value
```
- **공백 있을 때만** 따옴표로 감싼다(`/\s/.test`, `:166`) — JS `\s` 집합(§7).
- 감쌀 때 내부 `"`를 `\"`로 이스케이프(`.replaceAll('"','\\"')`).
- 공백 없으면 값 그대로(따옴표 없음).

### 5.3 round-trip 계약 — 정확히 무엇이 보존되나

**오라클이 assert하는 것은 `parse∘serialize∘parse == parse`(`test.ts:151-155`), raw 문자열 identity가 아니다.**
- serialize는 **정규 순서**로 재배열 → 원본 토큰 순서 손실. 예: `assignee:x is:pr` → `is:pr assignee:x`.
- 소문자화·trim으로 값 일부 정규화 → 원본 대소문자/여백 손실.
- **그러나 파싱 결과 struct는 idempotent**: 한 번 파싱한 걸 serialize해서 다시 파싱하면 같은 struct.

**보존되는 것:** qualifier의 의미(scope/state/draft/assignee/author/review-*/labels/freeText 값), free-text 원형(따옴표 포함, `raw`로).
**손실되는 것:** 토큰 순서, `is:*`/키 대소문자, qualifier 값의 앞뒤 여백, `state:all` 이외 상태의 `state:` vs `is:` 표기.

**⚠️ latent round-trip 깨짐(오라클 미커버):**
1. **값에 `"` 포함 + 공백 있음.** `quoteIfNeeded('a "b')` → `"a \"b"`(`:166`). 재파싱 시 tokenizer에 백슬래시 처리 없음(§3) → `\`는 리터럴 value, 두 번째 `"`가 quote를 닫아 값이 mangle. `a "b` ≠ round-trip.
2. **값에 `"` 포함 + 공백 없음.** `quoteIfNeeded('a"b')` → 공백 없어 감싸지 않음 → `author:a"b`. 재파싱: tokenizer가 `"`를 여는 따옴표로 봐 `b` 뒤 미종결 → value `author:ab`(따옴표 소실). `a"b` ≠ round-trip.
3. 이 둘은 **GitHub 사용자명/라벨엔 사실상 안 나오는** 문자라 오라클이 안 잡지만, Rust 포팅 시 "따옴표 든 값은 round-trip 불변식에서 제외"임을 명시해야 오라클 통과 후에도 안전. → §10 개방질문.

---

## 6. withQualifier (`:227-276`) · stripRepoQualifiers (`:287-305`)

### 6.1 `withQualifier(rawQuery, key, value)` (`:227-276`)

**전략: parse → 해당 필드 mutate → serialize**(`:232`,`:275`). 그래서 **재-tokenize**되고 free-text는 정규 위치(끝)로 이동하되 값은 보존.

```
parsed = parseTaskQuery(rawQuery)                    (:232)
switch(key):                                          (:233)
  'author':    parsed.author = typeof value==='string' ? value : null   (:234-236)
  'assignee':  parsed.assignee = typeof value==='string' ? value : null (:237-239)
  'reviewRequested': parsed.reviewRequested = string?value:null;
                     if truthy → parsed.scope='pr'    (:240-245)
  'reviewedBy':      parsed.reviewedBy = string?value:null;
                     if truthy → parsed.scope='pr'    (:246-251)
  'labels':    parsed.labels = Array.isArray(value) ? value : []         (:252-254)
  'state':     parsed.state = value∈{open,closed,merged,all} ? value : null (:255-259)
               if state==='merged' → scope='pr'       (:260-262)
               if state!=='open'  → draft=false       (:263-265)
  'draft':     parsed.draft = (value === 'true')       (:267-268)
               if draft → scope='pr'; state='open'     (:269-273)
return serializeTaskQuery(parsed)                      (:275)
```

**포팅 핀:**
- **단일값 클리어**: `null`(또는 non-string) 전달 → 필드 `null`(`:235`,`:238`,...). `typeof value==='string'` 게이트.
- **`labels`는 전체 배열 교체**(add/remove는 호출자 몫, 주석 `:224-225`). `Array.isArray` 아니면 `[]`(`:253`).
- **`state`는 4종 화이트리스트, 그 외 → null**(`:256-259`). `value==='merged'` → scope pr(`:260-262`); `value!=='open'` → draft 끔(`:263-265`).
- **`draft`는 `value==='true'` 문자열 비교**(`:268`) — boolean 아니라 문자열 `'true'`. 그 외 전부 false.
- **부작용 스코프 강제**는 `withQualifier` 안에서도 있고(위) serialize→아니 — serialize는 스코프 재계산 안 함. 단 다음 `parseTaskQuery`(테스트가 하는)가 post-pass로 재확정. 오라클 `test.ts:204-215`가 draft/merged/reviewRequested의 `scope==='pr'` 핀.
- free-text 보존: `parseTaskQuery`가 free-text를 `raw`로 담았고 serialize가 끝에 붙이므로 `"exact phrase"`·`milestone:"next release"` 원형 유지(`test.ts:197-202`).

### 6.2 `stripRepoQualifiers(rawQuery)` (`:287-305`)

**`repo:owner/name` 토큰 제거 + 공백 든 토큰 재-따옴표.** parse 안 거치고 tokenizer(**value만**, `raw` 아님) 직접 순회.

```
kept=[]
for token of tokenizeSearchQuery(rawQuery.trim()):        (:289)  ← value 리스트
    if /^repo:[^\s]+$/i.test(token): continue             (:290-292)  ← repo: 제거(case-insensitive)
    if /\s/.test(token):                                  (:293)  ← 공백 든 토큰 재따옴표
        [rawKey, ...rest] = token.split(':')              (:294)
        if rest.length > 0:                               (:295)
            kept.push(`${rawKey}:"${rest.join(':')}"`)     (:296)  ← qualifier면 값만 감쌈
        else:
            kept.push(`"${token}"`)                        (:298)  ← standalone이면 통째 감쌈
    else:
        kept.push(token)                                  (:301)
return kept.join(' ')                                      (:304)
```

**포팅 핀:**
- **repo 정규식 `/^repo:[^\s]+$/i`**(`:290`): `repo:` 프리픽스(대소문자 무관, `i` 플래그) + 공백 없는 값 1자 이상. 오라클: `repo:foo/bar` 제거(`test.ts:122-124`), `REPO:Foo/Bar` 제거(`:126-128`), `repo:a/b` 사이에서도(`:130-132`,`:141-143`,`:145-147`). **주의:** `repo:"foo bar"`(공백 든 값, unwrap 후 `repo:foo bar`)는 `[^\s]+` 불일치 → 제거 안 되고 `:293`에서 `repo:"foo bar"`로 재-따옴표 유지. 엣지(레포명 공백은 비현실적).
- **재-따옴표 이유**(주석 `:284-285`): tokenizer가 `"needs review"`를 `needs review`로 unwrap했으므로, 살아남은 공백 토큰을 다시 감싸야 한 토큰으로 재직렬화됨. qualifier(`label:needs review`)면 값만(`label:"needs review"`), standalone(`needs review`)이면 통째(`"needs review"`). 오라클 `test.ts:134-139`(standalone → `"needs review"`).
- **`repo:` 제거는 `stripRepoQualifiers`만** — `parseTaskQuery`엔 `repo:` 인식 없음(→ free-text로 들어감). 이 함수는 IPC fan-out 전 전처리용(주석 `:279-285`).

---

## 7. `.trim()` / `.toLowerCase()` / regex / whitespace 전수 열거 (최고 divergence 리스크)

| # | 위치 | JS 코드 | JS 시맨틱 | Rust 포팅 결정 |
|---|---|---|---|---|
| 1 | `:34` | `/\s/.test(char)` (tokenizer 분할) | **JS `\s`** 집합 | JS `\s` predicate 손수 구현 필수 |
| 2 | `:73` | `rawQuery.trim()` (parse 입력) | **JS trim** 집합 | `str::trim()` ≠ JS trim |
| 3 | `:74` | `token.toLowerCase()` (`is:*` 비교) | Unicode full lower(locale-independent) | `to_lowercase()` (ASCII만 나오나 비교값이라 무해) |
| 4 | `:105` | `rest.join(':').trim()` (qualifier 값) | JS trim | `str::trim()` 함정 |
| 5 | `:106` | `rawKey.toLowerCase()` (키 비교) | Unicode full lower | `to_lowercase()` |
| 6 | `:134` | `value.toLowerCase()` (`state:` 값) | Unicode full lower | `to_lowercase()` — **결과가 struct에 저장**됨(`:142`), ASCII 4종만 유효라 무해하나 정확히 |
| 7 | `:161` | `freeTextTokens.join(' ').trim()` (freeText) | JS trim | `str::trim()` 함정 |
| 8 | `:166` | `/\s/.test(value)` (quoteIfNeeded) | JS `\s` | JS `\s` predicate |
| 9 | `:166` | `value.replaceAll('"', '\\"')` | 전역 치환 | `str::replace` |
| 10 | `:290` | `/^repo:[^\s]+$/i.test(token)` | JS `\s`(부정) + `i` 플래그 | 프리픽스 검사 + JS `\s` 부정 |
| 11 | `:293` | `/\s/.test(token)` (stripRepo) | JS `\s` | JS `\s` predicate |

### 7.1 JS `\s` 집합 정확 정의(포팅 계약)

JS 정규식 `\s` = `[ \t\n\v\f\r   -     　﻿]`.
- **U+FEFF(ZWNBSP/BOM) 포함** ← Rust `char::is_whitespace`는 **미포함**.
- **U+0085(NEL) 제외** ← Rust `char::is_whitespace`는 **포함**.
- **U+180E(몽골 모음 구분자)**: 현대 JS(ES2018+)에선 `\s` 미포함(과거 포함). Rust 미포함 — 일치.

### 7.2 JS `String.prototype.trim()` 집합

JS trim = WhiteSpace ∪ LineTerminator = `\s`의 문자 집합과 **동일**(U+FEFF 포함, U+0085 제외, U+2028/2029 포함). Rust `str::trim()`는 White_Space property(U+0085 포함, U+FEFF 제외). **동일한 U+FEFF/U+0085 divergence.**

**결론(포팅 계약):** tokenizer 분할(`:34`)·quoteIfNeeded(`:166`)·stripRepo(`:290`,`:293`)의 `\s`, 그리고 trim 3곳(`:73`,`:105`,`:161`)·값 trim(`:105`)은 **모두 JS `\s` 집합을 쓰는 커스텀 predicate/trim으로 구현**해야 오라클 및 실제 입력에서 일치한다. `str::is_whitespace`/`str::trim` **직접 사용 금지**(U+FEFF·U+0085에서 어긋남). ASCII 입력에선 무해하나 규율로 pin.

### 7.3 `toLowerCase` 결정

`:74`/`:106`/`:134` 세 곳. JS `toLowerCase()`(locale-independent Unicode). Rust는 `to_lowercase()`(Unicode) 사용 권장 — 단 세 곳 모두 결과를 **ASCII 리터럴과 비교**(`is:open`, `assignee`, `open` 등)하거나(`:74`,`:106`) ASCII 4종 화이트리스트 검사(`:134`)뿐이라 non-ASCII 입력은 어차피 매치 안 됨. `to_ascii_lowercase`로 바꿔도 **매치 결과는 동일**(non-ASCII는 어느 쪽이든 리터럴과 불일치). 단 `:134`의 결과가 struct `state`에 저장되나 유효값이 ASCII 4종뿐이라 실질 무해. **권장: `to_ascii_lowercase`로 충분하고 더 안전**(Unicode `to_lowercase`의 특수 케이스 회피) — Codex 확인 대상(§10).

---

## 8. 오라클 (case-by-case) — `src/shared/task-query.test.ts` (33개 `it`)

각 줄: 입력 → 기대 → 고정 크럭스. **전부 순수**(shell-out 없음).

**describe tokenizeSearchQuery (5)**
1. **(`test.ts:11`)** `'is:open assignee:@me foo'` → `['is:open','assignee:@me','foo']`. 공백 분할 기본.
2. **(`:19`) [quote unwrap]** `'"needs review" foo'` → `['needs review','foo']`. standalone 쌍따옴표 벗김.
3. **(`:23`) [quote unwrap]** `"'with spaces' bar"` → `['with spaces','bar']`. 홑따옴표.
4. **(`:27`) [qualifier quote]** `'label:"needs review" author:alice'` → `['label:needs review','author:alice']`. 값 따옴표를 한 토큰으로.
5. **(`:34`) [empty]** `''` → `[]`. 빈 입력.

**describe parseTaskQuery (12)**
6. **(`:40`) [defaults]** `''` → scope `all`, state `null`, labels `[]`, freeText `''`. 기본값.
7. **(`:48`)** `'is:issue is:open'` → scope `issue`, state `open`.
8. **(`:54`) [alias]** `'is:pull-request is:open'` → scope `pr`, state `open`. pull-request 별칭.
9. **(`:60`) [scope widen]** `'is:issue is:pr'` → scope `all`.
10. **(`:65`) [scope widen, 역순]** `'is:pr is:issue'` → scope `all`. 순서 무관.
11. **(`:70`) [draft]** `'is:draft'` → scope `pr`, state `open`, draft `true`.
12. **(`:77`) [draft 우선]** `'is:draft is:issue'` → scope `pr`, state `open`, draft `true`. post-pass가 issue 덮음.
13. **(`:84`)** `'is:pr is:open'` → scope `pr`, state `open`, draft `false`. is:open은 draft 안 켬.
14. **(`:91`) [멀티 qualifier]** `'assignee:@me author:alice review-requested:@me label:bug free text'` → assignee `@me`, author `alice`, reviewRequested `@me`, scope `pr`(review-requested 강제), labels `['bug']`, freeText `'free text'`. **free-text 2토큰 join.**
15. **(`:103`) [review scope 유지]** `'review-requested:@me is:issue'` → scope `pr`, reviewRequested `@me`. issue 덮어도 pr 유지.
16. **(`:109`) [unknown → free]** `'custom:value hello'` → freeText `'custom:value hello'`. 미지 qualifier·bare word.
17. **(`:114`) [state:all]** `'is:pr state:all'` → scope `pr`, state `all`.

**describe stripRepoQualifiers (6)**
18. **(`:122`)** `'is:open repo:foo/bar assignee:@me'` → `'is:open assignee:@me'`. repo 제거.
19. **(`:126`) [case-insensitive]** `'REPO:Foo/Bar is:open'` → `'is:open'`. 대문자 REPO 제거.
20. **(`:130`)** `'label:bug repo:a/b'` → `'label:bug'`. 다른 qualifier 유지.
21. **(`:134`) [re-quote standalone]** `'"needs review" repo:x/y'` → `'"needs review"'`. unwrap된 공백 토큰 재-따옴표.
22. **(`:141`) [all repo]** `'repo:foo/bar repo:baz/qux'` → `''`. 전부 repo면 빈 문자열.
23. **(`:145`) [bare word 보존]** `'hello repo:a/b world'` → `'hello world'`. 공백 없는 단어 그대로.

**describe serializeTaskQuery (3)**
24. **(`:151`) [ROUND-TRIP]** `raw='is:pr is:open author:alice label:bug review-requested:bob hello world'`; `parse(serialize(parse(raw)))` deep-equals `parse(raw)`. **구조체 idempotency**(raw identity 아님).
25. **(`:157`) [label quote]** `parse('label:"needs review"')` → labels `['needs review']`, freeText `''`; `serialize(...)` 는 `'label:"needs review"'` 포함. 공백 라벨 재-따옴표.
26. **(`:164`) [정확 문자열]** `serialize(parse('is:pr state:all')) === 'is:pr state:all'`. state:all 정규 표기 + 순서.

**describe withQualifier (7)**
27. **(`:171`) [set/clear author]** `withQualifier('hello','author','alice')` → author `alice`, freeText `hello`; 다시 `null` → author `null`, freeText `hello`. free-text 불변.
28. **(`:180`) [labels 교체]** `withQualifier('label:bug label:enh','labels',['triage'])` → labels `['triage']`. 전체 배열 교체.
29. **(`:185`) [labels clear]** `withQualifier('label:bug is:pr','labels',[])` → labels `[]`, scope `pr`. 빈 배열로 클리어, scope 유지.
30. **(`:191`) [state all]** `withQualifier('is:pr is:open','state','all')` → state `all`, 결과에 `state:all` 포함.
31. **(`:197`) [quoted free-text 보존]** `withQualifier('"exact phrase" milestone:"next release"','author','alice')` → 결과에 `"exact phrase"` 및 `milestone:"next release"` 포함, author `alice`. **정확 구문·미지 qualifier 원형 유지.**
32. **(`:204`) [PR-only scope]** `withQualifier('','draft','true')`/`('','state','merged')`/`('','reviewRequested','@me')` 각각 scope `pr`.
33. **(`:210`) [draft→open PR]** `withQualifier('is:pr is:closed','draft','true')` → scope `pr`, state `open`, draft `true`. draft가 closed 덮음.

**분류 태그:**
- **round-trip 케이스**: 24(구조체 idempotency), 26(정확 문자열), 31(free-text 보존 round-trip).
- **quoting/escaping 케이스**: 2·3·4·5(unwrap), 21·25·31(re-quote).
- **negation 케이스**: **없음** — DSL에 negation 문법 자체가 없어 오라클도 없음(§2.3).
- **Unicode/whitespace/case-folding 케이스**: **직접 커버 0건.** 전부 ASCII 입력. §7의 U+FEFF/U+0085 divergence, `toLowerCase` 특수 케이스는 오라클로 노출 안 됨 → Codex 교차검증에서 별도 pin 필요(§10).

---

## 9. Rust 생태계 노트 (사실만 — 결정 X)

- **regex 크레이트 불요(hand-roll 권장).** 모듈의 정규식은 3종뿐: `/\s/`(공백 1문자 판정), `/^repo:[^\s]+$/i`(프리픽스+비공백 값), `.replaceAll('"',...)`(리터럴 치환). 전부 char-scan/문자열 연산으로 대체 가능. **오히려 `regex` 크레이트를 쓰면 `\s` 집합이 Unicode-mode(U+FEFF 미포함, U+0085 포함)로 JS와 어긋나 위험**(§7.1). 워크스페이스에 text-search용 `regex`가 도입됐더라도(조사 `text-search.md:470` 참고, 실제 도입 여부 미확인) 이 모듈은 **쓰지 않는 게 안전** — JS `\s` predicate를 손수 정의.
- **HashMap ordering 우려 없음.** serialize 필드 순서는 `ParsedTaskQuery` struct의 고정 필드 순서(`:174-209`)이고, 유일한 컬렉션 `labels`는 push 순서 보존 배열(`:131`). Rust `struct` + `Vec<String>`이면 순서 정확 재현. JS 객체/Map insertion-order 의존 지점 0.
- **enum 대응.** `scope`(3-variant)/`state`(4-variant + null → `Option<enum>`)는 Rust enum이 자연스러움. serialize/parse의 리터럴 비교(`'is:open'` 등)를 enum↔str 매핑으로.
- **`split(':')` 시맨틱.** `[rawKey, ...rest] = token.split(':'); rest.join(':')`(`:104-105`)는 **첫 콜론 기준 분리**(값은 이후 콜론 유지). Rust `str::splitn(2, ':')`가 정확 등가. 콜론 없으면 `rest` 빈 → value `''`.
- **`.replaceAll` vs `.replace`.** `:166`의 `replaceAll('"','\\"')`는 전역 치환 → Rust `str::replace`(전역이 기본). 단 §5.3의 round-trip 비대칭 유의.
- **문자열 순회 단위.** tokenizer(`:32-33`)는 UTF-16 code unit이지만 재구성 결과는 코드포인트 순회(`chars()`)와 동일(§3). Rust `&str`은 UTF-8이라 인덱싱 대신 `chars()` iterator 권장 — 이 모듈엔 byte-offset 슬라이싱이 없어(text-search와 달리) char-boundary panic 리스크 없음.

---

## 10. Codex 교차검증용 개방 질문

1. **tokenizer quoting/whitespace 충실도.** (a) tokenizer 분할(`:34`)·`quoteIfNeeded`(`:166`)·stripRepo(`:290`,`:293`)의 `/\s/`를 **JS `\s` 집합**(U+FEFF 포함, U+0085 제외; §7.1)으로 구현해야 하는지 — Rust `char::is_whitespace`(반대)를 쓰면 U+FEFF/U+0085 경계 입력에서 오라클 아닌 실입력이 어긋난다. 오라클은 ASCII만이라 미노출. (b) 미종결 따옴표(`"abc`)가 flush에서 남는 값 그대로 방출되는지(`:24-25`,`:49`), Rust 상태기계가 동일하게. (c) `value=''&&raw!=''`(빈 따옴표쌍 `""`)일 때 push되는지(`:25`).
2. **case-folding: `to_lowercase` vs `to_ascii_lowercase`.** `:74`/`:106`/`:134` 세 `.toLowerCase()`가 전부 ASCII 리터럴 비교(§7.3)라 `to_ascii_lowercase`로 충분하고 Unicode 특수 케이스(터키어 İ→i̇ 등)를 회피하는 게 더 안전한지, 아니면 JS 충실히 `to_lowercase`를 쓸지. `:134`는 결과가 `state` struct에 저장되나 유효값 ASCII 4종뿐 — 실질 차이 없음 확인.
3. **round-trip identity 보증 범위.** 오라클(`test.ts:151-155`)은 `parse∘serialize∘parse == parse`(구조체 idempotency)만 보증, `serialize∘parse == identity`가 **아님**을 계약으로 명시할 것. 특히 §5.3의 latent 깨짐 2종(값에 `"` 포함 시 `quoteIfNeeded`의 `\"`를 tokenizer가 역해석 못 함 `:166`↔`:32-48`)이 오라클 미커버 — "따옴표 든 qualifier 값은 round-trip 불변식 제외"로 문서화할지, 아니면 Rust에서 tokenizer에 백슬래시 이스케이프를 추가해 고칠지(= Orca와 divergence). Orca 충실 원칙상 **버그까지 복제** 권장이나 확인 필요.
4. **qualifier 우선순위/dedup.** (a) `is:*` 체인(`:75-102`)이 `key:value` 체인(`:104-148`)보다 먼저 검사되는 순서, draft post-pass(`:151-153`)가 later token을 이기는 규칙(케이스 12·15)을 Rust에서 동일 순서로. (b) 단일값 qualifier는 마지막 값이 이기고 `label`만 배열 누적(중복 허용, `:131`)임을 pin. (c) `state:` 화이트리스트 게이트(`:135-141`) 밖 값은 free-text로 떨어짐(`:148`).
5. **regex-vs-hand-roll 결정.** §9대로 `regex` 크레이트를 **쓰지 않고** JS `\s` predicate·프리픽스 검사·`splitn(2,':')`로 hand-roll하는 게 맞는지. `regex` 크레이트의 `\s`는 Unicode-mode라 §7.1 divergence를 재도입할 위험 — 도입 시 `(?-u)` ASCII-mode로도 U+FEFF/U+0085를 정확히 재현 못 하므로 custom predicate가 유일한 정확 경로임을 확인.
6. **`stripRepoQualifiers`의 repo 정규식 엣지.** `/^repo:[^\s]+$/i`(`:290`)가 공백 든 repo 값(`repo:"foo bar"` unwrap 후 `repo:foo bar`)을 제거 안 하고 재-따옴표 유지하는 엣지(§6.2)가 의도된 동작인지, Rust 포팅이 이를 복제할지. 실제 레포명 공백은 비현실적이나 오라클 미커버.
