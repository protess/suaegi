# Research: 터미널 query/reply/OSC-color 이스케이프 파싱 클러스터 — Rust 포팅 계약서

**대상**: Orca `src/shared/` 4개 모듈 @ v1.4.150-rc.0 — `suaegi-term` 크레이트에 신규 모듈로 추가
- `terminal-osc-color-reply.ts` (240L) + `.test.ts` (51L) — **FOUNDATION**. 나머지가 의존.
- `terminal-query-reply.ts` (69L) + `.test.ts` (99L) — self-contained 응답 분류기.
- `terminal-reply-query-extraction.ts` (160L, **자체 테스트 없음**) — osc-color-reply 의존.
- `terminal-reply-query-scan.ts` (125L) + `.test.ts` (41L) — extraction + osc-color-reply 의존. 이 테스트가 extraction 의 **간접 오라클**.

**성격**: 전부 **PURE**. fs/IPC/`Date`/`performance` 참조 0건 (grep 확인). 유일한 런타임 외부 의존은 `terminal-query-reply.test.ts:1` 의 `@xterm/headless` — 이건 **테스트 전용**, 모듈 본체는 순수. `.test.ts` 3개가 오라클, **verbatim 포팅** 대상.

---

## 결론 요약 (먼저 읽을 것)

1. **가장 큰 위험 = 바이트 vs UTF-16 code unit, 그리고 non-char-boundary 슬라이스 PANIC.** 이 클러스터는 전부 **JS `string`(UTF-16 code unit) 위에서** 동작한다. `data[offset]`, `charCodeAt(index)`, `.slice(a,b)`, `.indexOf`, `.length`, `.startsWith(s, offset)` 는 모두 **code unit 인덱스**다. Rust 포트가 `&str` 위에서 **바이트 오프셋으로** 슬라이스하면 멀티바이트 문자 경계에서 **panic** 한다. 대상 데이터는 PTY 바이트 스트림이므로 **`&[u8]` 위에서 동작하는 것이 자연스럽고 안전**하다(§오픈질문 1).
2. **이스케이프 문자 상수는 전부 ASCII 단일/2바이트**: ESC=`0x1b`, BEL=`0x07`, OSC=`ESC ]`=`[0x1b,0x5d]`, ST=`ESC \`=`[0x1b,0x5c]`, CSI=`ESC [`=`[0x1b,0x5b]`, DCS=`ESC P`=`[0x1b,0x50]`. 소스는 상수를 ``/``(osc-color-reply:8-10) 와 `\x1b`/`\x07`(본체 리터럴) 두 표기로 섞어 쓰지만 **바이트값은 동일**하다.
3. **숫자 파싱에 `parseInt` 없음.** color 채널만 `Number()`(osc-color-reply:91,96) 를 쓰고, 그마저 정규식 `^\d+(\.\d+)?%?$`(:89,93) 로 게이트한다 — **radix/16진수/부호/공백 유입 불가**. CSI 파라미터는 **정수로 파싱하지 않고** 정규식·문자열 동등비교로만 분류한다(파라미터 값 자체는 무의미).
4. **OSC 종료자는 BEL(`0x07`) 또는 ST(`ESC \`) 양쪽**을 받는다(osc-color-reply:141-146). 스트림이 `ESC` 와 `\` 사이에서 쪼개지면 **partial** 로 보류한다(:147-149). 이 partial 규약이 클러스터 전체의 chunk-경계 처리 핵심이다.
5. **`terminal-reply-query-extraction.ts` 는 `findCsiFinalByteIndex` 를 빼면 사실상 전부 미테스트**다. scan 테스트가 커버하는 건 오직 `findCsiFinalByteIndex`(간접). `extractHiddenStartupRendererQueryData`/`containsCsiRendererQuery`/`containsStatefulRendererQuery`/`isStateless…`/`isStateful…` 는 **직접·간접 어느 테스트도 실행하지 않는다**. → 포트는 이 모듈 동작을 **새 핀 테스트로 고정**해야 한다(§4, §오픈질문 4).
6. **seq 산술은 code unit 단위**다(scan:59,62). `pendingStartSeq + pending.length === chunkStartSeq`(:59) 로 chunk 연속성을 판정하는데, `pending.length` 는 UTF-16 length. suaegi 의 PTY 출력 seq 카운터가 **바이트** 단위라면 non-ASCII 에서 어긋난다(§오픈질문 5).
7. **suaegi-term 과의 중복 거의 없음.** 기존 `grid.rs` 는 `alacritty_terminal` vte 파서가 `Event::PtyWrite`(grid.rs:135) 로 CSI 질의(DA/DSR/CPR 등) 응답을 **이미 생성**한다. 이 4개 모듈은 **에뮬레이터 계층이 아니라 트랜스포트 계층**(원격/데몬 경로에서 raw 바이트를 스캔·분류) 이다. `encode.rs` 는 **아웃바운드**(키→바이트)만 다룬다. 통합 지점은 `session.rs` 의 `reply_tx` 즉시-송신 큐(§오픈질문 6).

---

## 0. 이스케이프-시퀀스 문법 요약 (포팅 계약의 뼈대)

이 클러스터가 인식하는 시퀀스 문법 전체:

| 종류 | 프레이밍 | 이 클러스터에서의 형태 | 종료자 |
|---|---|---|---|
| **OSC 색상 질의** | `ESC ]` … | `ESC ] 10 ; ?` / `ESC ] 10 ; ? ; ?` / `ESC ] 11 ; ?` | BEL(`0x07`) 또는 ST(`ESC \`) |
| **OSC 색상 응답**(빌드) | `ESC ]` … | `ESC ] {slot} ; rgb:RRRR/GGGG/BBBB ESC \` | ST(`ESC \`) 고정 |
| **CSI 질의/응답** | `ESC [` … | 파라미터 `[?>=]?[0-9;]*` + final byte `0x40..0x7e` | final byte 자체 |
| **DCS 응답/질의** | `ESC P` … | `ESC P … ESC \` (body `$q`/`+q`/`$r`/`>|`) | ST(`ESC \`) |

- **CSI final byte 범위 = `0x40..=0x7e`**(`@`~`~`) — extraction:139 의 `code >= 0x40 && code <= 0x7e`. 이게 CSI 시퀀스 끝 판정의 유일한 규칙.
- OSC 색상 질의의 `?` 는 "값을 물어본다"는 리터럴 물음표(`0x3f`). 응답(빌드)은 `rgb:` 접두 + 16진 word 3채널.
- **partial 규약**: 데이터가 시퀀스 중간에서 끊기면 `{kind:'partial'}`(osc) / `endIndex===-1`(scan) / `pending` 반환(extraction/scan) 으로 **보류**하고 다음 chunk 를 기다린다.

---

## 1. `terminal-osc-color-reply.ts` — FOUNDATION (OSC 10/11 색상 질의 파스/빌드)

### 1.1 상수 / 타입 (:1-34)

- **`TerminalOscColorQueryReplyColors`**(:1-4) — `{ foreground?: string; background?: string }` (CSS 색 문자열). export.
- **`TerminalOscColorQuerySlot = 10 | 11`**(:6) — export. Rust: `enum Slot { Fg=10, Bg=11 }` 또는 `u16` 상수.
- **`OSC = ']'`**(:8) = `ESC ]` = `[0x1b,0x5d]`. **2 code unit**.
- **`BEL = ''`**(:9) = `[0x07]`.
- **`STRING_TERMINATOR = '\\'`**(:10) = `ESC \` = `[0x1b,0x5c]`. **2 code unit**. (`\\` 은 소스상 이스케이프된 백슬래시 1개.)
- **`TERMINAL_OSC_COLOR_QUERY_PREFIXES`**(:11-14) — `[{slot:10, prefix:"ESC]10;"}, {slot:11, prefix:"ESC]11;"}]`. 각 prefix 는 `OSC + "NN;"` = 5 code unit.
- **`TERMINAL_OSC_COLOR_QUERY_BODIES`**(:15-24) — slot→허용 body 표. **slot 10**: `{body:'?', slots:[10]}`, `{body:'?;?', slots:[10,11]}`. **slot 11**: `{body:'?', slots:[11]}` 뿐. → **`?;?` 결합 질의는 slot 10 에서만** 유효(fg 질의가 fg+bg 둘 다 요청), slot 11 은 단일 `?` 만.
- **`TerminalOscColorQueryParseResult`**(:26-29) — `{kind:'match', slots, endIndex}` | `{kind:'partial'}` | `{kind:'none'}`. export.
- **`TerminalOscTerminatorParseResult`**(:31-34) — `{kind:'complete', endIndex}` | `{kind:'partial'}` | `{kind:'none'}`. **비-export**.

### 1.2 `cssColorToOscRgb(value?) : string|null` (:36-64) — export, pure

CSS 색 → OSC `rgb:` 문자열 변환. **빌드 경로 전용**(파싱 아님).
- :37-39 `!value` → `null` (undefined/빈문자열).
- :40 `value.trim()` — **JS `trim()`**: `\t\n\v\f\r 공백` + 유니코드 공백/BOM 제거. **Rust `str::trim()` 은 유니코드 `White_Space` 기준** — 근사하나 완전 동일 아님(오픈질문 3).
- :41 정규식 `/^#([0-9a-f]{3}|[0-9a-f]{6})$/i` — hex 3 또는 6자리, **case-insensitive**. `?.[1]` 로 캡처.
- :43-49 3자리면 각 문자 중복(`c`→`cc`) 확장.
- :50-52 `byteHexToWord(expanded.slice(0,2))` 등 — `slice(0,2)/(2,4)/(4,6)`. **ASCII hex 문자열이라 code unit=byte, 경계 안전**. 결과 `rgb:{word}/{word}/{word}`.
- :54 `/^rgba?\(\s*([^)]+)\)$/i` — `rgb(...)`/`rgba(...)`.
- :58-63 `parseCssRgbChannels` → 각 채널 `.toString(16).padStart(2,'0').repeat(2)` (8bit→16bit word 복제, `c0`→`c0c0`).

**`byteHexToWord(byte)`**(:66-68) — `byte.repeat(2)` (2자리 hex→4자리 word).

**`parseCssRgbChannels(body) : [n,n,n]|null`**(:70-86) — 비-export.
- :71 `body.split('/')[0]?.trim()` — **`/` 앞부분만**(alpha 슬래시 표기 버림). `split('/')[0]` 이 `undefined` 가능(빈 문자열이면 `['']`→`''`, truthy 실패 → null:72-74).
- :75-77 `,` 포함이면 `split(',')`, 아니면 `split(/\s+/)`; **각각 `.slice(0,3)`**.
- :78-80 정확히 3개 아니면 null.
- :81-85 각 채널 `parseCssRgbChannel`, 하나라도 null 이면 null.

**`parseCssRgbChannel(component) : number|null`**(:88-97) — 비-export.
- :89-91 `/^(\d+(?:\.\d+)?)%$/` 매치면 `clampByte(Number(percent)/100*255)`.
- :93 아니면 `/^\d+(?:\.\d+)?$/` **미매치 시 null** — **부호·hex·지수 표기 전부 거부**. `Number()` 로 넘어가는 문자열은 순수 십진수뿐이라 **radix 함정 없음**.
- :96 `clampByte(Number(component))`.

**`clampByte(value)`**(:99-101) — `Math.min(255, Math.max(0, Math.round(value)))`. **`Math.round` = 반올림 half-up(+∞ 방향)**. 값이 항상 ≥0 이라 Rust `f64::round`(half away from zero) 와 **양수 구간에서 동일** — 발산 없음.

### 1.3 응답 빌드 (:103-125)

- **`terminalOscColorQueryReply(colors, slot) : string|null`**(:103-113) — export. slot 10 이면 `cssColorToOscRgb(colors.foreground)`, 11 이면 `background`. null 이면 null. 성공 시 **`` `\x1b]${slot};${color}\x1b\\` ``**(:112) = `ESC ] {10|11} ; rgb:… ESC \`. **종료자 ST 고정**(BEL 아님).
- **`isTerminalOscColorQueryReply(reply)`**(:115-117) — 비-export 타입가드 `reply !== null`.
- **`terminalOscColorQueryReplies(colors, slots) : string[]|null`**(:119-125) — export. slots 각각 응답 빌드, **하나라도 null 이면 전체 null**(`every`).

### 1.4 body/종료자 파싱 (:127-208) — 파싱 핵심

- **`terminalOscColorQuerySlotsForBody(slot, body) : slots|null`**(:127-132) — export. body 표(:15-24)에서 정확 문자열 `entry.body === body` 매치, `?.slots ?? null`.

- **`parseTerminalOscTerminator(data, offset)`**(:134-152) — 비-export. **바이트 vs char 핵심**:
  - :138 `offset >= data.length` → **partial**.
  - :141 `data[offset] === BEL` → **complete**, `endIndex = offset + 1` (`BEL.length`).
  - :144 `data.startsWith(STRING_TERMINATOR, offset)` → **complete**, `endIndex = offset + 2` (`ST.length`).
  - :147 `data[offset] === '\x1b' && offset+1 >= data.length` → **partial** (ST 가 ESC/`\` 사이에서 쪼개짐; 주석 :148).
  - :151 else → **none**. (예: `ESC X` 처럼 ESC 뒤가 `\` 아님.)

- **`completeTerminalOscColorQuery(slot, body, terminator)`**(:154-164) — 비-export. terminator 가 complete 아니면 그대로(partial/none) 반환; complete 면 body→slots 조회, 있으면 `{kind:'match', slots, endIndex}` 없으면 none.

- **`parseTerminalOscColorQueryBody(data, bodyStart, slot)`**(:166-191) — 비-export. prefix(`ESC]NN;`) 뒤 body 파싱:
  - :171 `bodyStart >= data.length` → partial.
  - :174 `data[bodyStart] !== '?'` → none.
  - :177 `parseTerminalOscTerminator(data, bodyStart+1)` — 단일 `?` 뒤 종료자.
  - :178 종료자 결과가 `none` 이 **아니면**(complete/partial) `completeTerminalOscColorQuery(slot,'?', …)` 로 확정/보류.
  - :181 종료자가 none 이고 `slot !== 10 || data[bodyStart+1] !== ';'` → none. **결합 질의는 slot 10 + `;` 일 때만 진행**.
  - :184 `bodyStart+2 >= data.length` → partial.
  - :187 `data[bodyStart+2] !== '?'` → none.
  - :190 `completeTerminalOscColorQuery(slot, '?;?', parseTerminalOscTerminator(data, bodyStart+3))` — `?;?` 뒤 종료자.
  - **핵심**: `?;?;?`(3중) 같은 건 :190 에서 종료자 파스가 `;` 를 만나 none → 전체 none. **고정폭 문법**이라 unbounded 스캔 없음(테스트 5 참조).

- **`parseTerminalOscColorQuery(data, offset)`**(:193-208) — **export, 최상위 진입점**.
  - :197-199 prefix 표에서 `data.startsWith(prefix, offset)` 매치 찾기.
  - :200-205 매치 없으면: `fragment = data.slice(offset)`; **어떤 prefix 라도 `prefix.startsWith(fragment)`** 이면 partial(아직 prefix 도 다 안 옴), 아니면 none. → `ESC ]1` 같은 부분 prefix 를 partial 로 보류.
  - :206-207 `bodyStart = offset + entry.prefix.length`; `parseTerminalOscColorQueryBody(…)`.

### 1.5 `sendTerminalOscColorQueryReplies(data, colors, sendInput) : boolean` (:210-240) — export

data 안의 모든 OSC 색상 질의를 찾아 응답 송신. **부수효과는 주입된 `sendInput` 콜백**(순수성 유지).
- :216-217 `offset=0`, `while offset < data.length`.
- :218 `data.indexOf(OSC, offset)` — 다음 `ESC ]` 위치. `-1` 이면 break.
- :222 `parseTerminalOscColorQuery(data, oscIndex)`.
- :223-232 `match` → `terminalOscColorQueryReplies` 빌드, 각 응답 `sendInput`, `sent=true`, `offset = query.endIndex` 로 점프.
- :234-236 `partial` → break (다음 chunk 대기).
- :237 `none` → `offset = oscIndex + OSC.length`(+2) 로 한 칸 넘기고 계속.
- 반환: 하나라도 보냈으면 true.

### 1.6 trap-class 목록 (osc-color-reply)

| site | 구문 | Rust 발산 위험 / 결정 |
|---|---|---|
| :50-52 | `expanded.slice(0,2)` 등 | ASCII hex only → byte=char, **안전**. |
| :138,171,184 | `.length` 비교 | code unit 수. `&[u8]` 면 byte 수 — 이 문법은 전부 ASCII 이므로 등가. |
| :141,147,174,187 | `data[offset]` / `data[bodyStart+k]` | **code unit 인덱싱**. `&[u8]` 에선 byte 인덱싱으로 직역 가능(전부 ASCII 비교값). `&str` 인덱싱은 컴파일 불가 → `&[u8]` 채택 근거. |
| :144 | `data.startsWith(ST, offset)` | Rust: `data[offset..].starts_with(b"\x1b\\")` — byte 슬라이스, **offset 이 char 경계 아니어도 `&[u8]` 면 안전**. |
| :197,202 | `startsWith(prefix, offset)` / `prefix.startsWith(fragment)` | 위와 동일. `&[u8]` 권장. |
| :201 | `data.slice(offset)` | `&str` 면 offset non-boundary 시 **panic**. `&[u8]` 이면 안전. |
| :218 | `data.indexOf(OSC, offset)` | Rust: `data[offset..].windows(2).position(...)` 또는 `memchr`/`find`. offset 은 이전 endIndex(항상 유효). |
| :91,96 | `Number(percent)` / `Number(component)` | 정규식 게이트로 십진수만 → `f64::from_str` 또는 정수 파스. **radix 함정 없음**. |
| :40 | `value.trim()` | JS trim vs Rust trim 미세차(오픈질문 3). 빌드 경로라 오라클 영향 적음. |
| 상수 | ``/``/`\x1b\\` | 전부 ASCII 바이트 리터럴. |

---

## 2. `terminal-query-reply.ts` — 응답 분류기 (self-contained)

xterm 의 `onData` 스트림에서 **사용자 타이핑 vs 에뮬레이터가 질의에 자동 생성한 응답**을 구분(:46-55 주석). 응답은 지연에 민감하므로 입력 coalescing 을 우회해 **즉시 송신**해야 함(#7329). **보수적 설계**: 완전·정형 응답 문법만 매치, 화살표/기능키/kitty 키스트로크는 절대 오분류 안 함.

### 2.1 상수 / 정규식 (:13-44)

- **`ESC = String.fromCharCode(0x1b)`**(:13) — `[0x1b]`.
- 7개 정규식, 전부 `new RegExp('...')` + `\u`-이스케이프(소스에 리터럴 제어문자 없음). **모두 `^...$` 앵커** → **완전한 단일 응답 하나**만 매치(다중 시퀀스 불가).
  - **`CPR_OR_DSR_RE`**(:30) `^\[\??[0-9;]*[Rn]$` — CPR/DECXCPR(final `R`) + DSR(final `n`). `?` optional.
  - **`DEVICE_ATTRIBUTES_RE`**(:31) `^\[[?>=]?[0-9;]*c$` — DA1/DA2/DA3(final `c`), private 도입자 `?>=` optional.
  - **`WINDOW_SIZE_REPORT_RE`**(:33) `^\[[468];[0-9]+;[0-9]+t$` — pixel/text-area 크기 보고(`4`/`6`/`8` 시작, final `t`).
  - **`DECRPM_RE`**(:35) `^\[\??[0-9;]*\$y$` — mode 보고, body 끝 `$y`. private(`?`) / ANSI 양쪽.
  - **`KITTY_FLAGS_RE`**(:38) `^\[\?[0-9]+u$` — kitty 키보드 플래그(`ESC[?N u`). **`?` 가 kitty 키스트로크(`ESC[code;mods u`)와 구분**하는 유일 표식.
  - **`OSC_RESPONSE_RE`**(:40) `^\][0-9]+;[^]*(?:|\\)$` — OSC 응답. body 는 **BEL/ESC 를 뺀 임의 문자**(negated class), 종료자 BEL 또는 ST. **여기서 `[^]*` 가 non-ASCII 페이로드도 매치** → 포트가 `&str` regex 면 유니코드, `&[u8]` regex 면 바이트(오픈질문 1).
  - **`DCS_RESPONSE_RE`**(:43) `^P(?:[01]\$r[^]*|>\|[^]*)\\$` — DECRQSS(`ESC P [01]$r … ESC\`) + XTVERSION(`ESC P >| … ESC\`).

### 2.2 `isTerminalQueryReply(data) : boolean` (:56-69) — export, pure

- :57 **가드 `data.length < 3 || data[0] !== ESC`** → false. **`.length` = code unit 수** — non-ASCII 응답에서 바이트 수와 다를 수 있으나, 최소 길이 3 은 전부 ASCII 프레이밍이라 실질 무해. `&[u8]` 이면 `data.len() < 3 || data[0] != 0x1b`.
- :60-68 7개 정규식 OR.

### 2.3 trap-class (query-reply)

- **`parseInt`/`slice`/`indexOf` 전무.** 오직 `.length`(:57), `data[0]`(:57), 정규식 7개.
- **핵심 포팅 결정**: 정규식을 Rust `regex` 크레이트로 옮길지, 아니면 **바이트 상태기계로 직역**할지. `regex::bytes::Regex` 를 쓰면 `^...$` + 바이트 클래스 그대로 이식 가능. `[^]` 는 `regex::bytes` 에서 `[^\x07\x1b]`(**바이트** 부정) — JS 의 code-unit 부정과 non-ASCII 에서 미묘하게 다를 수 있으나(멀티바이트 UTF-8 의 각 바이트가 0x07/0x1b 아니면 통과) 실무상 동등. **정규식 앵커 `^$` 는 Rust 에서 `\A…\z`(또는 `regex` 의 기본 `^$`는 multiline off) 로 전체 문자열 매치 보장** 필요.
- **`@xterm/headless` 의존은 오직 테스트**(:1 import) — 모듈 본체 순수. 포트 오라클에서 그 한 케이스(§7.2 test2)는 xterm 필요 → 리터럴로 대체.

---

## 3. `terminal-reply-query-extraction.ts` — 미테스트 모듈 (동작 전량 고정 필요)

**왜 존재**: xterm 의 hidden-output 복원 큐 / main 의 pending-cap bulk drop 이 **버리려는 바이트에서 질의를 건져내야** 함(:1-6 주석). 삼켜진 질의 = 프로그램이 응답을 영원히 대기. **osc-color-reply 에만 의존**(:7 import `parseTerminalOscColorQuery`).

### 3.1 상수 / 타입

- **`HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS = 64`**(:9) — export. partial 시 보류 window 폭(**code unit**).
- **`ExtractedRendererQueryData`**(:11-16) — `{ statelessQueryData, statefulQueryData, oscColorQueryData, pending }` (모두 string).

### 3.2 `extractHiddenStartupRendererQueryData(data, pending) : ExtractedRendererQueryData` (:18-102) — export, pure

`input = pending + data`(:20) 를 스캔, 질의를 3종 버킷으로 누적, 미완성 꼬리는 `pending` 으로 반환.
- :28 `while offset < input.length`.
- :29 `candidateIndex = input.indexOf('\x1b', offset)`; :30-32 `-1` 이면 break.
- :33-40 `candidateIndex+1 >= input.length`(ESC 가 맨 끝) → **`pending: input.slice(candidateIndex)`**(전체 꼬리 보류, **64 상한 미적용**).
- **CSI 분기**(:41-62) `input.startsWith('\x1b[', candidateIndex)`:
  - :42 `findCsiFinalByteIndex(input, candidateIndex+2)`.
  - :43-53 `-1`(final byte 아직 없음) → **`pending: input.slice(candidateIndex, candidateIndex+64)`** (64 상한 적용). ← **여기가 non-ASCII 시 `&str` 슬라이스 panic 후보**(candidateIndex+64 가 char 경계 아닐 수 있음).
  - :54 `sequence = input.slice(candidateIndex, finalByteIndex+1)`.
  - :55-59 `isStatelessRendererReplyCsiQuery` → stateless 버킷, `isStatefulRendererReplyCsiQuery` → stateful 버킷, **둘 다 아니면 어느 버킷에도 안 넣고 건너뜀**.
  - :60 `offset = finalByteIndex+1`.
- **OSC 분기**(:64-84) `input.startsWith('\x1b]', candidateIndex)`:
  - :65 `parseTerminalOscColorQuery(input, candidateIndex)`.
  - :66-76 `partial` → `pending: input.slice(candidateIndex, candidateIndex+64)` (64 상한).
  - :77-80 `none` → `offset = candidateIndex+2`(한 칸 넘김).
  - :81-83 (match) `oscColorQueryData += input.slice(candidateIndex, query.endIndex)`; `offset = query.endIndex`.
- **잔여 분기**(:86-98) ESC 뒤가 `[`·`]` 도 아님(예: `ESC P` DCS, `ESC O`, `ESC b`):
  - :86 `parseTerminalOscColorQuery(input, candidateIndex).kind === 'partial'` → `pending: input.slice(candidateIndex)`.
  - **분석**: 여기 도달 시 `input[candidateIndex+1]` 은 `]` 가 아님(위 분기 탈락) 이고 `candidateIndex+1 < length`(:33 통과). `parseTerminalOscColorQuery` 의 partial 은 prefix(`ESC]NN;`) 부분매치일 때만 나는데 두번째 문자가 `]` 여야 함 → **이 분기의 partial 은 실제로 발생 불가(dead/defensive)**. 항상 :95-97 `offset = candidateIndex+1` 로 떨어져 ESC 한 칸만 넘긴다.
  - **DCS(`ESC P`)를 extraction 은 처리하지 않는다** — scan 모듈(§4)과의 의도적 차이. 여기선 그냥 ESC 를 스킵.
- :101 루프 정상 종료 → `pending: ''`.

### 3.3 `containsCsiRendererQuery(data) : boolean` (:104-118) / `containsStatefulRendererQuery(data) : boolean` (:120-134) — export, pure

`data.indexOf('\x1b[')` 로 CSI 시작 찾기 → `findCsiFinalByteIndex` → `-1` 이면 **즉시 false**(미완성이면 없다고 봄) → sequence 검사. 전자는 stateless OR stateful, 후자는 stateful 만. 다음 `indexOf('\x1b[', finalByteIndex+1)` 로 순회.

### 3.4 `findCsiFinalByteIndex(data, offset) : number` (:136-144) — export, pure. **scan 모듈이 import**

- :137-142 `for index=offset..data.length`: `code = data.charCodeAt(index)`; **`code >= 0x40 && code <= 0x7e`** 이면 `index` 반환.
- :143 없으면 `-1`.
- **`charCodeAt` = UTF-16 code unit 값**. 멀티바이트 문자면 surrogate/BMP code point. 0x40..0x7e 범위는 ASCII 뿐이라, non-ASCII 파라미터가 껴도 그 code unit 은 >0x7e 라 스킵 → **`&[u8]` 로 바이트 순회해도 동등**(UTF-8 continuation byte 0x80-0xbf, 선두 0xc0+ 전부 범위 밖). **오히려 `&[u8]` 이 더 명확·안전**.

### 3.5 `isStatelessRendererReplyCsiQuery(sequence) : boolean` (:146-156) — export, pure

- :147 `sequence.endsWith('c')` → true (모든 DA 질의).
- :150-155 OR: `=== '\x1b[5n'`(DSR), `=== '\x1b[>q'`(XTVERSION), `=== '\x1b[14t'`, `=== '\x1b[16t'`(pixel size).

### 3.6 `isStatefulRendererReplyCsiQuery(sequence) : boolean` (:158-160) — export, pure

- `=== '\x1b[6n'`(CPR) OR (`startsWith('\x1b[?') && endsWith('$p')`)(DECRQM private mode 질의).

### 3.7 trap-class (extraction) — **최고 위험 모듈**

| site | 구문 | 위험 / 결정 |
|---|---|---|
| :48-51, :71-74 | `input.slice(candidateIndex, candidateIndex+64)` | **`&str` 면 +64 가 char 경계 아닐 때 panic**. non-ASCII OSC/CSI 페이로드에서 발현. `&[u8]` 채택 또는 `str::char_indices` 로 경계 보정 필수. **테스트에 non-ASCII 케이스 없음 → 신규 핀 필요.** |
| :38, :91 | `input.slice(candidateIndex)` | candidateIndex 는 indexOf(ESC) 결과라 ESC(0x1b) 위치 = char 경계. **이건 안전**(단일바이트 시작). |
| :54, :111, :127 | `input.slice(candidateIndex, finalByteIndex+1)` | finalByteIndex 는 charCodeAt 스캔 결과. `&str` 면 경계 이슈 잠재. `&[u8]` 안전. |
| :138 | `data.charCodeAt(index)` | code unit. `&[u8]` 바이트 순회로 직역(§3.4). |
| :29,105,115,121,131 | `indexOf('\x1b')` / `indexOf('\x1b[')` | ESC/`ESC[` 바이트 검색 → `memchr`/`windows(2)`. |
| :41,64,86,147(endsWith),150-159 | `startsWith`/`endsWith`/`===` | 전부 ASCII 프레이밍 바이트 비교. |
| — | `parseInt` | **없음**. |

**미테스트 = 신규 핀 대상**: `extractHiddenStartupRendererQueryData`(전체), `containsCsiRendererQuery`, `containsStatefulRendererQuery`, `isStatelessRendererReplyCsiQuery`, `isStatefulRendererReplyCsiQuery` 는 **어느 테스트도 실행 안 함**. `findCsiFinalByteIndex` 만 scan 테스트(§7.3)로 간접 커버.

---

## 4. `terminal-reply-query-scan.ts` — seq 기반 상태ful 스캐너 (extraction 의 간접 오라클 보유)

터미널 출력 스트림에서 **응답 유발 질의**(DSR/CPR, DA1/DA2, DECRQM, XTGETTCAP-adjacent, OSC 10/11) 를 seq 좌표와 함께 추출. chunk 경계에서 쪼개진 질의를 **seq 연속성**으로 재조립. `findCsiFinalByteIndex`(extraction) + `parseTerminalOscColorQuery`(osc-color-reply) 의존(:1-2).

### 4.1 상수 / 타입 (:4-25)

- **`ESC = '\x1b'`**(:4), **`MAX_PENDING_QUERY_CHARS = 4096`**(:5) — pending 상한(**code unit**, extraction 의 64 와 **다름**).
- **`DEVICE_ATTRIBUTES_QUERY_RE`**(:7) `^\[[?>=]?[0-9;]*c$` (DA). **`MODE_QUERY_RE`**(:8) `^\[\??[0-9;]+\$p$` (DECRQM, body 끝 `$p`; extraction 은 `startsWith('\x1b[?')&&endsWith('$p')` 로 판정 — **미세차: scan 은 `?` optional·param 필수**).
- **`TerminalReplyQuerySequence`**(:11-15) — `{ data: string; startSeq: number; endSeq: number }`.
- **`TerminalReplyQueryScanState`**(:17-20) — `{ pending: string; pendingStartSeq: number|null }`.
- **`EMPTY_TERMINAL_REPLY_QUERY_SCAN_STATE`**(:22-25) — `{ pending:'', pendingStartSeq:null }`. export.

### 4.2 `isReplyElicitingCsi(sequence) : boolean` (:27-46) — 비-export

- :28 DA regex, :31 MODE regex, 또는 OR(:34-45): `\x1b[5n`, `\x1b[6n`, `\x1b[?6n`, `\x1b[?996n`, `\x1b[>q`, `\x1b[14t`, `\x1b[16t`, `\x1b[18t`, `\x1b[?u`, `\x1b[?2031h`. **extraction 의 stateless/stateful 목록과 겹치되 다름**(scan 이 `18t`/`?6n`/`?996n`/`?u`/`?2031h` 추가; extraction 이 `>q`를 stateless 로) → **두 모듈 목록을 각각 그대로 이식**(공유 상수화 시 목록 diff 주의).

### 4.3 `boundedPending(input, startIndex) : string` (:48-50)

`input.slice(startIndex, startIndex + 4096)`. **`&str` 면 +4096 char 경계 panic 후보**.

### 4.4 `scanTerminalReplyQuerySequences(data, chunkStartSeq, previous) : {queries, state}` (:52-125) — export, pure

- :57-59 **`continuesPending = previous.pendingStartSeq !== null && previous.pendingStartSeq + previous.pending.length === chunkStartSeq`**. ← **seq 산술이 `pending.length`(code unit)에 의존**. suaegi seq 가 byte 면 non-ASCII 에서 어긋남(오픈질문 5).
- :60 `pending = continuesPending ? previous.pending : ''` (불연속이면 **버림**).
- :61 `input = pending + data`; :62 `inputStartSeq = chunkStartSeq - pending.length`.
- :66 `while offset < input.length`; :67 `candidateIndex = input.indexOf(ESC, offset)`; break if `-1`.
- :71-77 `candidateIndex+1 >= input.length`(ESC 맨 끝) → `pending: boundedPending(input, candidateIndex)`, `pendingStartSeq: inputStartSeq+candidateIndex`.
- **CSI 분기**(:81-85) `startsWith('\x1b[')`: `endIndex = findCsiFinalByteIndex(input, candidateIndex+2)`; `-1` 아니면 `matches = isReplyElicitingCsi(input.slice(candidateIndex, endIndex+1))`.
- **OSC 분기**(:86-95) `startsWith('\x1b]')`: `parseTerminalOscColorQuery`; `partial`→`endIndex=-1`; `match`→`endIndex = osc.endIndex-1`, `matches=true`; `none`→`endIndex = candidateIndex+1`(한 칸 넘김). **osc.endIndex 는 종료자 뒤 exclusive 인덱스라 -1 로 inclusive 변환**.
- **DCS 분기**(:96-102) `startsWith('\x1b P')`: `terminatorIndex = input.indexOf('\x1b\\', candidateIndex+2)`; 있으면 `endIndex = terminatorIndex+1`, `body = input.slice(candidateIndex+2, terminatorIndex)`, `matches = body.startsWith('$q') || body.startsWith('+q')` (DECRQSS/XTGETTCAP 질의). **extraction 엔 없는 분기.**
- **else 분기**(:103-105) `endIndex = candidateIndex`(ESC+비-`[]P` → 그 ESC 한 칸만 소비, non-match).
- :107-113 `endIndex === -1`(CSI/OSC 미완성) → `pending: boundedPending(input, candidateIndex)`, `pendingStartSeq: inputStartSeq+candidateIndex`, 반환.
- :114-120 `matches` → `queries.push({ data: input.slice(candidateIndex, endIndex+1), startSeq: inputStartSeq+candidateIndex, endSeq: inputStartSeq+endIndex+1 })`.
- :121 `offset = endIndex+1`.
- :124 루프 종료 → `state: EMPTY…`.

### 4.5 trap-class (scan)

| site | 구문 | 위험 / 결정 |
|---|---|---|
| :49 | `input.slice(startIndex, startIndex+4096)` | **`&str` +4096 char 경계 panic**. non-ASCII pending 시 발현. |
| :84,100,116 | `input.slice(candidateIndex, endIndex+1)` 등 | endIndex 는 CSI final byte/OSC endIndex/DCS terminator. `&str` 경계 잠재, `&[u8]` 안전. |
| :59,62 | `pending.length` (seq 산술) | **code unit ↔ byte 단위 정합 필수**(오픈질문 5). |
| :67,97 | `indexOf(ESC)`, `indexOf('\x1b\\', candidateIndex+2)` | 바이트 검색. |
| :81,86,96,101 | `startsWith` | ASCII 프레이밍. |
| — | `parseInt` | **없음** (regex/`===` 로만 분류). |

---

## 5. 의존성 배선 (포트 모듈 그래프)

```
terminal-osc-color-reply   (의존 0, FOUNDATION)
        ▲            ▲
        │ parseTerminalOscColorQuery
        │            │
 extraction ──────── scan
   ▲  findCsiFinalByteIndex (scan 이 import)
   │
 (extraction 자체 export 는 scan 이 findCsiFinalByteIndex 만 사용)

terminal-query-reply       (의존 0, 독립 — 클러스터 어느 것도 import 안 함)
```

- `extraction.ts:7` → `parseTerminalOscColorQuery`(osc-color-reply) 사용: `extractHiddenStartupRendererQueryData` :65, :86.
- `scan.ts:1` → `findCsiFinalByteIndex`(extraction) 사용: :82. `scan.ts:2` → `parseTerminalOscColorQuery`(osc-color-reply) 사용: :87.
- `query-reply.ts` 는 **클러스터 import 0** — 완전 독립. 포트 순서: **osc-color-reply → (extraction, query-reply 병렬) → scan**.

---

## 6. suaegi-term 기존 코드와의 관계 (중복 회피)

- **`grid.rs`** — `alacritty_terminal` vte 파서. `Event::PtyWrite(text)`(grid.rs:135) 로 **CSI 질의(DA/DSR/CPR/text-area 등) 응답을 이미 생성**해 `pty_writes` 큐(:121)에 적재, `take_pty_writes()`(:659)로 수거. **즉 에뮬레이터 계층의 응답 생성은 이미 있음.** OSC 10/11 색상 질의는 alacritty 가 `Event::ColorRequest` 로 넘기는데 **현재 grid.rs 에 그 핸들러 없음**(grep 0건) → osc-color-reply 포트가 채울 자리, 단 **alacritty 는 구조화된 `ColorRequest{index, format}` 이벤트를 주므로 바이트 파싱(`parseTerminalOscColorQuery`)이 아니라 응답 빌드(`terminalOscColorQueryReply`)만 필요**할 수 있음(오픈질문 6).
- **`session.rs`** — `reply_tx`/`reply_rx`(:108) **즉시-송신 전용 언바운드 큐**. 리더 스레드가 `grid.take_pty_writes()` 결과를 `reader_reply_tx.try_send`(:226)로 흘림. 주석 :113-114 이 정확히 "DA1/DSR/OSC 색상 질의 핸드셰이크 지연" 을 언급 — **`isTerminalQueryReply`(query-reply) 의 '즉시 송신' 목적과 동일 동기**. 현재는 alacritty 가 만든 것만 즉시 송신하므로 별도 분류기 불필요; **원격/데몬 트랜스포트(plan8-pty-daemon)에서 raw 바이트를 재분류할 때** 이 모듈들이 필요.
- **`encode.rs`** — **아웃바운드 전용**(키→바이트, bracketed paste). `wrap_bracketed_paste`(:317)가 `text.contains/replace`(`&str`) 로 동작하는 선례. inbound OSC/CSI **파싱 코드는 없음** → 이 4개 모듈은 신규.
- **결론**: 중복 없음. 이 클러스터는 **트랜스포트-계층 raw-byte 스캐너/분류기**로, 에뮬레이터(grid)와 아웃바운드 인코더(encode)와 다른 층. 다만 osc-color-reply 의 **응답 빌드** 부분은 alacritty `ColorRequest` 핸들러로 grid 통합 가능.

---

## 7. 오라클: 테스트 케이스별 정밀 (input → expected → crux)

### 7.1 `terminal-osc-color-reply.test.ts` (5 it, 51L) — `parseTerminalOscColorQuery` 만 직접 검증

| # | line | input @0 | expected | crux / trap |
|---|---|---|---|---|
| 1a | 6,9-13 | `ESC]10;?ESC\` | `match slots[10] endIndex=len(8)` | ST 종료, slot10 단일 `?`. |
| 1b | 7,14-18 | `ESC]11;?BEL` | `match slots[11] endIndex=7` | **BEL 종료**, slot11. |
| 2 | 22-28 | `ESC]10;?;?ESC\` | `match slots[10,11] endIndex=len` | **결합 질의, slot10 전용**. |
| 3a | 32 | `ESC]10;?ESC` | `partial` | **split ST**(ESC 끝, `\` 아직). |
| 3b | 33 | `ESC]10;?;?ESC` | `partial` | 결합 질의 split ST. |
| 4a | 37-39 | `ESC]10;?not-a-query ESC\` | `none` | `?` 뒤 `n` 은 종료자·`;` 아님 → none. |
| 4b | 40 | `ESC]11;?ESC X` | `none` | ESC 뒤 `X`(≠`\`) 이고 끝 아님 → 종료자 none(:151). |
| 4c | 41 | `ESC]10;?;#123456 ESC\` | `none` | `?;` 뒤 `#`(≠`?`) → :187 none. |
| 4d | 42 | `ESC]10;?;?;?ESC\` | `none` | `?;?` 뒤 `;`(종료자 아님, :190) → none. **3중 거부**. |
| 4e | 43 | `ESC]11;?;?ESC\` | `none` | slot11 은 `;?` 불가(:181). |
| 5 | 47-49 | `ESC]10;?` + `'x'*10000` | `none` | `?` 뒤 `x` → **10k 스캔 없이 즉시 none**(고정폭 문법, unbounded 대기 방지). |

**미커버(핀 필요)**: 빌드 경로(`cssColorToOscRgb`/`terminalOscColorQueryReply`/`…Replies`) 전량 — hex3/hex6/rgb()/rgba()/percent/clamp/null-병합, **비-ASCII·잘못된 색 문자열**. `sendTerminalOscColorQueryReplies`(다중 OSC 스캔, `sent` 반환) 도 미커버. `parseCssRgbChannels` 의 `/` alpha 분리·`,` vs 공백 분리도 미커버.

### 7.2 `terminal-query-reply.test.ts` (4 it, 99L) — `isTerminalQueryReply`

- **it1(:6-38) true 케이스 22개**: CPR `ESC[3;1R`/`ESC[22;1R`; DSR `ESC[0n`; DA `ESC[?1;2c`/`ESC[?61;4c`/`ESC[>0;276;0c`; window `ESC[6;16;8t`/`ESC[4;384;640t`; DECRPM `ESC[?2026;2$y`/`ESC[4;1$y`(private/ANSI); **OSC `ESC]11;rgb:2828/2c2c/3434 ESC\`(ST) / `ESC]10;rgb:c0c0/c0c0/c0c0 BEL`(BEL)**; DECXCPR `ESC[?12;5R`; text-area `ESC[8;24;80t`; kitty `ESC[?0u`/`ESC[?31u`; DCS `ESC P1$r2 q ESC\`/`ESC P1$r0m ESC\`/`ESC P0$r ESC\`/`ESC P>|xterm.js(5.6.0) ESC\`. **crux**: 각 정규식 1:1, `^$` 완전매치.
- **it2(:40-55)**: **실제 `@xterm/headless` Terminal 에 `ESC[>q` 써서 XTVERSION 응답을 받아** 분류 — **포팅 불가(xterm 의존)**. 응답 형태(`ESC P>|xterm.js(…) ESC\`)는 it1 :37 이 이미 커버 → **포트는 이 케이스를 리터럴 핀으로 대체**.
- **it3(:57-62)**: `ESC[1;2R` → true. **문서화된 Shift+F3/CPR 충돌**(고의 수용).
- **it4(:64-98) false 케이스 26개**: 평문 `yes`/`y`/`\r`/`Ctrl-C(0x03)`; 화살표 `ESC[A/B/C/D`, Home `ESC[H`, End `ESC[F`; 기능키 `ESC[15~`/`ESC[3~`; **bare `ESC`**(len<3); `ESC b`/`ESC P`(len2<3); **kitty 키스트로크 `ESC[97;5u`/`ESC[13u`**(`?` 없음→KITTY_FLAGS 불가); 수식 F1/F2/F4 `ESC[1;2P`/`Q`/`S`; bracketed paste `ESC[200~`/`ESC[201~`; **미종료 OSC `ESC]11;rgb:…`(종료자 없음→false)**; 미종료 DCS `ESC P1$r2 q`. **crux**: 보수적 앵커·최소길이·`?` 표식이 키스트로크 오분류를 막음.

**미커버(핀 후보)**: **non-ASCII OSC 페이로드**(`ESC]11;rgb:…` 대신 유니코드 타이틀류) 를 `OSC_RESPONSE_RE` 가 어떻게 처리하는지 — `&str` regex vs `&[u8]` regex 결정에 필요.

### 7.3 `terminal-reply-query-scan.test.ts` (3 it, 41L) — `scanTerminalReplyQuerySequences`

| # | line | 시나리오 | expected | crux |
|---|---|---|---|---|
| 1 | 8-17 | `'before'+ESC[6n+'after'+ESC[?2031h` @seq100, EMPTY | queries `[{ESC[6n,106,110},{ESC[?2031h,115,123}]`, state EMPTY | **seq = 문자열 인덱스+100**(‘before’=6→106; len4→110; ‘after’5→115; len8→123). **code-unit 좌표.** |
| 2 | 19-29 | `ESC[?`@20 → 그다음 `2026$p`@23 | 1st queries `[]` state`{pending:'ESC[?', startSeq:20}`; 2nd queries `[{ESC[?2026$p,20,29}]` | **연속 chunk 재조립**(23=20+3). MODE_QUERY_RE 매치. |
| 3 | 31-40 | `ESC[?`@20 → `2026$p`@**30** | 2nd queries `[]` | **seq 불연속(30≠23)→pending 폐기**(continuesPending false). |

**미커버(핀 필요, 광범위)**:
- **OSC 분기(:86-95)** — scan 컨텍스트에서 OSC 색상 질의 매치(`endIndex=osc.endIndex-1`) 미검증. `ESC]10;?ESC\` 를 scan 이 query 로 뽑는지 핀.
- **DCS 분기(:96-102)** — `ESC P$q… ESC\`/`ESC P+q… ESC\` 매치, 종료자 없을 때 미매치 미검증.
- **DA/그 외 CSI 목록** — `ESC[c`(DA), `ESC[5n`, `ESC[14t/16t/18t`, `ESC[?u`, `ESC[?996n` 등 개별 미검증(테스트는 `ESC[6n`·`ESC[?2026$p`·`ESC[?2031h` 만).
- **`MAX_PENDING_QUERY_CHARS=4096` 절단** 및 **`boundedPending` char-경계** 미검증.
- **non-ASCII 데이터에서 seq 산술**(오픈질문 5) — 테스트 전부 ASCII.
- **else 분기(:103-105)** ESC+비프레임 문자 스킵 미검증.

### 7.4 extraction (자체 테스트 0) — **전량 핀 필요**

scan 테스트가 `findCsiFinalByteIndex` 만 간접 커버. `extractHiddenStartupRendererQueryData` 의 3-버킷 분류·64 상한 pending·CSI/OSC/잔여 분기, `containsCsiRendererQuery`/`containsStatefulRendererQuery`, `isStateless…`/`isStateful…` 는 **오라클 없음** → 포트가 §3 서술을 기준으로 신규 핀 작성(특히 **non-ASCII CSI 파라미터에서 64-슬라이스 경계**, **미완성 CSI/OSC 의 pending 반환**, **DCS 를 스킵만 하는 동작**).

---

## 8. Codex 교차검증용 오픈 질문

1. **바이트 vs char 동작 모델 (최우선)** — 이 클러스터는 JS UTF-16 `string` 위에서 `data[i]`/`charCodeAt`/`slice`/`indexOf`/`startsWith(s,offset)`/`.length` 를 쓴다. 포트는 **`&[u8]` 위에서 동작**해야 한다(권장): (a) 모든 오프셋이 바이트라 non-char-boundary 슬라이스 panic 이 원천 차단, (b) 문법이 전부 ASCII 프레이밍이라 바이트 직역이 등가, (c) 대상이 PTY 바이트 스트림. `&str` 을 고른다면 **모든 `slice` 를 `str::get`/`char_indices` 로 경계-안전화**하고 64/4096 window 를 char 경계로 스냅해야 하는가? `OSC_RESPONSE_RE` 의 `[^]*` 와 `charCodeAt` 범위검사(0x40-0x7e)는 `&[u8]` 에서 완전 등가임을 확인.
2. **OSC 종료자 BEL vs ST** — `parseTerminalOscTerminator`(osc-color-reply:134-152)는 BEL(1byte)·ST(2byte)·split-ST-partial 3-way. 응답 빌드는 ST 고정(:112). scan 의 DCS 종료자는 `indexOf('\x1b\\')`(:97)로 ST 만. 포트가 BEL/ST endIndex 오프셋(+1 vs +2)과 split-ST partial 을 정확히 재현하는가? `&[u8]` 에서 `starts_with(b"\x1b\\")` 로.
3. **`parseInt`/color-value 파싱** — 이 클러스터엔 `parseInt` 가 **전무**하고 color 채널만 정규식 게이트된 `Number()`(십진수/퍼센트). radix 함정 없음. 단 (a) `value.trim()`(:40) 의 JS↔Rust 공백집합 미세차, (b) `Math.round` half-up(양수라 Rust 등가), (c) `Number('12.5%')/100*255` 부동소수 → `clampByte` 반올림의 비트-정확 일치 — Rust `f64` 산술로 재현 시 오차 없는지?
4. **미테스트 extraction 모듈의 계약** — `extractHiddenStartupRendererQueryData`/`contains*`/`isStateless*`/`isStateful*` 는 오라클이 없다(§7.4). §3 서술이 정확한가? 특히 (a) 잔여 분기(:86-98)의 partial 이 **실제 dead code** 인지(두번째 문자 `]` 필요 조건 분석), (b) extraction 이 **DCS 를 스킵만** 하는 게 의도인지(scan 은 처리), (c) 64 vs scan 의 4096 window 차이가 의도인지. 신규 핀 테스트로 고정해도 되는가?
5. **seq 산술 단위** — scan 의 `pendingStartSeq + pending.length === chunkStartSeq`(:59), `inputStartSeq = chunkStartSeq - pending.length`(:62)는 **code-unit(UTF-16) length** 기준. suaegi 의 PTY 출력 seq 카운터가 **바이트 오프셋**이라면 non-ASCII 청크에서 연속성 판정·startSeq/endSeq 가 어긋난다. 포트는 seq 를 **바이트 단위로 통일**하고 `pending.len()`(바이트)로 계산해야 하는가? 호출 측(hidden-output restore queue / pending-cap drop) 의 seq 정의 확인 필요.
6. **suaegi-term 통합 지점** — (a) `session.rs` `reply_tx` 즉시-송신 큐(:108,226)가 `isTerminalQueryReply` 의 '즉시 송신' 동기와 일치 → 원격/데몬 트랜스포트(plan8)에서 raw 재분류가 필요한 시점은 언제인가? (b) osc-color-reply 의 **응답 빌드**는 alacritty `Event::ColorRequest{index,format}` 핸들러로 grid.rs 에 통합 가능(바이트 파싱 `parseTerminalOscColorQuery` 불필요)한가, 아니면 트랜스포트 경로가 raw OSC 파싱을 별도로 요구하는가? (c) `encode.rs` 아웃바운드·`grid.rs` 에뮬레이터와 중복 없음 확인 — 이 클러스터는 신규 트랜스포트 계층 모듈로 배치.
