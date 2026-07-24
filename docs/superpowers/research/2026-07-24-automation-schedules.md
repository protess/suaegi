# automation-schedules 조사: cron + RRULE 스케줄링 엔진

> 2026-07-24. Orca v1.4.150-rc.0 소스를 **직접 읽고** `file:line`으로 인용한다.
> 구현하지 않는다 — 이 문서가 포팅 계약의 증거 기반이다. 서브에이전트가 verbatim 포팅한다.
> 인용 경로 표기: 별도 명시 없으면 전부 `src/shared/automation-schedules.ts`.
> 다른 파일은 파일명 명시(`automations-types.ts:32` 등).
>
> **가장 중요한 발견 세 줄:**
> 1. **RRULE·cron 모두 hand-rolled.** 서드파티 `rrule`/`cron` 라이브러리 의존이 **0건**이다
>    (import는 `AutomationSchedulePreset` 타입과 `isClipboardTextByteLengthOverLimit` 뿐 — `:3-4`).
>    지원 RRULE는 `HOURLY`/`DAILY`/`WEEKLY` + `BYHOUR`/`BYMINUTE`/`BYDAY`의 **극소 부분집합**뿐.
> 2. **Vixie cron의 DOM/DOW OR 규칙을 재현하되, "restricted" 판정이 Orca 고유의 크기 휴리스틱이다.**
>    `daysOfMonth.size !== 31`, `daysOfWeek.size !== 7`(`:208-209`). `*/1`, `0-7`, `1-7`은 전부
>    "unrestricted"로 접힌다. 표준 cron 크레이트와 갈라지는 핵심 지점.
> 3. **모든 날짜 산술이 시스템 로컬 타임(wall-clock)이다.** `Automation.timezone` 필드는 존재하나
>    이 모듈은 **쓰지 않는다** — `new Date().getHours()/getDate()/getDay()` 로컬 계열만 쓴다.
>    테스트 리터럴도 `Z` 없는 로컬 시각. Rust 포팅은 테스트를 **고정 TZ로 핀**해야 결정론적이다(§5).

---

## 0. 요약 — 이 조사가 확정한 사실

1. **순수/시계의존 분리.** `nextAutomationOccurrenceAfter`/`latestAutomationOccurrenceAtOrBefore`는
   시각을 인자로 받아 `Date.now()`를 **안 읽는다**(시계-순수). 단 내부 산술이 로컬 TZ 의존이라
   "완전 순수"는 아니다. `isValid*`/`classify*`/`format*`은 `Date.now()`를 읽는다(§1, §5).
2. **cron 필드 파서는 이름 테이블을 month·day-of-week에만 붙인다**(`:206`, `:198`). minute/hour/day-of-month은
   숫자만(`:203-204`, `:187-192`). 7→0 정규화는 day-of-week 전용(`:199`).
3. **DoS 가드가 토크나이즈보다 먼저 온다.** `getAutomationCronExpressionFields`는 regex-free 이며
   2048바이트 초과 입력을 토큰화 전에 `[]`로 잘라낸다(`:213-216`, `AUTOMATION_CRON_EXPRESSION_MAX_BYTES=2*1024`).
4. **스캔 창 `CRON_SCAN_DAYS = 9*366 = 3294`일**(`:10`). 비윤년 세기(2100 등)를 낀 8년 Feb-29 갭을 커버.
   분 단위 스캔은 `CRON_SCAN_MINUTES = 3294*24*60 = 4,743,360`(`:11`).
5. **오프-더-미닛 dtstart는 즉발하지 않고 전진한다.** cron/hourly 모두 `<` vs `<=` 경계가 크럭스(§4).
6. **레이블 포맷(`Intl.DateTimeFormat`)은 로케일·TZ 의존 표현 계층** — 결정론적 코어(classify의 `kind`,
   `hour`/`minute`/`dayOfWeek`)에서 반드시 격리(§6).

---

## 1. 공개 표면 (exported surface)

| export | 시그니처(요약) | 반환 | 순수성 | 인용 |
|---|---|---|---|---|
| `AUTOMATION_CRON_EXPRESSION_MAX_BYTES` | const | `2048` (=2*1024) | const | `:12` |
| `AutomationCronScheduleClassification` | type union | 6-variant(`hourly`/`daily`/`weekdays`/`weekly`/`custom`/`invalid`) | type | `:35-41` |
| `getAutomationCronExpressionFields(expr, maxFields=5)` | tokenizer | `string[]` | **순수** | `:213-236` |
| `isValidAutomationSchedule(schedule)` | rrule OR cron 판정 | `boolean` | 시계의존(`Date.now()`) | `:262-272` |
| `isValidAutomationCronSchedule(schedule)` | cron 전용 판정 | `boolean` | 시계의존 | `:274-281` |
| `parseAutomationRrule(rrule)` | 편집용 파싱 | `{preset,hour,minute,dayOfWeek}` (throws) | **순수** | `:283-313` |
| `tryParseAutomationRrule(rrule)` | 위의 null-안전판 | `... | null` | **순수** | `:315-323` |
| `classifyAutomationCronSchedule(schedule)` | cron 분류 | `AutomationCronScheduleClassification` | 시계+로케일 의존 | `:424-432` |
| `formatAutomationSchedule(scheduleExpression)` | 사람용 레이블 | `string` | 시계+로케일 의존 | `:434-445` |
| `buildAutomationRrule(args)` | preset→RRULE 문자열 | `string` | **순수** | `:522-541` |
| `buildAutomationCronSchedule(args)` | preset→cron 문자열 | `string` | **순수** | `:543-562` |
| `nextAutomationOccurrenceAfter(rrule, dtstart, after)` | 다음 발생 | `number` (throws) | **시계-순수**(TZ의존) | `:564-604` |
| `latestAutomationOccurrenceAtOrBefore(rrule, dtstart, now)` | ≤now 최근 발생 | `number | null` | **시계-순수**(TZ의존) | `:606-636` |

내부(비-export) 핵심: `parseRrule`(`:70`), `parseCronNumber`(`:101`), `parseCronField`(`:111`),
`parseCronExpression`(`:181`), `isAutomationCronFieldWhitespace`(`:238`), `parseSchedule`(`:254`),
`cronMatches`(`:490`), `cronDateMatches`(`:498`), `cronHasPossibleOccurrence`(`:511`),
`scanDayCandidates`(`:467`), `dayMatches`(`:459`), `atLocalTime`(`:447`), `startOfLocalDay`(`:453`),
`floorToMinute`(`:484`), `formatTime`(`:325`), `classifyParsedCronSchedule`(`:377`).

**디스패치 규칙(크럭스):** `parseSchedule`(`:254-260`)은 `trimmed.includes('=')`이면 RRULE, 아니면 cron.
즉 `=`가 하나라도 있으면 RRULE 경로. 그래서 `FREQ=DAILY;...`는 절대 cron 파서에 안 닿는다 —
`isValidAutomationCronSchedule`는 `parseCronExpression`을 **직접** 부르므로 RRULE 문자열을 거부한다(§7 케이스 20).

---

## 2. Cron 시맨틱 (정확히)

### 2.1 필드 토크나이즈 — `getAutomationCronExpressionFields` (`:213-236`), regex-free

- **가드 우선(`:214-216`):** `isClipboardTextByteLengthOverLimit(expression, 2048)`이 true면 **즉시 `[]`**.
  토큰화 루프에 진입조차 안 한다. → 오버사이즈 붙여넣기 DoS 차단.
- **수동 문자 루프(`:219-234`):** `charCodeAt` + `isAutomationCronFieldWhitespace`로 토큰 경계 판정.
  `maxFields`에 도달하면 break(`:230-232`). 정규식 `\s+` split을 **절대 안 쓴다**(테스트가 이를 assert, §7 케이스 13·14).
- **왜 regex-free(DoS):** 공격자 클립보드 텍스트에 대한 `\s+` 정규식은 대용량 입력에서 병목/역추적 위험.
  바이트 가드가 먼저, 그다음 O(n) 선형 스캔.
- **공백 코드포인트(`:238-252`):** ASCII space(32), `\t\n\v\f\r`(9–13), NBSP(160), OGHAM(5760),
  EN QUAD..HAIR SPACE(8192–8202), LINE SEP(8232), PARA SEP(8233), NNBSP(8239), MMSP(8287),
  IDEOGRAPHIC SPACE(12288), BOM/ZWNBSP(65279). → 유니코드 공백으로 붙여넣기해도 필드 분리됨.
- **필드 수 검증:** `parseCronExpression`은 `getAutomationCronExpressionFields(expression, 6)`으로
  **6개까지** 받아본 뒤 `parts.length !== 5`면 "Cron schedule must have five fields." throw(`:182-185`).
  → 필드 5개 초과 감지용으로 maxFields=6.

### 2.2 필드 파서 — `parseCronField` (`:111-179`)

리스트 → 스텝 → 범위/와일드카드/단일값 순으로 해석:
- **리스트(`,`):** `args.value.split(',')`(`:120`). 각 part `trim()`, 빈 문자열이면 throw(`:121-124`).
- **스텝(`/`):** `part.split('/')`, `length > 2`면 throw(`:125-128`). `rangePart` 없으면 throw(`:130-132`).
  step = 없으면 1, 있으면 `Number`, **정수·≥1 아니면 throw**(`:133-136`).
- **범위 해석(`:138-154`):**
  - `*` → `start=min, end=max`(`:140-142`).
  - `a-b`(`includes('-')`) → `split('-')` 길이 2·양끝 비어있지 않음 검증, 각각 `parseCronNumber`(`:143-150`).
  - 단일값 → `start=end=parseCronNumber(...)`(`:151-154`).
- **정규화 + 경계 검증(`:156-170`):** `normalize` 적용(있으면). `start`, `end`, `normalizedStart`,
  `normalizedEnd`가 모두 `[min,max]` 안이어야 하고 **`start > end`면 throw**. (정규화 전/후 값 **둘 다** 검사.)
- **전개(`:171-173`):** `for value=start; value<=end; value+=step` → `result.add(normalize(value) ?? value)`.
- **빈 결과 방지(`:175-177`):** `result.size === 0`이면 throw.

`parseCronNumber`(`:101-109`): `toUpperCase()` → 이름 맵 조회 → 없으면 `Number()`. **정수 아니면 throw**.

### 2.3 필드별 파라미터 — `parseCronExpression` (`:181-211`)

| 필드 | 순서 | min | max | names | normalize | 인용 |
|---|---|---|---|---|---|---|
| minute | 0 | 0 | 59 | — | — | `:203` |
| hour | 1 | 0 | 23 | — | — | `:204` |
| day-of-month | 2 | 1 | 31 | — | — | `:187-192` |
| month | 3 | 1 | 12 | `MONTH_NAMES` | — | `:206` |
| day-of-week | 4 | 0 | 7 | `DAY_NAMES` | `v===7?0:v` | `:193-200` |

**이름 테이블:**
- `MONTH_NAMES`(`:45-58`): `JAN=1 … DEC=12` (**1-인덱스**).
- `DAY_NAMES`(`:59-68`): 2글자 `SU=0,MO=1,…,SA=6`(= `DAY_CODES` 인덱스) **+** 3글자 `SUN=0,MON=1,…,SAT=6`.
  → `MON-FRI` = `parseCronNumber('MON')=1 .. parseCronNumber('FRI')=5` = 범위 `1..5`.
- `DAY_CODES = ['SU','MO','TU','WE','TH','FR','SA']`(`:43`) — RRULE BYDAY용 0-인덱스.
- `WEEKDAY_CODES = ['MO','TU','WE','TH','FR']`(`:44`).

**7→0 정규화(`:199`):** day-of-week에서 `7`은 일요일(0)로 접힌다. 그래서:
- `*` → 0..7 전개 후 정규화 → `{0,1,2,3,4,5,6}` size 7 → **unrestricted**.
- `0-7` → 정규화 후 `{0..6}` size 7 → **unrestricted** (§7 케이스 17).
- `1-7` → 정규화 후 `{1,2,3,4,5,6,0}` size 7 → **unrestricted** (§7 케이스 17). ← 7이 0으로 접혀 7일 전부가 됨.

**restricted 판정(크럭스, `:208-209`):**
```
dayOfMonthRestricted: daysOfMonth.size !== 31
dayOfWeekRestricted:  daysOfWeek.size !== 7
```
- `*/1`(day-of-month) → `1..31 step 1` = size 31 → **unrestricted** (§7 케이스 17: `0 9 */1 * MON`).
- 이건 표준 cron 라이브러리와 갈라지는 **가장 위험한 재현 포인트**. "필드가 정확히 전 범위 집합이면 unrestricted"라는
  집합-크기 휴리스틱이지, "`*` 리터럴이냐"가 아니다.

### 2.4 DOM/DOW OR-vs-AND (Vixie cron) — `cronDateMatches` (`:498-509`)

```
month 불일치 → false                               (:500-502)
dayOfMonthMatches = daysOfMonth.has(getDate())      (:503)
dayOfWeekMatches  = daysOfWeek.has(getDay())        (:504)
if (dayOfMonthRestricted && dayOfWeekRestricted)    (:505)
    return dayOfMonthMatches || dayOfWeekMatches    (:506)  ← 둘 다 제한 시 OR
return dayOfMonthMatches && dayOfWeekMatches         (:508)  ← 아니면 AND
```
- **둘 다 restricted → OR** (Vixie 규칙). 예: `0 9 1 * MON` = 매월 1일 **또는** 매주 월요일. → classify는
  단일 요일/일자로 접히지 않아 `custom`(§7 케이스 16).
- **한쪽만/양쪽 다 unrestricted → AND.** 예: `0 9 */1 * MON`은 DOM unrestricted(size31)이라 AND →
  월요일만(§7 케이스 17). `0 0 29 2 *`는 DOW unrestricted라 AND → Feb 29(§7 케이스 21).
  `0 0 31 2 *`도 AND → date31 && month Feb → 영원히 없음(§7 케이스 19).
- **시각 매칭:** `cronMatches`(`:490-496`)는 `cronDateMatches` 통과 후 `hours.has(getHours()) && minutes.has(getMinutes())`.

### 2.5 발생 가능성 검증 — `cronHasPossibleOccurrence` (`:511-520`)

```
day = startOfLocalDay(anchor)
for i in 0..CRON_SCAN_DAYS:            # 3294회
    if cronDateMatches(rule, day): return true   # 날짜만, 시/분 무시
    day += DAY_MS                       # 고정 24h 가산 (재-플로어 없음)
return false
```
- **`CRON_SCAN_DAYS = 9*366 = 3294`(`:10`).** 주석(`:9`): "Feb 29 같은 유효 cron은 비윤년 세기를 낀 8년 갭을
  가질 수 있다." (예 2096→2104, 2100 비윤년 스킵). 9년 창으로 확실히 포착.
- **DST 미세드리프트 주의:** `day += DAY_MS`는 고정 86,400,000ms 가산이라 재-`startOfLocalDay`를 안 한다.
  DST 경계를 넘으면 `day`가 로컬 자정에서 ±1h 어긋나지만, `cronDateMatches`는 월/일/요일만 보므로
  하루 경계를 실제로 넘지 않는 한 판정은 안정적. **엄밀 재현 시 개방질문**(§9).

---

## 3. RRULE 시맨틱 (정확히)

### 3.1 파싱 — `parseRrule` (`:70-99`)

- `rrule.split(';')` → 각 `key=value`, **키는 uppercase**로 맵에 저장(`:71-77`).
- `FREQ` ∈ {`HOURLY`,`DAILY`,`WEEKLY`} 아니면 throw `'Unsupported automation recurrence.'`(`:78-81`).
- `BYHOUR` 기본 `'9'`, `BYMINUTE` 기본 `'0'`(`:82-83`). byHour 정수 0..23, byMinute 정수 0..59 아니면 throw(`:84-89`).
- `BYDAY = (get('BYDAY') ?? '').split(',').filter(Boolean)`(`:90`).
- **WEEKLY 전용 검증(`:91-97`):** byDay가 비었거나 어떤 요소든 `DAY_CODES`(SU..SA)에 없으면
  throw `'Invalid recurrence day.'`. → **malformed BYDAY 거부**(§7 케이스 6·7).

### 3.2 편집용 매핑 — `parseAutomationRrule` (`:283-313`)

- `HOURLY` → `{preset:'hourly', hour:byHour, minute:byMinute, dayOfWeek:1}`(`:290-292`).
- `DAILY` → `{preset:'daily', ..., dayOfWeek:1}`(`:293-295`).
- `byDay.join(',') === WEEKDAY_CODES.join(',')`(정확히 `MO,TU,WE,TH,FR`) → `weekdays`(`:296-298`).
- 그 외 WEEKLY: `byDay.length !== 1`이면 throw(`:299-301`). 단일 코드의 `DAY_CODES.indexOf` <0이면 throw(`:302-306`).
  → `{preset:'weekly', hour, minute, dayOfWeek}`(`:307-312`). **일요일(0) 보존**(§7 케이스 5).

### 3.3 빌드 — `buildAutomationRrule` (`:522-541`) / `buildAutomationCronSchedule` (`:543-562`)

둘 다 `hour = clamp(floor(hour),0,23)`, `minute = clamp(floor(minute),0,59)`(`:528-529`, `:549-550`).

| preset | RRULE 출력 | cron 출력 |
|---|---|---|
| `hourly` | `FREQ=HOURLY;BYMINUTE=${m}` (**BYHOUR 없음**) `:530-532` | `${m} * * * *` `:551-553` |
| `weekdays` | `FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=${h};BYMINUTE=${m}` `:533-535` | `${m} ${h} * * 1-5` `:554-556` |
| `weekly` | `FREQ=WEEKLY;BYDAY=${DAY_CODES[clamp(dow,0,6)]};BYHOUR=${h};BYMINUTE=${m}` `:536-539` | `${m} ${h} * * ${clamp(dow,0,6)}` `:557-560` |
| `daily`(default) | `FREQ=DAILY;BYHOUR=${h};BYMINUTE=${m}` `:540` | `${m} ${h} * * *` `:561` |

**크럭스:** hourly RRULE은 BYHOUR를 **안 넣는다**. 그리고 아래 §4의 hourly 발생 계산도 **byHour를 무시**하고 byMinute만 쓴다.

---

## 4. 발생 계산 (정확히) — 경계 `<` vs `<=`가 핵심

### 4.1 `nextAutomationOccurrenceAfter(rrule, dtstart, after)` (`:564-604`)

`parseSchedule`로 cron/rrule 분기(`:569`).

**CRON 경로(`:570-588`):**
```
candidate = floorToMinute(max(dtstart, after))     (:571)
if candidate <= after:  candidate += MINUTE_MS      (:572-574)   ← after 이후로 엄격 전진
if candidate < dtstart:                              (:575)
    candidate = floorToMinute(dtstart)
    if candidate < dtstart: candidate += MINUTE_MS   (:576-579)   ← dtstart 이상 보장(오프-더-미닛 전진)
for i in 0..CRON_SCAN_MINUTES:                       (:581)       # 4,743,360회
    if cronMatches(rule, candidate): return candidate (:582)
    candidate += MINUTE_MS
throw 'Unable to compute next automation run.'       (:587)
```

**HOURLY 경로(`:589-597`):**
```
start = max(dtstart, after)
base = new Date(start); base.setMinutes(byMinute,0,0)   ← byHour 무시, 현재 시각의 분만 조정
candidate = base.getTime()
if candidate <= after || candidate < dtstart:  candidate += HOUR_MS   (:594-596)
return candidate
```
- `after`엔 `<=`(엄격 전진), `dtstart`엔 `<`(dtstart와 정확히 같으면 발화 허용). **오프-더-미닛 dtstart 케이스**(§7 케이스 2):
  dtstart 10:30 / after 09:00 / minute 0 → base 10:00 → `10:00<=09:00`? no, `10:00<10:30`? yes → +HOUR → **11:00**.

**WEEKLY/DAILY 경로(`:599-603`):** `scanDayCandidates(rule, max(dtstart-1, after), 1)`. null이면 throw.
- **`dtstart-1`(크럭스):** 아래 scanDayCandidates가 forward에서 `candidate > anchor`(엄격)를 쓰므로, dtstart와
  정확히 일치하는 발생을 살리려면 anchor를 1ms 당겨야 한다.

### 4.2 `latestAutomationOccurrenceAtOrBefore(rrule, dtstart, now)` (`:606-636`)

```
if now < dtstart: return null                        (:611)
```
**CRON(`:615-624`):** `candidate=floorToMinute(now)`, `for i<CRON_SCAN_MINUTES && candidate>=dtstart`:
`cronMatches`면 return, 아니면 `candidate -= MINUTE`. 못 찾으면 null. (dtstart 하한 포함.)

**HOURLY(`:625-633`):** `base.setMinutes(byMinute,0,0)`(byHour 무시). `candidate > now`면 `-= HOUR`.
`candidate >= dtstart ? candidate : null`.
- §7 케이스 1: hourly@9:00(minute 0), now 2026-05-13T14:20 → base 14:00, `14:00>14:20`? no → **14:00**. (byHour=9는 무시됨.)

**WEEKLY/DAILY(`:634-635`):** `scanDayCandidates(rule, now, -1)`, `>= dtstart`면 반환 else null.

### 4.3 `scanDayCandidates` (`:467-482`) / `dayMatches` (`:459-465`)

```
day = startOfLocalDay(anchor)
for i in 0..370:                                     # 최대 370일
    candidate = atLocalTime(day, byHour, byMinute)
    if dayMatches(rule, candidate):
        if direction==1  && candidate >  anchor: return candidate   ← forward 엄격
        if direction==-1 && candidate <= anchor: return candidate   ← backward 포함
    day += direction * DAY_MS
return null
```
- `dayMatches`(`:459-465`): `DAILY`면 항상 true. 아니면 `DAY_CODES[getDay()] in byDay`(로컬 요일).
- **반복 상한 370**(주 스케줄이 특정 요일을 반드시 1년 안에 찾도록 여유).
- forward=`>`(엄격), backward=`<=`(포함) — anchor 처리와 §4.1의 `dtstart-1`이 이 비대칭을 보정.

### 4.4 반복 상한 / 헬퍼 요약

- `CRON_SCAN_MINUTES = 4,743,360`(`:11`) — cron 다음/최근 발생 스캔.
- `scanDayCandidates` 370회, `cronHasPossibleOccurrence` 3294회.
- `atLocalTime`(`:447-451`): `d=new Date(dayMs); d.setHours(hour,minute,0,0)` → 그날 로컬 벽시각.
- `startOfLocalDay`(`:453-457`): `setHours(0,0,0,0)`. `floorToMinute`(`:484-488`): `setSeconds(0,0)`.

---

## 5. 타임존/시계 의존 (Rust 테스트 핀 근거)

**`Date.now()`를 읽는 곳(시계의존):** `isValidAutomationSchedule`(`:265`),
`isValidAutomationCronSchedule`(`:277`), `classifyParsedCronSchedule → cronHasPossibleOccurrence(rule, Date.now())`(`:378`),
`formatTime → new Date()`(`:326`). 이들은 `classifyAutomationCronSchedule`/`formatAutomationSchedule`로 전파.

**날짜 산술은 전부 로컬 TZ(wall-clock):** `getHours/getMinutes/getDate/getDay/getMonth`, `setHours/setMinutes/setSeconds`
— `atLocalTime`(`:449`), `startOfLocalDay`(`:455`), `floorToMinute`(`:486`), `cronMatches`(`:494-495`),
`cronDateMatches`(`:499-504`), `dayMatches`(`:463`), `scanDayCandidates` 경유 전부. **UTC 계열(`getUTC*`) 사용 0건.**

**`Automation.timezone` 필드(`automations-types.ts:116`)는 이 모듈에서 미사용.** 즉 스케줄 계산은
프로세스 로컬 타임존을 암묵적으로 쓴다. 이는 Orca가 스케줄러를 로컬 호스트 서비스로 돌린다는 전제(§8)와 연결.

**테스트 리터럴이 로컬 시각:** 오라클은 `new Date('2026-05-13T14:20:00')`(‼️ `Z` 없음)처럼 쓴다. 입력 리터럴과
내부 산술이 **같은 로컬 TZ**를 쓰므로 대부분 케이스는 TZ가 상쇄되지만, 주 스케줄(`getDay`)·DST 전이·자정 근처는
TZ 선택에 민감하다. **Rust 포팅 핀 전략:** 테스트를 고정 TZ(예 `TZ=America/Los_Angeles` 또는 UTC 고정)로 실행하고,
리터럴 파싱과 내부 산술을 **동일 TZ**로 맞춘다. `chrono-tz`/`chrono::Local`을 쓰되 테스트에서 결정론적으로 고정할 것.
(오라클 값의 요일: 2026-05-01=Fri, 05-15=Fri, 05-18=Mon, 2028-02-29=Tue — §7 검증에 사용.)

---

## 6. 로케일/포맷 계층 (결정론 코어에서 격리 대상)

- `formatTime(hour,minute)`(`:325-332`): `new Intl.DateTimeFormat(undefined,{hour:'numeric',minute:'2-digit'})`.
  → **로케일 + 시스템 TZ 의존** 표현. 결과 문자열(예 "10:15 AM")은 로케일 따라 다름.
- 요일 이름: `new Intl.DateTimeFormat(undefined,{weekday:'long'}).format(new Date(2026,0,4+dayOfWeek))`(`:371`, `:409`).
  - **앵커 `2026-01-04`는 일요일**(검증됨) → `4+0`=Sun … `4+6`=Sat. dayOfWeek 0..6 ↔ Sun..Sat 매핑.
  - en 로케일에서만 "Mondays"/"Sundays". 오라클은 `formatTimeForTest`(같은 Intl)로 비교해 **자기일관**하지만,
    Rust 포팅은 레이블 문자열을 **하드코딩 영어**로 재현하거나 로케일 계층을 분리해야 한다.
- `classifyParsedCronSchedule`(`:377-422`)의 **결정론 부분**: `kind`(hourly/daily/weekdays/weekly/custom/invalid),
  `minute`/`hour`/`dayOfWeek`는 TZ/로케일 무관. **`label` 문자열만 로케일 의존.** 포팅은 이 둘을 분리하라.
  - hourly 레이블은 `Intl` 없이 `:${padStart(2,'0')}`(`:362`, `:396`) — 로케일 무관.
- 분류 로직 세부: `getSingleSetValue`(`:334`), `setContainsExactly`(`:341`), `setContainsRange`(`:348`).
  hourly = minute 단일 && hours 0-23 전체 && 캘린더 unrestricted && DOW unrestricted(`:387-398`).
  daily/weekdays/weekly는 minute·hour 단일 && 캘린더 unrestricted 전제(`:399-420`).
  weekdays = `daysOfWeek === {1,2,3,4,5}`(`:404`). weekly = DOW 단일값(`:407`). 그 외 = `custom`(`:421`).
  발생 불가면 맨 앞에서 `invalid`(`:378-380`).

---

## 7. 오라클 (case-by-case, `automation-schedules.test.ts`)

21개 `it()`. 각 줄: 입력 → 기대 → 고정하는 크럭스.

1. **(`:31`)** hourly@9:00, dtstart 05-12T00:00, now 05-13T14:20 → **14:00**. `latest` hourly가 **byHour 무시**, 현재 시각 분만.
2. **(`:41`) [오프-더-미닛]** hourly@9:00(min0), dtstart 10:30, after 09:00 → **11:00**. `candidate<dtstart → +HOUR`(`:594-596`).
3. **(`:51`)** weekdays@9:30, dtstart 05-01, after 05-15T12:00 → `getDay()==1`,09:30. 주말 후보 배제(`dayMatches`).
4. **(`:63`)** weekly h16 m45 dow3 build→parse round-trip 동일. `DAY_CODES` 인덱스 왕복.
5. **(`:73`) [일요일 보존]** weekly dow0 → parse dow0(월요일로 강제 안 됨). `DAY_CODES[0]='SU'` 왕복(`:302-303`).
6. **(`:83`) [malformed BYDAY]** `BYDAY=NO` throw `'Invalid recurrence day.'`; `BYDAY=MO,NO` tryParse `null`. `:91-97`.
7. **(`:90`)** `FREQ=WEEKLY;BYHOUR=9;BYMINUTE=0`(BYDAY 없음): isValid **false**; next는 `'Invalid recurrence day.'` throw. WEEKLY는 byDay 필수.
8. **(`:103`)** `FREQ=YEARLY` → `'Invalid schedule'`. 미지원 FREQ 폴백(`:78-81` throw → `:442-444` catch).
9. **(`:107`)** `FREQ=HOURLY;BYMINUTE=5` → `'Hourly at :05'`. `padStart(2,'0')`(`:362`).
10. **(`:111`)** `15 10 * * 1-5`: next(dtstart 05-01, after 05-15T12:00) → **2026-05-18T10:15**(금 12:00 이후 → 월 10:15);
    latest → **2026-05-15T10:15**(금 10:15 < 12:00). cron 양방향 스캔 + DOW 범위.
11. **(`:127`)** buildCron: hourly→`15 * * * *`, daily→`15 9 * * *`, weekdays→`15 9 * * 1-5`, weekly dow0→`15 9 * * 0`.
12. **(`:140`)** 레이블: `5 * * * *`→Hourly:05; `15 10 * * *`→Daily; `15 10 * * MON-FRI`→Weekdays; `30 12 * * 7`→**Sundays**(7→0 정규화 + Intl).
13. **(`:149`) [regex-free 토크나이즈]** `'15'+NBSP(160)+'10\n*\t*\rMON-FRI'` → `['15','10','*','*','MON-FRI']`,
    format→Weekdays; `split(/\s+/)` 호출 **0건** assert. `:213-252`.
14. **(`:160`) [오버사이즈 DoS 가드]** `'secret-cron-field '.repeat(2048)` → getFields `[]`, isValidCron **false**;
    `\s+` split 0건. **바이트 가드가 토큰화보다 먼저**(`:214-216`).
15. **(`:171`)** classify: `15 10 * * MON-FRI`→`{weekdays,h10,m15}`; `30 12 * * 7`→`{weekly,h12,m30,dow0}`.
16. **(`:185`) [DOM/DOW OR & 미지원=custom]** `*/30 9-17 * * MON-FRI`, `0 9 1 * *`, `0 9 1 * MON`, `0 9,17 * * MON-FRI`
    → 전부 `'Custom schedule'`. `0 9 1 * MON`은 DOM·DOW **둘 다 restricted → OR** → 단일 분류 불가.
17. **(`:192`) [all-value 필드 = unrestricted]** `0 9 */1 * MON` next(after 05-15T12:00) → **2026-05-18T09:00**
    (`*/1`=size31 unrestricted → DOW와 AND → 월요일). `0 9 * * 0-7` valid, `0 9 * * 1-7` valid(둘 다 size7 unrestricted).
18. **(`:203`) [malformed separators]** `*/15/2 9 * * *`(스텝 2개, `:126-128`) false; `0 9 1--5 * *`(빈 범위 끝, `:145`) false.
19. **(`:208`) [no-possible-run]** `0 0 31 2 *`: isValid **false**, format `'Invalid schedule'`. DOM31 & Feb → AND 영구 불가;
    3294일 스캔 내 매치 0(`:511-520`).
20. **(`:213`) [cron-only 검증기 RRULE 거부]** daily RRULE: isValidAutomationSchedule **true**,
    isValidAutomationCronSchedule **false**(`parseCronExpression`가 `FREQ=...`를 5-필드로 못 봄 → throw).
21. **(`:219`) [leap-day]** `0 0 29 2 *` next(after 05-15T12:00, 2026) → **2028-02-29T00:00**. DOM29 & Feb → AND;
    2027 Feb 미존재, 2028 윤년. `CRON_SCAN_MINUTES` 창(≈9년)이 포착.

---

## 8. 동반 모듈 (M4)

### 8.1 `automation-run-retention.ts`

- **`MAX_AUTOMATION_RUNS_PER_AUTOMATION = 100`**(`:3`).
- **`pruneAutomationRuns(runs, maxPerAutomation=100)`**(`:7-27`):
  - **final 상태만 evict 대상**(`isFinalAutomationRunStatus`, `:14`). in-flight(pending/dispatching/dispatched)는 절대 삭제 안 함(`:26`).
  - `Map.groupBy`로 automationId별 그룹(`:15`). 정렬 `b.createdAt - a.createdAt || b.scheduledFor - a.scheduledFor`(`:18`)
    — createdAt 내림차순, 동률은 scheduledFor로 **결정론적 tie-break**.
  - `slice(0, Math.max(0, maxPerAutomation))`(`:20`) — **음수 cap 클램프**(음수 slice가 tail을 지우는 함정 방지).
  - **반환은 원본 append 순서 유지**(`:26`, `runs.filter`) — 호출자가 위치로 인덱싱.
- **`backfillAutomationRunNumbers(runs)`**(`:38-54`): automationId별 최고 runNumber 추적(`:39-45`), 미번호 run에
  `highest+1` 부여(`:46-53`). **append 위치가 아니라 최고 번호 이후로** 매김(다운그레이드 후 재발급 방지, 주석 `:29-37`).
- **`nextAutomationRunNumber(runsForAutomation)`**(`:57-65`): `reduce(max(n, runNumber ?? 0), 초기값=length) + 1`.
  레거시(번호 없음)도 count로 시드해 전진.

### 8.2 오라클 `automation-run-retention.test.ts` (요점)

- cap 이하 전량 유지(`:42`); 최신 N만(`:47`, 10→3 → `a-7,a-8,a-9`); automation별 독립 cap(`:52`);
  survivor append 순서 유지(`:60` → `a-2,a-3,b-2,b-3`); createdAt 동률 scheduledFor tie-break(`:66` → `y`);
  cap 0/음수 → `[]`(`:74`); **in-flight 절대 유지**(`:81` → old-pending/dispatching/dispatched 전부 생존);
  11,184 runaway → 400(`:98`). backfill: 위치 번호(`:111` → 1,1,2); 기존 번호 불변(`:120`);
  번호 재발급 금지(`:128` → 2,3); automation별 최고 survivor 위(`:138` → 200,7,201,8).
  nextRunNumber: 최고 survivor 계속(`:150` → 2797); 레거시 count 폴백(`:158` → 101); 신규 1(`:162`);
  prune 사이클 넘어 반복 없음(`:166` → 251).

### 8.3 `automation-run-identity.ts` (`:1-16`)

- `getAutomationLegacyRepoId({projectId})` → `projectId`(`:5-7`).
- `getAutomationRunRepoId(a)` → `a.runContext?.repoId ?? getAutomationLegacyRepoId(a)`(`:9-11`).
- `getAutomationRunProjectId(a)` → `a.runContext?.projectId ?? getAutomationLegacyRepoId(a)`(`:13-15`).
  → runContext 우선, 없으면 레거시 projectId 폴백.

### 8.4 관련 타입 (`automations-types.ts`)

- **`AutomationSchedulePreset = 'hourly' | 'daily' | 'weekdays' | 'weekly' | 'custom'`**(`:32`).
  (build 함수는 `Exclude<..,'custom'>`만 받음, `:523`, `:544`.)
- **`AutomationRunStatus`**(`:8-17`): pending/dispatching/dispatched/completed/skipped_precheck/skipped_missed/
  skipped_unavailable/skipped_needs_interactive_auth/dispatch_failed.
- **`isFinalAutomationRunStatus`**(`:21-30`): completed, dispatch_failed, 그리고 4개 skipped_* → **true**.
  pending/dispatching/dispatched → false(= in-flight, evict 불가).
- `Automation.timezone: string`(`:116`), `rrule: string`(`:117`), `dtstart: number`(`:118`),
  `missedRunPolicy`/`missedRunGraceMinutes`(`:122-123`). `AutomationRun.runNumber?: number`(`:157`).

### 8.5 `clipboard-text.ts` — 바이트 길이 가드 (cron 파서 의존)

- **`isClipboardTextByteLengthOverLimit(text, maxBytes)`**(`:77-82`):
  `text.length > maxBytes || measureClipboardTextByteLength(text, {stopAfterBytes:maxBytes}).exceededLimit`.
  - **1차 컷: `text.length`(UTF-16 코드유닛 수) > maxBytes**면 즉시 true — 실제 바이트 세기 전에 상한. UTF-8은 코드유닛당
    ≥1바이트라 안전한 상계.
  - **2차: `measureClipboardTextByteLength`**(`:20-37`)가 `stopAfterBytes` 초과 시 조기 반환(`exceededLimit:true`).
- `measureClipboardTextByteLength`(`:20-37`): 코드포인트 순회, `getUtf8ByteLengthForCodePoint`(`:174-185`)로
  UTF-8 바이트 누적(≤0x7f→1, ≤0x7ff→2, ≤0xffff→3, else 4). 서로게이트 페어는 index +1(`:32-34`). **정규식 없음.**
- cron이 쓰는 상한: `AUTOMATION_CRON_EXPRESSION_MAX_BYTES = 2*1024 = 2048`(automation-schedules.ts `:12`).
  (clipboard 자체 상한 `CLIPBOARD_TEXT_*_MAX_BYTES = 16*1024*1024`는 `:1-2`, cron과 무관.)

---

## 9. Rust 생태계 노트 (사실만 — 결정 X)

- **cron 크레이트:** `cron`, `saffron`, `croner`, `cron_clock` 등이 존재한다(일반 지식). 대부분 Vixie DOM/DOW OR
  규칙을 구현하나, **Orca 고유의 "집합-크기 restricted" 휴리스틱**(`size!==31`/`size!==7`, `*/1`·`0-7`·`1-7`을
  unrestricted로 접음)과 **정확히 일치하는지 소스 확인 없이 단정 불가**. 또 Orca는 5-필드(초 없음)에 name 테이블이
  minute/hour/day-of-month엔 없다 — 크레이트마다 필드 수·이름 허용이 다르다.
- **RRULE 크레이트:** `rrule` 크레이트가 존재한다(일반 지식). 그러나 Orca가 쓰는 건 `HOURLY`/`DAILY`/`WEEKLY` +
  `BYHOUR`/`BYMINUTE`/`BYDAY`의 **극소 부분집합**이고, hourly는 BYHOUR를 무시하는 등 **Orca 특유 동작**이 있다.
  풀 RRULE 크레이트를 쓰면 오라클과 미묘히 어긋날 위험(특히 dtstart 경계·off-the-minute·시각 무시 동작).
- **크레이트 시맨틱을 fetch 없이 검증할 수 없다.** 위 호환성 주장은 소스 대조로만 확정 가능. **권고 아님 — 리스크 기록:**
  오라클(§7)의 미묘한 크럭스(DOM/DOW OR + 크기 휴리스틱, 7→0, dtstart-1, `<` vs `<=`, hourly byHour 무시,
  3294일 스캔)를 보존하려면 **hand-roll이 안전 기본값**으로 보인다. 최종 판단은 플래닝 단계.

---

## 10. Codex 교차검증용 개방 질문

1. **DOM/DOW restricted 휴리스틱 충실도.** `daysOfMonth.size !== 31` / `daysOfWeek.size !== 7`(`:208-209`)가
   `*/1`, `0-7`, `1-7`, `1-31`, `,`-나열을 어떻게 접는지 — Rust 크레이트(있다면)와 대조. §7 케이스 16·17이 회귀 핀.
2. **TZ 핀 전략.** 모든 산술이 로컬 wall-clock(§5). 테스트를 어떤 고정 TZ로 돌릴지, `chrono::Local` vs 명시 TZ.
   특히 §7 케이스 3(주 요일)·10·17·21이 TZ/DST에 민감한지 실측 필요. DST 전이에서 `atLocalTime`의 `setHours`가
   존재하지 않는 시각(spring-forward gap)을 만들 때 JS `Date` 동작과 chrono 동작이 일치하는지.
3. **DST 미세드리프트.** `cronHasPossibleOccurrence`(`:517`)·`scanDayCandidates`(`:479`)가 `day += DAY_MS`로
   재-플로어 없이 진행 — DST 경계에서 자정 이탈이 판정을 바꿀 코너가 있는지(현재 오라클엔 노출 안 됨, 잠재 핀).
4. **RRULE 라이브러리 표면.** hand-roll 확정 가정이 맞는지. hourly가 BYHOUR를 무시(§3.3·§4)하는 등
   Orca 특유 동작을 `rrule` 크레이트로 재현하려다 어긋나는 지점 목록화.
5. **`text.length`(UTF-16) 대 UTF-8 바이트.** `isClipboardTextByteLengthOverLimit` 1차 컷(`clipboard-text.ts:79`)이
   UTF-16 코드유닛 수 기준 — Rust `str`(UTF-8 바이트)로 옮길 때 `.chars().count()`(코드포인트)와 다르다.
   서로게이트/BMP-외 문자에서 경계값이 어긋나지 않는지(오라클 케이스 14는 ASCII라 미노출).
6. **`parseSchedule`의 `=` 디스패치**(`:256`). `=` 포함 → RRULE 취급이 cron `a=b` 같은 이상 입력을 어떻게
   처리하는지(현재는 무조건 RRULE 파서로 → throw → invalid). 포팅이 이 코너를 동일 처리하는지.
7. **`nextAutomationOccurrenceAfter` cron의 이중 보정**(`:571-580`). `max(dtstart,after)` floor 후 `after` 전진,
   그 다음 `dtstart` 재보정 — 두 조건이 상호작용하는 코너(dtstart>after인데 dtstart가 오프-더-미닛)를 오라클이
   충분히 덮는지, 추가 핀 테스트가 필요한지.
