# Plan — automation-schedules (cron + RRULE 스케줄링 엔진) 확정

조사: `docs/superpowers/research/2026-07-24-automation-schedules.md` (Orca @ v1.4.150-rc.0,
인용 file:line 고정). Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS** 반영(11개 load-bearing
체크 전부 CONFIRMED, 정밀 수정 8건). 이 문서가 구현 계약이다. 인용은 별도 명시 없으면
`src/shared/automation-schedules.ts`.

## 0. 결정 (조사 + Codex 확정)

Orca Automations = 에이전트를 recurrence(hourly/daily/weekdays/weekly preset, raw 5-필드 cron,
또는 RRULE 문자열)로 스케줄. 이 모듈은 **파싱·검증·분류·발생시각 계산**의 순수 엔진이다 —
IPC/fs/네트워크/Electron 0, 서드파티 cron/rrule 의존 **0**(hand-rolled, Codex 체크1 CONFIRMED).
자율-검증 최적 타겟(James "약한 자율 백엔드 계속"): 순수 로직 + 21-케이스 `.test.ts` 오라클.

**크레이트: 새 leaf `suaegi-automation`** (suaegi-fuzzy/suaegi-keys 선례 — 타 suaegi 크레이트
의존 0). 외부 의존은 **`chrono` + `chrono-tz`만**(로컬 wall-clock 산술을 명시 TZ로 결정론적으로
재현하기 위함, §F2). serde는 M4 retention 타입에 필요 시 최소 도입(스케줄 코어는 순수).

## 1. Codex 반영 픽스 (구현자 필독)

- **F1 — cron·RRULE 전부 hand-roll(크레이트 금지).** Codex 체크3 CONFIRMED: "restricted"는
  **파싱 결과 집합의 크기**로만 결정(`daysOfMonth.size !== 31`, `daysOfWeek.size !== 7`, `:208-209`) —
  `*/1`·`1-31`·전체 콤마열거→DOM unrestricted, `0-7`·`1-7`·전체 콤마열거→DOW unrestricted(7→0 정규화 후).
  Rust `cron`/`cronexpr`/`croner`/`saffron` 어느 것도 이 "구문 무관·집합 카디널리티" 계약을 보장 안 함
  (게다가 대부분 7-필드·확장 문법 허용). **`rrule` 크레이트도 금지**(풀 RFC 표면, hourly-ignores-byHour
  같은 Orca 특유 동작 없음). **최소 수용 매트릭스(M1 테스트로 고정):** `0 9 */1 * MON`=월요일만,
  `0 9 1-31 * MON`=월요일만, DOM 전체 콤마열거=unrestricted, `0-7`/`1-7`/DOW 전체 콤마열거=unrestricted,
  `0 9 1 * MON`=OR, 5-필드만·Orca 정확한 이름·확장 문법 거부, `7`→`0` 정규화가 카디널리티 판정 **전**에.
- **F2 — TZ는 명시 IANA(ambient `chrono::Local` 금지).** 모든 산술이 로컬 wall-clock
  (`getHours/getDate/getDay/setHours`, UTC 계열 0건, Codex 체크6-8). 스케줄 레이어에 **명시 timezone을
  주입**하고 테스트를 거기에 핀. **오라클 테스트=`Etc/UTC` 핀**(호스트 변동 제거, 현재 기대 civil date 전부 보존),
  **별도 DST 스위트=`America/Los_Angeles`**(spring-gap/fall-fold). `TZ=` 환경변수로 전역 Local을 감싸는 방식은
  약함(병렬 레이스·플랫폼 TZDB 편차·숨은 프로세스 상태) → API가 timezone을 인자로 받는다.
- **F3 — 고정 `DAY_MS` 스테핑 verbatim(bug-compatible).** `cronHasPossibleOccurrence`(`:517`)·
  `scanDayCandidates`(`:479`)가 `day += DAY_MS`(86,400,000ms 고정, 재-`startOfLocalDay` 없음). "깨끗한"
  캘린더-일 스테핑으로 바꾸면 Orca와 갈라진다(Codex Q3: 실제 호환 위험 — non-1h 전이·skipped civil date에서
  로컬 날짜가 바뀌거나 건너뛸 수 있음). **verbatim 이식**(divergence 승인 없음). DST 스위트는 이 동작을 **문서화**
  (수정 아님). 이식 계약의 기본 = 버그까지 재현.
- **F4 — 바이트 가드는 Rust `s.len()`(UTF-8).** `isClipboardTextByteLengthOverLimit`의 JS 1차 컷은 UTF-16
  `.length`지만(Codex Q5), valid Rust `str`에선 `s.len()`이 이미 UTF-8 바이트 길이 → **`s.len() > 2048` reject,
  `== 2048` accept**. JS의 `.chars().count()` 이식 **금지**(멀티바이트 언더카운트). 경계 결과는 valid Unicode에서
  동일(unpaired surrogate만 표현 불가 — Rust `str`엔 존재 불가, 무관). 별도 헬퍼 이식 불필요, 단순 `s.len()` 체크.
- **F5 — `=` 디스패치 literal.** `parseSchedule`(`:254-260`): `trimmed.includes('=')`면 RRULE, 아니면 cron.
  `=` 있는 cron-유사 입력도 무조건 RRULE 경로(→invalid). **pin 추가:** `0 9 * * MON=1`→RRULE→invalid,
  `FREQ=DAILY`→valid, `FREQ =DAILY`/다중-`=` malformed, cron-only 검증기는 `parse_cron_expression` 직접 호출.
- **F6 — retention tie-break은 total 아님(Rust stable sort 의존).** 비교자 `b.createdAt-a.createdAt ||
  b.scheduledFor-a.scheduledFor`(`automation-run-retention.ts:18`)는 createdAt·scheduledFor 동률 시 0 반환,
  최종 `id` 비교 없음. JS 안정정렬이 입력순 보존으로 결정론 확보 → Rust는 **`sort_by`(안정)** 사용하고 최종
  `runs.filter`로 append 순서 보존(`:26`). id 추가 tie-break **넣지 말 것**(Orca와 갈라짐).
- **F7 — cron off-the-minute 경계는 오라클 갭 → 추가 pin 필수(M3).** 오라클은 hourly 경로만 테스트
  (`automation-schedules.test.ts:41-48`), cron 이중보정(`:571-580`)엔 focused 테스트 **없음**(Codex Q7).
  **M3에 직접 cron-boundary pin 추가:** (a) `dtstart>after` & dtstart off-minute→다음 분으로 ceil,
  (b) `dtstart==after` 정확히 매칭 분→strict `after`가 skip, (c) `dtstart<after` & after off-minute→floor 후
  `<=after` 전진, (d) `after==dtstart-1` & dtstart 정확 매칭→dtstart eligible, (e) 첫 ceil 분 불일치→스캔 지속.
  이건 **오라클 이식이 아니라 우리가 추가하는 회귀 핀**(mutation 검증 대상).
- **F8 — 정밀 표기(무영향).** 포맷팅은 `new Date()`(`:326`)로 시계 읽음(literal `Date.now()` 아님) —
  이식엔 무관(둘 다 wall-clock). retention 음수-cap `[]`는 **입력 전부 final일 때만**(in-flight는 `:26`으로 생존).

## 2. 마일스톤

### M1 — cron 파스 + 검증 (`cron.rs`, 전부 unit/mutation)
`parse_cron_number`(`:101-109`, 이름맵→`Number`, 정수 아니면 err), `parse_cron_field`(`:111-179`,
리스트→스텝→범위/와일드카드, 정규화 전후 경계검증 `start>end` err, 빈결과 err), `parse_cron_expression`
(`:181-211`, 5-필드 강제[maxFields=6으로 초과감지], 필드별 min/max/names/normalize 표 §2.3, **restricted=
size 휴리스틱** F1), `get_automation_cron_expression_fields`(`:213-236`, **바이트가드 F4 먼저**, regex-free
선형 토크나이즈, 유니코드 공백 `:238-252`), `cron_date_matches`(`:498-509`, **DOM/DOW OR-vs-AND** 크럭스),
`cron_matches`(`:490-496`), `cron_has_possible_occurrence`(`:511-520`, 3294일 스캔 F3), `is_valid_automation_cron_schedule`.
*오라클:* 케이스 13·14(토크나이즈 regex-free+DoS), 18(malformed separators), 19(no-possible-run `0 0 31 2 *`),
16·17(DOM/DOW OR + size 휴리스틱). *mutation:* OR↔AND 뒤집기, `size!==31/7` 경계, 7→0 정규화 제거,
바이트가드 우회, `start>end` 검증 제거, 3294 축소(→leap 케이스는 M3이나 valid 판정은 여기).

### M2 — RRULE 파스/빌드 + 라운드트립 (`rrule.rs`)
`parse_rrule`(`:70-99`, FREQ∈{HOURLY,DAILY,WEEKLY} else err, BYHOUR 기본9/BYMINUTE 기본0, WEEKLY byDay
필수+DAY_CODES 검증), `parse_automation_rrule`(`:283-313`, preset 역매핑, **일요일0 보존**), `try_parse_*`
(null-safe), `build_automation_rrule`(`:522-541`)/`build_automation_cron_schedule`(`:543-562`)(clamp,
**hourly는 BYHOUR 미포함** §3.3), `parse_schedule`(`:254-260`, **`=` 디스패치** F5). *오라클:* 4·5(dow0 보존
라운드트립), 6·7(malformed BYDAY 거부, WEEKLY byDay 필수), 11(buildCron), 20(`=` cron-only 거부). *mutation:*
FREQ 화이트리스트, byDay 검증 제거, hourly BYHOUR 누출, dow0→1 강제, `=` 디스패치 반전. F5 추가 pin.

### M3 — 발생 계산 (`occurrence.rs`, **the risky core**) — TZ 명시
`next_automation_occurrence_after`(`:564-604`, cron 이중보정 `<=after`/`<dtstart` + hourly `<=`/`<` +
weekly/daily `scanDayCandidates(max(dtstart-1,after),1)`), `latest_automation_occurrence_at_or_before`
(`:606-636`, `now<dtstart→None`, cron 역스캔, hourly byHour무시, weekly/daily 역방향), `scan_day_candidates`
(`:467-482`, forward `>` strict/backward `<=` incl, 370 상한), `day_matches`(`:459-465`), `at_local_time`
(`:447-451`)/`start_of_local_day`(`:453-457`)/`floor_to_minute`(`:484-488`). **전부 명시 timezone 인자(F2).**
*오라클:* 1(latest hourly byHour무시), 2(off-the-minute hourly→11:00), 3(weekdays 주말배제), 10(cron 양방향),
17(`*/1` unrestricted→월요일만), 21(**leap-day** `0 0 29 2 *`→2028-02-29). **추가 핀(F7):** cron off-the-minute
5-케이스. **DST 스위트(F3, `America/Los_Angeles`):** spring-forward/fall-back 정/역 스캔, 고정 DAY_MS 드리프트
문서화. *crux/mutation:* `<` vs `<=` 경계 각각, `dtstart-1` 제거, forward/backward 방향 부등호, byHour 누출,
CRON_SCAN_MINUTES 축소(→leap 놓침), 고정 DAY_MS→캘린더일 변경(DST 스위트가 감지).

### M4 — 분류 + 레이블 + retention (`classify.rs` + `retention.rs`)
`classify_automation_cron_schedule`(`:424-432`)/`classify_parsed_cron_schedule`(`:377-422`): **결정론 코어
(kind/hour/minute/dayOfWeek)와 로케일 레이블 분리**(§6). kind = hourly/daily/weekdays/weekly/custom/invalid
(`get_single_set_value`/`set_contains_exactly`/`set_contains_range`, weekdays={1..5}, weekly=DOW단일, 발생불가→
invalid 선두). `format_automation_schedule`(`:434-445`): 레이블은 **하드코딩 영어**("Hourly at :MM"/"Daily"/
"Weekdays"/"Sundays" 등, en 오라클 미러 — Intl 미이식, 요일명 테이블 하드코딩). **retention(`retention.rs`):**
`prune_automation_runs`(`automation-run-retention.ts:7-27`, cap=100, **final만 evict**[in-flight 절대 생존],
groupBy automationId, **안정정렬** F6, 음수clamp, 최종 filter로 append순 보존), `backfill_automation_run_numbers`
(`:38-54`, 최고번호+1 재발급 방지), `next_automation_run_number`(`:57-65`), `automation-run-identity`(`:1-16`).
*오라클:* 8(YEARLY→invalid), 9(hourly 레이블), 12(레이블 Sundays 7→0), 15(classify weekdays/weekly),
16(custom) + `automation-run-retention.test.ts` 전량(cap·in-flight생존·tie-break·backfill·nextNumber).
*mutation:* kind 매핑 오류, weekdays 집합, 레이블 7→0, isFinal 목록에서 skipped_* 누락(→in-flight 오evict),
음수 slice 함정, 안정정렬 파괴, 최고번호 재발급.

## 3. Deferred (명시)
- **automations 런타임**(스케줄러 서비스·missed-run grace 정책 실행·에이전트 dispatch) = 별건, 사람눈/IPC.
  이 플랜은 **순수 스케줄링 엔진만**. `Automation.timezone` 필드(`automations-types.ts:116`)는 이 모듈 미사용 —
  런타임이 주입할 때 배선.
- **UI**(automations 리스트·편집기·preset 피커) = 사람눈.
- **DST 정확 호환 정책**(존재하지 않는/모호한 로컬 시각의 JS `Date.setHours` 동작 재현) — DST 스위트가
  현 동작을 문서화하되, 완전 JS-일치는 런타임 통합 시 결정(F3, non-blocking).

## 4. 순서 (확정)
M1 cron 파스/검증 → M2 RRULE 파스/빌드 → M3 발생계산(TZ 명시, F7 추가핀 + DST 스위트) → M4 분류/레이블/retention.
불변식: hand-roll(크레이트 금지, F1), TZ 명시 인자(F2), 고정 DAY_MS verbatim(F3), UTF-8 s.len() 가드(F4),
`=` 디스패치 literal(F5), 안정정렬(F6), transient≠false-negative, 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]]
