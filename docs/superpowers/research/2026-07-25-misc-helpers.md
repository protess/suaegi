# misc-helpers 조사: 6개 순수 헬퍼 배치 → `suaegi-misc` 크레이트

> 2026-07-25. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> 구현하지 않는다 — 이 문서가 포팅 계약(contract)이다. 서브에이전트는 여기서 **verbatim** 포팅한다.
> Orca 원본 경로 base: `…/scratchpad/orca-src/src/shared/`.
>
> **가장 중요한 발견 한 줄:** 6개 중 **하드코어 함정 3개는 오라클이 침묵한다.** ① `image-data-uri`의
> `/\s/g`와 `stable-pane-id`의 `.trim()`은 **U+FEFF/U+0085 양방향 발산**을 숨기는데 테스트는 ASCII 공백만
> 친다(§4·§6). ② `stable-pane-id`의 `/^\d+$/`는 **Rust `\d`가 유니코드 숫자를 먹는다**(JS는 ASCII만) —
> 테스트 없음(§6). ③ `stable-pane-id`의 UUID 정규식은 **소문자 전용·대문자 거부**인데 대문자 케이스가
> 오라클에 없다(§6). 이 셋은 통과하는 테스트만으로는 못 잡는다 → 각 §의 "추가 핀" 필수.

---

## 0. 요약 — 이 배치가 확정한 결정

1. **6개 전부 zero-import·순수·클럭 비의존.** 각 `.ts`는 `vitest` 외 import이 없다(테스트 파일만
   `import { … } from 'vitest'` + 대상 모듈). 프로덕션 코드 import = 0건. `rate-limit-reset-format`조차
   `Date.now()`/`new Date()`를 **호출하지 않는다** — `now`를 **파라미터로 주입**받는다(§2). 즉 클럭 주입은
   이미 소스가 해놨다. 배치에서 제외해야 할 "몰래 impure/coupled" 모듈은 **없다**.
2. **`suaegi-misc` 크레이트 하나에 6개 모듈 모듈로 담는다.** 상호 의존도 없다(서로 import 안 함).
3. **최대 발산 위험 = 공백/케이스/숫자 정규식 3종(§4·§6).** MEMORY의 `U+FEFF/U+0085` 함정,
   `to_lowercase vs to_ascii_lowercase` 함정, 그리고 Rust regex의 유니코드-기본 `\d`가 여기서 동시에 터진다.
   포트는 **JS `\s` 집합을 손으로 정의**하고, **UUID는 대소문자 구분 소문자 매칭**, **숫자는 `[0-9]`**(유니코드 `\d` 금지)로 못박아야 한다.
4. **Math.round 발산은 실질 무해 — 단 순서 보존이 계약.** `usage-percentage-display`의 `Math.round`는
   음수 .5에서 Rust `f64::round`(away-from-zero)와 다르지만 **양쪽 다 클램프가 뒤따라 0으로 수렴**한다.
   진짜 계약은 #7574 주석의 **"complement 전에 round"** 순서다(§1).
5. **오라클 침묵 경로:** `osc-title-scan-tail`의 `trimOscTitleScanTail`(>4096) 경로는 **테스트가 0건** —
   여기가 UTF-16 length vs Rust byte-len 발산의 서식지다(§5). 서브에이전트는 이 경로에서 **패닉 금지**(char
   boundary 보정) + **단위 선택(byte)** 을 Codex에 올려야 한다.
6. **"base64 인코딩"·"해싱"은 없다.** 프롬프트가 의심한 두 지점이 실제로는 **문자열 concat + 검증뿐**:
   `image-data-uri`는 base64를 **디코드/재인코딩하지 않고** 공백만 벗겨 그대로 이어 붙인다(§4).
   `stable-pane-id`는 **해시가 전혀 없다** — caller가 준 UUID를 검증·조합할 뿐(§6).

**트랩 클래스 히트맵 (프로덕션 코드만):**

| 모듈 | Math.* | Date/clock | trim/`\s`/case | `\d`/number-fmt | `.length`/slice(UTF-16) | base64/hash |
|---|---|---|---|---|---|---|
| usage-percentage-display | `Math.round`×2, `Math.min/max`, `Number.isFinite` | 없음 | 없음 | 없음 | 없음 | 없음 |
| rate-limit-reset-format | `Math.floor`×3, `%`×3, `Math.min`, `Number.isFinite`×2 | **주입된 `now` 파라미터**(호출 없음) | 없음 | 정수 template literal | 없음 | 없음 |
| markdown-toc-panel-width | `Math.min/max`, `Number.isFinite`×2 | 없음 | 없음 | `typeof===number` 가드 | 없음 | 없음 |
| image-data-uri | 없음 | 없음 | **`/\s/g` replace**, `startsWith`(케이스 민감) | 없음 | `startsWith` | **base64 미인코딩(concat)** |
| osc-title-scan-tail | `Math.min/max` | 없음 | 없음 | 없음 | **`.length`/`.slice(-N)`/indexOf/lastIndexOf** | 없음 |
| stable-pane-id | 없음 | 없음 | **`.trim()`**, UUID_RE **소문자 전용** | **`/^\d+$/`(Rust `\d`≠)** | `.length>256`, slice | **해시 없음** |

---

## 1. `usage-percentage-display.ts` (36L) — 클램프·라운딩·complement 순서

### 공개 표면
- `type UsagePercentageDisplay = 'used' | 'remaining'` (`usage-percentage-display.ts:1`). Rust: 2-variant enum.
- `const DEFAULT_USAGE_PERCENTAGE_DISPLAY = 'used'` (`:4`).
- `normalizeUsagePercentageDisplay(value: unknown): UsagePercentageDisplay` (`:6-8`) — 순수.
- `clampUsedPercent(usedPercent: number): number` (`:13-18`) — 순수.
- `getDisplayedUsagePercentage(usedPercent: number, display): number` (`:20-36`) — 순수.
- 클럭 비의존, import 0.

### 정확 시맨틱
- `normalizeUsagePercentageDisplay` (`:7`): `value === 'used' || value === 'remaining' ? value : 'used'`. **문자열 정확 일치, 케이스 폴드 없음** — `'Used'`/`'left'`/`undefined` 전부 default `'used'`.
- `clampUsedPercent` (`:14-17`): `!Number.isFinite` → `0`; else `Math.max(0, Math.min(100, Math.round(usedPercent)))`. **순서 주의: round → min(100) → max(0)** (round가 클램프보다 먼저).
- `getDisplayedUsagePercentage` (`:24-35`): `!Number.isFinite` → `0`(주석 `:25`: 무효 데이터를 "100% 잔량"으로 표시 금지); `boundedUsedPercent = Math.min(100, Math.max(0, usedPercent))` (`:28`); `roundedUsedPercent = Math.round(boundedUsedPercent)` (`:34`); `display === 'used' ? rounded : 100 - rounded` (`:35`). **클램프(0..100) → round → complement** 순서가 계약.

### 트랩 클래스 발생 지점
- **`Math.round` (`:17`, `:34`)** — JS `Math.round`는 half **toward +∞**(`Math.round(-0.5)===-0`), Rust `f64::round`는 half **away-from-zero**(`(-0.5).round()==-1.0`). **발산은 음수 .5에서만.** 그러나 두 함수 모두:
  - `:17` clampUsedPercent: round가 raw에 먼저 적용되지만 직후 `max(0, min(100, …))`가 음수를 0으로 흡수 → 결과 동일(예: `-0.5`, `-0.6` 모두 0).
  - `:34` getDisplayedUsagePercentage: round **이전**에 `[0,100]`로 클램프(`:28`) → round 입력이 항상 비음수 → 발산 없음.
  → **실질 무해**하나 포트는 (a) round↔클램프 **순서를 절대 뒤집지 말 것**, (b) round는 half-up/away 어느 쪽이든 비음수 입력이라 무관하지만 `f64::round` 그대로 써도 됨.
- **`Math.round(100 - x)` 발산이 진짜 계약 (#7574)** (`:29-35` 주석): complement를 **round 이후**에 취해야 함. `getDisplayedUsagePercentage(20.5,'remaining')`은 `100 - Math.round(20.5)=79`이지 `Math.round(100-20.5)=80`이 아니다. 포트가 `100 - usedPercent`를 먼저 하고 round하면 **1% drift**. → **round-then-complement 강제.**
- **`Number.isFinite` (`:14`, `:24`)** — Rust `f64::is_finite()`. NaN/±∞ → false, 일치. 입력 시그니처는 `f64`.
- **케이스/공백 함정 없음.** `normalizeUsagePercentageDisplay`는 `==` 정확 일치만.

### 오라클 (usage-percentage-display.test.ts, 케이스별)
- `:10` `normalize(undefined)` → `'used'` · 크럭스: unknown→default.
- `:11` `normalize('left')` → `'used'` · 크럭스: 비일치 문자열→default(케이스 폴드 아님).
- `:15` `getDisplayed(6,'used')` → `6` · 기본.
- `:16` `getDisplayed(6,'remaining')` → `94` · complement.
- `:20` `getDisplayed(20.5,'used')` → `21` · **크럭스: 양수 .5 half-up round.** (half-away/half-up 구분을 고정하는 유일한 절대값 핀.)
- `:23` `getDisplayed(20.5,'remaining')` → `79` · **크럭스: round된 21의 complement, 독립 round(80) 금지 (#7574).** ← 최상위 트랩 핀.
- `:24` `getDisplayed(120,'remaining')` → `0` · 클램프 120→100, 100-100.
- `:25` `getDisplayed(-20,'used')` → `0` · 클램프 -20→0.
- `:26` `getDisplayed(NaN,'remaining')` → `0` · **크럭스: 비유한 가드가 100 아닌 0.**
- `:31-33` `clampUsedPercent(NaN/+∞/-∞)` → `0` · 비유한→0(주석 `:30`: `NaN%` CSS 폭 방지).
- `:39-45` 프로퍼티: `raw ∈ {20.5,6.5,79.5,0.5,99.5}` × `display ∈ {used,remaining}`에서 `getDisplayed(clamp(raw),d) === getDisplayed(raw,d)` · **크럭스: pre-clamp와 raw가 동일 결과(no-op).** 절대값이 아닌 **동등성**만 검증(양수 .5 전용).

### 오라클이 안 치는 것 (추가 핀 권장)
- `clampUsedPercent(-0.5)` 같은 **음수 .5**(결과 0이지만 round↔클램프 순서 고정용) — 프로퍼티 배열은 양수만.
- `getDisplayed` 절대값은 `:20`(20.5→21) 하나로만 half-up 고정. 충분하나 `getDisplayed(0.5,'used')→1` 핀 추가 시 견고.

---

## 2. `rate-limit-reset-format.ts` (61L) — **클럭 이미 주입됨**, floor·modulo·정수 포맷

### 공개 표면
- `formatResetDuration(ms: number): string` (`rate-limit-reset-format.ts:10-26`) — 순수.
- `formatResetCountdown(ms: number): string` (`:29-32`) — 순수.
- `getResetCountdownNextTickDelay(now: number, resetTimes: readonly number[]): number | null` (`:45-61`) — 순수, **`now`는 주입 파라미터**.
- 모듈 private const: `MINUTE_MS=60_000`(`:34`), `HOUR_MS=60*MINUTE_MS`(`:35`), `DAY_MS=24*HOUR_MS`(`:36`).
- **`Date.now()`/`new Date()` 호출 0건** — grep 대상 전무. 주석 `:2-3`이 "Pure (no platform imports)" 명시. **클럭 발산 위험 없음** — 소스가 이미 결정론적.

### 정확 시맨틱 — `formatResetDuration` (`:11-25`)
- `ms <= 0` → `'now'` (`:11-13`). 0과 음수 모두.
- `totalMins = Math.floor(ms / 60_000)` (`:14`).
- `totalMins < 60` → `` `${totalMins}m` `` (`:15-17`).
- `hours = Math.floor(totalMins / 60)` (`:18`); `mins = totalMins % 60` (`:19`).
- `hours >= 24` (`:20`): `days = Math.floor(hours/24)`, `remHours = hours % 24`; `remHours > 0 ? `${days}d ${remHours}h` : `${days}d`` (`:21-23`). **⚠️ days 분기는 분(minute)을 완전히 버린다** — 6d 7h 30m 입력도 `"6d 7h"`.
- else `mins > 0 ? `${hours}h ${mins}m` : `${hours}h`` (`:25`).

### 정확 시맨틱 — `getResetCountdownNextTickDelay` (`:49-60`)
- `nextDelay = null` (`:49`).
- `for resetAt of resetTimes` (`:50`): `!Number.isFinite(resetAt) || resetAt <= now` → `continue` (`:51-53`). NaN·±∞·과거·동일시각 스킵.
- `remainingMs = resetAt - now` (`:54`) — 항상 > 0.
- `tickUnitMs = remainingMs >= DAY_MS ? HOUR_MS : MINUTE_MS` (`:55`). **경계 `>=`: 정확히 1일이면 시간 단위.**
- `delayMs = (remainingMs % tickUnitMs) + 1` (`:57`). **`+1ms`**로 경계 직후 발화(주석 `:56`).
- `nextDelay = nextDelay === null ? delayMs : Math.min(nextDelay, delayMs)` (`:58`).
- return `nextDelay` (`:60`).

### 트랩 클래스 발생 지점
- **`Math.floor` (`:14`, `:18`, `:21`)** — 도달 시 피연산자 항상 **비음수**(`ms>0` 가드, totalMins/hours 비음수). Rust: `ms`를 `i64`로 받으면 `ms / 60_000` **정수 나눗셈이 비음수에서 floor와 동일**. `f64`로 받으면 `(ms/60_000.0).floor()`. **권장: `i64`(epoch-ms) 시그니처** — 정수 나눗셈 truncation이 여기서 곧 floor.
- **`%` modulo (`:19` `totalMins%60`, `:22` `hours%24`, `:57` `remainingMs%tickUnitMs`)** — 전부 **비음수 피연산자**. JS `%`와 Rust `%`(truncated remainder)는 비음수에서 일치. 음수 위험 없음(가드가 앞섬).
- **정수 → 문자열 포맷 (`:16-25` template literal)** — `totalMins` 등은 `Math.floor` 결과라 정수값. JS `` `${21}` ``→`"21"`. **Rust에서 `f64`를 `format!("{}", 21.0)`하면 `"21"`이 되긴 하나 위험** — `i64` 경로면 무조건 안전. **소수점 `.0` 유출 금지: 정수 타입으로 포맷.** `toFixed`/로케일(`Intl`) 없음 — 순수 정수 연결.
- **`Number.isFinite` (`:51`)** — Rust `f64::is_finite()`. `resetTimes`가 `f64` 슬라이스면 그대로; `i64`면 NaN/∞ 개념이 없어 이 가드가 **모트(moot)**가 됨 → **타입 결정 필요**(아래 Codex 질문). `now`/`resetAt` epoch-ms.
- **`Math.min` (`:58`)** — 도달 값은 항상 유한 양수라 NaN 발산 무관. Rust `i64::min` 또는 `f64::min`(단 f64면 NaN-무시가 JS `Math.min`의 NaN-전파와 다르나, NaN 도달 불가).

### 오라클 (rate-limit-reset-format.test.ts, 케이스별)
- `:15` `formatResetDuration(0)` → `'now'` · 비양수 가드.
- `:16` `formatResetDuration(-1)` → `'now'` · 음수.
- `:20` `47*MIN` → `'47m'` · <60분 경로.
- `:21` `3*HOUR+54*MIN` → `'3h 54m'` · 시+분.
- `:22` `2*HOUR` → `'2h'` · **분 0 잔여 드롭.**
- `:23` `6*DAY+7*HOUR` → `'6d 7h'` · days 분기(분 없음).
- `:24` `7*DAY` → `'7d'` · remHours 0 드롭.
- `:30` `formatResetCountdown(0)` → `'Resets now'` · `'now'`→특수 카피.
- `:31` `3h54m` → `'Resets in 3h 54m'` · prefix.
- `:32` `6d7h` → `'Resets in 6d 7h'`.
- (nextTick, `now = 1_000_000_000`) `:40` `[]` → `null` · 빈 배열.
- `:42` `[now-MIN, now]` → `null` · 과거·동일 스킵.
- `:43` `[NaN, +∞]` → `null` · **비유한 스킵.**
- `:48` `[now + 90*MIN + 30_000]` → `30_000+1` · **크럭스: <1일→분 단위, 90m30s 나머지 30s +1.**
- `:53` `[now + 2*DAY + 3*HOUR + 15*MIN]` → `15*MIN+1` · **크럭스: ≥1일→시간 단위, 나머지 15m +1.**
- `:61` `[later, soon]` → `10_000+1` · 최소값 선택.

### 오라클이 안 치는 것 (추가 핀 권장)
- **days 분기의 분 드롭 미검증:** 모든 day-테스트가 분 0(`6d7h`,`7d`). `formatResetDuration(6*DAY+7*HOUR+30*MIN)` → **`'6d 7h'`**(분 버림) 핀 필수 — 포트가 days 분기에 분을 포함해도 기존 테스트 전부 통과. **가장 얕은 발산 함정.**
- **24h 경계 미검증:** `formatResetDuration(24*HOUR)` → `'1d'`(hours=24, remHours=0). `hours>=24` 경계 핀.
- **나머지 0 tick 미검증:** `getResetCountdownNextTickDelay(now, [now+90*MIN])` → `0+1 = 1`(나머지 0 → delay 1). 0-나머지 특수처리 방지 핀.
- **정확히 DAY_MS 경계:** `[now + DAY_MS]` → `remainingMs===DAY_MS`이면 시간 단위(`>=`) 핀.

---

## 3. `markdown-toc-panel-width.ts` (32L) — min/max 클램프, **라운딩 없음**, `unknown` 가드

### 공개 표면
- const `MARKDOWN_TOC_PANEL_MIN_WIDTH=200` (`:1`), `_DEFAULT_WIDTH=240` (`:2`), `_MIN_EDITOR_WIDTH=320` (`:3`), `_MAX_WIDTH=600` (`:4`).
- `computeMaxMarkdownTocPanelWidth(containerWidth: number): number` (`:6-15`) — 순수.
- `clampMarkdownTocPanelWidth(width: unknown, containerWidth?: number, fallback = 240): number` (`:17-32`) — 순수.
- 클럭·import 0.

### 정확 시맨틱
- `computeMax` (`:7-14`): `!Number.isFinite(containerWidth) || containerWidth <= 0` → `600`(MAX) (`:7-9`); else `Math.min(600, Math.max(200, containerWidth - 320))` (`:11-14`).
- `clampMarkdownTocPanelWidth` (`:22-31`):
  - `typeof width !== 'number' || !Number.isFinite(width)` → `fallback`(기본 240) (`:22-24`). **`width: unknown`** — undefined/string/NaN/∞ 전부 fallback.
  - `maxWidth = containerWidth !== undefined ? computeMax(containerWidth) : 600` (`:26-29`). **두 번째 인자는 언제나 "컨테이너 폭"** — precomputed max가 아님(테스트 `:23-26`이 이걸 못박음).
  - return `Math.min(maxWidth, Math.max(200, width))` (`:31`).

### 트랩 클래스 발생 지점
- **`Number.isFinite` (`:7`, `:22`)** — `f64::is_finite()`. NaN/∞→fallback.
- **라운딩 함정 없음 — 그러나 그게 함정:** `Math.round`가 **어디에도 없다.** `containerWidth-320`, `width`가 소수면 결과도 소수(예: `computeMax(700.5)=380.5`, `clampMarkdownTocPanelWidth(350.5,700)=350.5`). **포트는 절대 반올림/정수화 금지 — `f64` 그대로 min/max.** 정수화하면 소수 입력에서 발산.
- **`typeof width !== 'number'` (`:22`) = `unknown` 모델링** — 포트의 공개 시그니처 결정 필요. `width`가 "숫자 아님(undefined/문자열/객체)"과 "NaN/∞"를 **둘 다 fallback**으로 접는다. Rust 권장: `width: Option<f64>`에서 `None`(비-숫자) + `Some(x) where !x.is_finite()` → fallback. 호출부(설정 역직렬화)에서 비-숫자를 `None`으로 매핑.
- **`containerWidth !== undefined` (`:27`)** — 선택 인자, `undefined` 센티넬이 "값 전달됨"과 구별. Rust `Option<f64>`: `None` → 600. **주의: `Some(0.0)`/`Some(음수)`/`Some(NaN)`은 전달된 값이므로 computeMax로 들어가 `:7` 가드에서 600.** `None`과 `Some(<=0)`가 둘 다 600으로 수렴하나 경로가 다름.
- trim/case/toFixed/Date/base64 없음.

### 오라클 (markdown-toc-panel-width.test.ts, 케이스별)
- `:12` `clamp(undefined)` → `240`(DEFAULT) · 비-숫자→fallback.
- `:13` `clamp(100)` → `200`(MIN) · `max(200,100)`.
- `:14` `clamp(900)` → `600`(MAX) · 컨테이너 없음→max 600, `min(600,900)`.
- `:18` `computeMax(700)` → `380` · `700-320=380 ∈ [200,600]`.
- `:19` `clamp(500,700)` → `380` · maxWidth=380, `min(380, max(200,500))`.
- `:20` `clamp(350,700)` → `350` · `min(380, max(200,350))`.
- `:25` `clamp(350, computeMax(700)=380)` → `200` · **크럭스: 2번째 인자를 컨테이너로 재취급 → `computeMax(380)=min(600,max(200,60))=200`.** precomputed max로 오해 방지.
- `:26` `clamp(350,700)` → `350` · 컨테이너 시맨틱 재확인.

### 오라클이 안 치는 것 (추가 핀 권장)
- **소수 입력 무-라운딩:** `clamp(350.5,700)` → `350.5`, `computeMax(700.5)` → `380.5` 핀 — 정수 테스트만으론 반올림 포트도 통과.
- **`computeMax` 비유한/비양수:** `computeMax(0)`/`computeMax(-5)`/`computeMax(NaN)` → `600` 핀.
- **`clamp` NaN width:** `clamp(NaN)` → `240` 핀(현재 undefined만).

---

## 4. `image-data-uri.ts` (20L) — **base64 미인코딩(concat)**, `/\s/g` 발산

### 공개 표면
- `buildImageDataUri(mimeType: string | undefined, base64Content: string): string | null` (`image-data-uri.ts:6-20`) — 순수, import 0.

### 정확 시맨틱 (`:12-19`)
- `!mimeType?.startsWith('image/')` → `null` (`:12-14`). optional chaining: `mimeType===undefined`이면 `undefined?.startsWith`→`undefined`, `!undefined`→`true`→null. **`'image/'` 접두 케이스 민감** — `'IMAGE/png'`은 false→null.
- `cleaned = base64Content.replace(/\s/g, '')` (`:15`) — **모든 공백 전역 제거.**
- `!cleaned` → `null` (`:16-18`). 공백 제거 후 빈 문자열→null.
- return `` `data:${mimeType};base64,${cleaned}` `` (`:19`). **base64를 디코드/재인코딩하지 않음 — 검증도 없음.** 그냥 문자열 연결.

### 트랩 클래스 발생 지점 (★ 최고 위험)
- **`.replace(/\s/g, '')` (`:15`) — U+FEFF/U+0085 양방향 발산.** JS `\s`(비-unicode 플래그) 집합 =
  `{0009,000A,000B,000C,000D,0020,00A0,1680,2000–200A,2028,2029,202F,205F,3000,`**`FEFF`**`}`.
  Rust `char::is_whitespace()`(Unicode White_Space) = `{0009–000D,0020,`**`0085`**`,00A0,1680,2000–200A,2028,2029,202F,205F,3000}`.
  - **U+FEFF(BOM/ZWNBSP):** JS `\s`는 제거, Rust `is_whitespace`는 **제거 안 함** → 포트가 `is_whitespace` 쓰면 BOM이 base64에 남아 URI 오염.
  - **U+0085(NEL):** Rust `is_whitespace`는 제거, JS `\s`는 **제거 안 함** → 포트가 JS가 남기는 바이트를 삭제.
  주석 `:3-5`이 명시: git diff/SSH 스트림의 line-wrapped base64를 정리하려는 의도 → **BOM 유입 현실적.**
  → **포트는 JS `\s` 집합을 손으로 정의한 predicate로 필터해야 한다**(FEFF 포함, 0085 제외). `str::trim`/`char::is_whitespace`/`split_whitespace` **금지.** 이것이 MEMORY의 `U+FEFF/U+0085` 함정 본체.
- **`startsWith('image/')` (`:12`) — 케이스 민감, 폴드 없음.** Rust `str::starts_with`도 케이스 민감 → 일치. `'Image/png'`/`'IMAGE/PNG'`은 양쪽 null. **`to_lowercase` 적용 금지**(원본이 대문자 MIME을 거부하는 게 정상 동작).
- **base64 인코딩 없음 (`:19`)** — 프롬프트 의심과 달리 **인코딩/디코딩/패딩/알파벳 결정이 없다.** caller가 준 이미 인코딩된 문자열을 공백만 벗겨 verbatim 연결. → 유일한 "인코딩" 관심사는 `\s` strip 뿐. `base64` crate 불필요.
- **optional chaining `mimeType?.` (`:12`)** — Rust `Option<&str>`, `None`→null.
- MIME은 출력에 verbatim 삽입(`:19`) — 새니타이즈 없음(`'image/svg+xml'`→그대로).

### 오라클 (image-data-uri.test.ts, 케이스별)
- `:6` `('image/png','bmV3')` → `'data:image/png;base64,bmV3'` · 기본 build.
- `:10` `('image/png','bm\nV3\t bmV3\r\n')` → `'…base64,bmV3bmV3'` · **크럭스: `\n \t space \r` strip — 단 ASCII 공백만.**
- `:16` `('image/png','   \n')` → `null` · strip 후 빈 문자열.
- `:19` `(undefined,'bmV3')` → `null` · MIME 누락.
- `:23` `('application/pdf','JVBER')` → `null` · 비-이미지.
- `:27` `('application/octet-stream','AAAA')` → `null` · 일반 비-이미지.

### 오라클이 안 치는 것 (추가 핀 필수 — 발산 불가시)
- **유니코드 공백 미검증:** 테스트는 `\n\t\r␠`(ASCII)만. **`﻿`/` `/``를 payload에 넣은 핀 필수** — 예: `buildImageDataUri('image/png','bm\u{FEFF}V3')` → `'…base64,bmV3'`(FEFF 제거), `('image/png','bm\u{0085}V3')` → **`'…base64,bm\u{0085}V3'`**(NEL 유지, JS는 안 벗김). 이 핀 없으면 `\s` 발산이 오라클에 안 보임.
- **대문자 MIME 거부:** `('IMAGE/PNG','bmV3')` → `null` 핀(케이스 민감 고정).
- **`'image/'` 정확(빈 서브타입):** `('image/','bmV3')` → `'data:image/;base64,bmV3'` 핀(startsWith true).

---

## 5. `osc-title-scan-tail.ts` (36L) — 터미널 OSC 꼬리, UTF-16 length vs byte-len

### 공개 표면
- `extractOscTitleScanTail(input: string): string` (`osc-title-scan-tail.ts:5-15`) — export, 순수.
- private: `extractIncompleteTitleOscTail(suffix)` (`:17-25`), `trimOscTitleScanTail(value)` (`:27-36`).
- const: `OSC_TITLE_SCAN_TAIL_LIMIT=4096` (`:1`), `OSC_TITLE_PREFIX_LENGTH=4` (`:2`), `OSC_TITLE_CODES=Set{'0','1','2'}` (`:3`).
- 클럭·import 0.

### 정확 시맨틱 — `extractOscTitleScanTail` (`:6-14`)
- `lastOsc = input.lastIndexOf('\x1b]')` (`:6`). ESC(U+001B)+`]`(U+005D) — OSC introducer의 **마지막** 위치.
- `lastOsc !== -1` (`:7`): `suffix = input.slice(lastOsc)` (`:8`).
  - `!suffix.includes('\x07') && !suffix.includes('\x1b\\')` (`:9`) — BEL(U+0007)도 ST(ESC+`\`)도 없음 = **미완결 OSC** → `extractIncompleteTitleOscTail(suffix)` (`:10`).
  - else(터미네이터 존재 = 완결) → `input.endsWith('\x1b') ? '\x1b' : ''` (`:12`).
- else(OSC introducer 없음) → `input.endsWith('\x1b') ? '\x1b' : ''` (`:14`).

### 정확 시맨틱 — `extractIncompleteTitleOscTail(suffix)` (`:18-24`)
- `parameterEnd = suffix.indexOf(';', 2)` (`:18`) — index 2(ESC·`]` 다음)부터 `;` 탐색.
- `parameterEnd === -1`(아직 `;` 없음) (`:19`): `partialParameter = suffix.slice(2)` (`:20`); `['','0','1','2'].includes(partialParameter) ? trim(suffix) : ''` (`:21`). **빈 문자열 `''` 포함**(suffix가 정확히 `\x1b]`일 때).
- else: `parameter = suffix.slice(2, parameterEnd)` (`:23`); `OSC_TITLE_CODES.has(parameter) ? trim(suffix) : ''` (`:24`). `'0'/'1'/'2'`만 유지.

### 정확 시맨틱 — `trimOscTitleScanTail(value)` (`:28-35`)
- `value.length <= 4096` → `value` (`:28-30`).
- `prefix = value.slice(0, Math.min(4, value.length))` (`:33`).
- `suffixBudget = Math.max(0, 4096 - prefix.length)` (`:34`).
- return `` `${prefix}${value.slice(-suffixBudget)}` `` (`:35`). introducer 보존 + 최신 payload 유지(주석 `:31-32`).

### 트랩 클래스 발생 지점 (★ UTF-16 length)
- **`.length`/`.slice(-N)` (`:28`, `:33`, `:34`, `:35`) — UTF-16 code-unit vs Rust byte-len 발산.** JS `length`는 UTF-16 단위, `slice(-suffixBudget)`는 UTF-16 단위 절단. 이모지=surrogate pair=2단위. Rust `str`는 UTF-8 바이트:
  - **단위 불일치:** 4096 임계·4-char prefix·suffixBudget이 JS에선 UTF-16 단위. 비-ASCII 타이틀이 4096 근방이면 byte-len/char-count와 발산.
  - **`slice(-N)` 절단 안전성:** JS는 surrogate pair를 쪼개도 lone surrogate(여전히 유효 JS 문자열). **Rust byte 슬라이스가 char boundary 아니면 패닉.** → **포트는 char boundary 보정 필수, 패닉 금지.**
  → 도달 조건이 `value.length > 4096`뿐이고 **테스트가 이 경로를 0건** 침(§오라클) → 단위 선택(byte 권장 — 터미널 스트림 충실성)을 Codex에 올리고 **비-ASCII·>4096 핀 추가.**
- **`indexOf`/`lastIndexOf`/`includes`/`endsWith`/앞쪽 `slice` (`:6`,`:8`,`:9`,`:12`,`:14`,`:18`,`:20`,`:23`) — ASCII needle라 byte-safe.** 탐색 대상(`\x1b]`, `;`, `\x07`, `\x1b\\`, `\x1b`)이 전부 ASCII/제어문자 → Rust `rfind`/`find`가 반환하는 byte offset에서 슬라이스해도 멀티바이트 UTF-8 내부를 가르지 않음(ASCII 바이트는 시퀀스 내부에 안 나타남). **introducer/param/terminator 스캔은 안전, `trimOscTitleScanTail`만 위험.**
- **`\x1b\\` (`:9`) = 2문자(ESC+backslash).** Rust `"\u{1b}\\"`. `\x07`=`"\u{07}"`.
- **`Set.has` (`:24`) / 배열 `.includes` (`:21`)** — `'0'/'1'/'2'`(+빈 문자열 `:21`). Rust match/집합.
- trim()/case/toFixed/Date/base64 없음(`trim`은 커스텀 함수명, `str.trim` 아님).

### 오라클 (osc-title-scan-tail.test.ts, 케이스별)
- `:6` `'\x1b]0;Codex work'` → `'\x1b]0;Codex work'` · 미완결, `;` 전 param `'0'`→타이틀 코드→전체 유지.
- `:7` `'\x1b]2;Codex working\x1b'` → 동일 · 후행 lone ESC는 suffix 일부(ST 아님), param `'2'`→유지(후행 ESC 포함).
- `:8` `'\x1b]'` → `'\x1b]'` · bare introducer, partial param `''`→유지.
- `:9` `'\x1b]1'` → `'\x1b]1'` · `;` 없음, partial `'1'` ∈ 리스트→유지.
- `:13` `'\x1b]133;D;13'` → `''` · param `'133'` 비-타이틀→폐기.
- `:14` `'\x1b]7;file://host/tmp'` → `''` · param `'7'`(하이퍼링크)→폐기.
- `:15` `'\x1b]133;D;0\x07\x1b'` → `'\x1b'` · **크럭스: suffix에 BEL(완결 분기)→`input.endsWith('\x1b')`→후행 ESC만 반환.**

### 오라클이 안 치는 것 (추가 핀 필수)
- **`trimOscTitleScanTail`(>4096) 경로 0건** — prefix/suffixBudget/`slice(-N)` 전부 미검증. **여기가 UTF-16 발산 서식지.** `>4096`자 + 비-ASCII payload 핀 필수(단위·패닉 고정).
- **`lastOsc === -1` 분기(`:14`) 미검증** — OSC introducer 아예 없는 입력. `extractOscTitleScanTail('hello')` → `''`, `extractOscTitleScanTail('hello\x1b')` → `'\x1b'` 핀.
- **ST(ESC+`\`) 터미네이터 분기 미검증** — `:15`는 BEL만. `extractOscTitleScanTail('\x1b]0;t\x1b\\')` → `''`(완결, 후행 ESC 아님) 핀.
- **비-ASCII 타이틀 payload**(이모지/CJK)로 introducer 스캔 byte-safety 확인 핀.

---

## 6. `stable-pane-id.ts` (67L) — **해시 없음**, UUID 소문자 전용·`\d`·trim 발산

### 공개 표면
- 브랜드 타입 `StablePaneId`/`TerminalLeafId`/`PaneKey` (`stable-pane-id.ts:10-12`) — **컴파일타임 전용**(`unique symbol` 브랜드), 런타임 표현 없음. Rust: newtype(`struct StablePaneId(String)`) 또는 검증만 하고 `String`.
- `isStablePaneId(value: string): boolean` (`:14-16`).
- `isTerminalLeafId(value: string): boolean` (`:18-20`) — `isStablePaneId`와 동일.
- `makePaneKey(tabId: string, stableLeafId: string): PaneKey` (`:22-30`) — **throw**.
- `parsePaneKey(paneKey: string): {tabId, leafId, stablePaneId} | null` (`:32-45`).
- `parseLegacyNumericPaneKey(paneKey: unknown): {tabId, numericPaneId, paneKey} | null` (`:47-67`).
- `UUID_RE` (`:4`). 클럭·import 0. **해시/crypto 0 — caller UUID 검증·조합뿐.**

### 정확 시맨틱
- `UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/` (`:4`). **소문자 hex 전용(`i` 플래그 없음)**, version `[1-5]`, variant `[89ab]`, `^…$` 앵커.
- `isStablePaneId` (`:15`): `UUID_RE.test(value)`. `isTerminalLeafId` (`:19`): 위임.
- `makePaneKey` (`:23-29`): `!tabId || tabId.includes(':')` → throw `'tabId must be non-empty and must not contain ":"'` (`:23-25`); `!isTerminalLeafId(stableLeafId)` → throw `'stableLeafId must be a UUID'` (`:26-28`); return `` `${tabId}:${stableLeafId}` `` (`:29`). **`!tabId` = 빈 문자열(falsy).**
- `parsePaneKey` (`:35-44`): `first = indexOf(':')`; `first <= 0 || first !== lastIndexOf(':') || first === length-1` → null (`:36-38`). (`first<=0`: `:` 없음 또는 index 0 = 빈 tab; 중간: `:` 2개↑; 끝: 빈 leaf.) `tabId=slice(0,first)`, `leafId=slice(first+1)`; `!isTerminalLeafId(leafId)` → null; return `{tabId, leafId, stablePaneId: leafId}`.
- `parseLegacyNumericPaneKey` (`:50-66`): `typeof paneKey !== 'string' || paneKey.length > 256` → null (`:50-52`); `trimmed = paneKey.trim()` (`:53`); `delimiter = trimmed.indexOf(':')`; `delimiter <= 0 || delimiter !== lastIndexOf(':') || delimiter === length-1` → null (`:55-61`); `numericPaneId = trimmed.slice(delimiter+1)` (`:62`); `!/^\d+$/.test(numericPaneId)` → null (`:63-65`); return `{tabId: trimmed.slice(0,delimiter), numericPaneId, paneKey: trimmed}` (`:66`). **반환 paneKey는 trim된 값.**

### 트랩 클래스 발생 지점 (★ 3종 오라클 침묵)
- **UUID_RE 소문자 전용·대문자 거부 (`:4`) — `i` 플래그 없음.** 포트는 **정확 소문자 `[0-9a-f]` 매칭.** `to_ascii_lowercase()` 후 매칭 금지(대문자 허용됨). Rust `regex`면 `(?i)` 금지, 손-롤이면 `matches!(c, '0'..='9' | 'a'..='f')`. **version `[1-5]`, variant `[89ab]` 제약도 정확히.** → **대문자 케이스가 오라클에 없음(아래).**
- **`/^\d+$/` (`:63`) — Rust `\d`가 유니코드 숫자 먹음.** JS `\d`(비-unicode 정규식) = **ASCII `[0-9]`만.** Rust `regex`의 `\d`는 **기본 Unicode-aware** → Arabic-Indic `٠-٩`, fullwidth `０-９` 등 매칭. `'tab-1:١٢'`가 Rust에선 통과, JS에선 null. → **포트는 `[0-9]`/ASCII-only 검사**(`(?-u:\d)` 또는 `bytes.all(|b| b.is_ascii_digit())`). **`\d` 그대로 금지.**
- **`.trim()` (`:53`) — U+FEFF/U+0085 발산(§4와 동일).** JS `trim`은 `\s`와 같은 집합(**FEFF 제거, 0085 제거 안 함**), Rust `str::trim`은 `char::is_whitespace`(**0085 제거, FEFF 제거 안 함**). → 마이그레이션 경로라 stakes는 낮으나 충실 포트는 **JS-trim predicate(FEFF 포함/0085 제외)** 필요. `str::trim` 그대로면 BOM 접두 legacy key에서 발산.
- **`.length > 256` (`:51`), `length-1` (`:36`,`:38`,`:60`) — UTF-16 단위.** 256 캡이 JS UTF-16 단위. 비-ASCII tabId가 256 근방이면 Rust byte-len과 발산(저 stakes). `:` needle 슬라이스(`:39`,`:40`,`:62`,`:66`)는 ASCII라 byte-safe.
- **해싱/결정성 함정 없음** — 프롬프트 의심과 달리 **해시 함수 0.** "stable id"는 caller UUID 그 자체; 이 모듈은 검증·`${tab}:${leaf}` 조합뿐. crypto/uuid crate 불필요.
- **브랜드 타입 (`:6-12`)** — 런타임 없음. Rust는 newtype 또는 검증-후-`String` 선택 자유.
- **`parseLegacyNumericPaneKey`의 `unknown` (`:50` `typeof !== 'string'`)** — Rust 타입이 `&str`이면 이 가드는 모트. 호출부가 비-문자열을 `None`/에러로 걸러줘야 하는지 결정.

### 오라클 (stable-pane-id.test.ts, 케이스별). `LEAF_ID='11111111-1111-4111-8111-111111111111'`(v4, variant 8) (`:10`)
- `:14-15` `isStablePaneId(LEAF_ID)`/`isTerminalLeafId(LEAF_ID)` → `true`.
- `:19-22` `['1','pane:1','11111111-1111-6111-8111-111111111111','']` 각각 both `false` · **크럭스: `'1'` 너무 짧음; `'pane:1'` `:` 포함; 세 번째는 version `'6'`(`[1-5]` 위반)→거부(version 제약 핀); `''` 빈.**
- `:26-34` `makePaneKey('tab-1',LEAF_ID)` → `'tab-1:{LEAF_ID}'`; `parsePaneKey` → `{tabId:'tab-1', leafId:LEAF_ID, stablePaneId:LEAF_ID}`.
- `:37` `makePaneKey('',LEAF_ID)` throw `/tabId/` · 빈 tab.
- `:38` `makePaneKey('tab:1',LEAF_ID)` throw `/tabId/` · tab에 `:`.
- `:39` `makePaneKey('tab-1','1')` throw `/UUID/` · 나쁜 leaf.
- `:43` `parsePaneKey('tab-1:1')` → `null` · leaf `'1'` 비-UUID.
- `:44` `parsePaneKey('tab:1:{LEAF_ID}')` → `null` · `:` 2개(first!==last).
- `:45` `parsePaneKey(':{LEAF_ID}')` → `null` · first===0(빈 tab).
- `:46` `parsePaneKey('tab-1:')` → `null` · first===length-1(빈 leaf).
- `:50-54` `parseLegacyNumericPaneKey(' tab-1:12 ')` → `{tabId:'tab-1', numericPaneId:'12', paneKey:'tab-1:12'}` · **크럭스: trim 적용(양끝 공백 제거), 반환 paneKey는 trim된 값.**
- `:55` `parseLegacyNumericPaneKey('tab-1:{LEAF_ID}')` → `null` · UUID는 all-digit 아님(`\d+` 실패).
- `:56` `parseLegacyNumericPaneKey('tab:1:12')` → `null` · `:` 2개.

### 오라클이 안 치는 것 (추가 핀 필수 — 3종 발산 불가시)
- **UUID 대문자 거부 미검증:** 대문자 hex 케이스 0건. **`isStablePaneId('AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA')` → `false`** 핀 필수 — case-insensitive 포트가 전부 통과.
- **`\d` 유니코드 미검증:** `parseLegacyNumericPaneKey('tab-1:١٢')`(Arabic) → **`null`** 핀 필수 — Rust `\d` 포트가 통과해버림.
- **`.trim()` FEFF/0085 미검증:** `parseLegacyNumericPaneKey('\u{FEFF}tab-1:12\u{FEFF}')` 동작 핀(JS는 BOM trim). 저 stakes지만 충실성 확인.
- **variant `[89ab]` 미검증:** `isStablePaneId('11111111-1111-4111-c111-111111111111')`(variant `c`) → `false` 핀(version만 `:20`에서 커버).
- **비-문자열 입력(`:50`)·`length>256` 캡 미검증** — Rust 타입에 따라 모트일 수 있음(저 우선순위).

---

## 7. Codex 교차검증 공개 질문 (consolidated)

1. **[클럭 주입]** `rate-limit-reset-format`은 소스가 이미 `now`를 파라미터로 받아 **`Date.now()` 호출 0** — 추가 주입 불필요. 확인: 배치 어디에도 숨은 클럭 의존 없음(6개 전부 순수·결정론). ✔ 이 판단 승인?
2. **[정수 타입 결정]** `rate-limit-reset-format`의 `ms`/`now`/`resetTimes`를 **`i64`(epoch-ms)** 로 받을지 `f64`로 받을지. `i64`면 `Math.floor`=정수 나눗셈, `%`=truncated remainder로 정확 일치하고 `.0` 유출 없음. 단 `i64`면 `Number.isFinite`(`:51`) 가드가 **모트**가 됨(NaN/∞ 표현 불가) — 호출부에서 비유한을 어떻게 거를지. **권장: `i64` + 호출부 필터.** 승인?
3. **[Math.round 발산]** `usage-percentage-display`의 `Math.round`(`:17`,`:34`)는 음수 .5에서 `f64::round`와 다르나 **양쪽 클램프가 0으로 흡수** → 실질 무해. 진짜 계약은 **round-then-complement 순서**(#7574). `f64::round` 그대로 쓰고 순서만 못박는 것 승인? (추가 핀: 음수 .5 through clamp.)
4. **[JS-`\s`/JS-trim predicate]** `image-data-uri`(`:15` `/\s/g`)와 `stable-pane-id`(`:53` `.trim()`)는 **U+FEFF 포함/U+0085 제외**의 JS 공백 집합이 필요. Rust `char::is_whitespace`/`str::trim`은 **반대**(0085 포함/FEFF 제외). → **공유 `is_js_whitespace(c)` predicate를 `suaegi-misc`에 한 벌 정의**(둘이 공유)하는 게 맞나, 아니면 각 모듈 로컬? 정확 집합: `{U+0009..U+000D, U+0020, U+00A0, U+1680, U+2000..U+200A, U+2028, U+2029, U+202F, U+205F, U+3000, U+FEFF}`. 승인?
5. **[유니코드 `\d`]** `stable-pane-id`(`:63` `/^\d+$/`)는 **ASCII `[0-9]` 전용**이어야 함(Rust `regex` `\d`는 유니코드 숫자 매칭). `(?-u:\d)`/`is_ascii_digit` 중 무엇? 그리고 UUID_RE(`:4`)는 **소문자·대소문자 구분** — `(?i)`/`to_ascii_lowercase` 금지 확인.
6. **[base64/data-URI 인코딩 선택]** `image-data-uri`는 **base64를 인코딩하지 않는다** — caller 제공 문자열을 공백 strip 후 verbatim concat(`:19`). 따라서 `base64` crate·알파벳·패딩 결정 **불필요.** 이 판단(인코딩 없음) 승인? 유일 인코딩 관심사는 §4의 `\s` strip.
7. **[UTF-16 단위 vs byte-len]** `osc-title-scan-tail`의 `trimOscTitleScanTail`(`:28-35`, 도달=`length>4096`)과 `stable-pane-id`의 `length>256`(`:51`)은 JS **UTF-16 단위**. 포트 단위: **byte**(터미널 스트림 충실) 권장하되 `slice(-N)`가 char boundary를 가르면 **패닉 금지(경계 보정).** 이 경로는 **오라클 0건** — byte 선택 + 비-ASCII·초과길이 핀 추가 승인?
8. **[secretly impure/coupled?]** 6개 중 배치에서 빼야 할 모듈 없음 — 전부 zero-import·순수·해시 없음·클럭 없음. 특히 `stable-pane-id`는 **해시 전혀 없음**(caller UUID 검증만). ✔ 승인?
9. **[추가 핀 총괄]** 각 §의 "오라클이 안 치는 것"이 뮤테이션-검증 회귀 테스트로 승격돼야 함. 최우선: (a) §4 유니코드 공백 payload, (b) §6 UUID 대문자 거부 + `\d` 유니코드 숫자 거부, (c) §2 days-분기 분 드롭, (d) §5 `>4096`+비-ASCII trim 경로 + `lastOsc===-1` 분기. 이 4묶음이 **통과 테스트만으론 안 잡히는** 발산.
