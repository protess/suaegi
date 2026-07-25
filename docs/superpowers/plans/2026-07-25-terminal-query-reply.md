# Plan — terminal query/reply/color 클러스터 (transport-layer escape 스캐너) 확정

조사: `docs/superpowers/research/2026-07-25-terminal-query-reply.md` (Orca @ v1.4.150-rc.0, 인용 file:line).
Codex 교차검증 판정 **VALIDATED-WITH-CORRECTIONS** (4모듈 public surface·byte-vs-UTF16·OSC 종결자·no-parseInt·
untested-extraction·seq 산술·오라클 카운트 CONFIRMED, 정정 6 + 6 open-question 답변). **이 문서가 구현 계약이며
조사를 supersede한다** (조사는 몇몇 부정확: reply_tx unbounded 오기·seq counter 존재 가정·64/4096을 "char"로 표기·
test-vector 22/26 오기 — 아래 C1–C8이 정정).

## 0. 결정 (조사 + Codex 확정)

Orca의 **터미널 raw-output query/reply/color escape 스캐너** 클러스터. **4 모듈**:
`terminal-osc-color-reply`(OSC 10/11 색상 query 파싱 + reply 빌드, foundation), `terminal-query-reply`(독립 —
OSC/CSI/DCS reply 분류기), `terminal-reply-query-extraction`(숨은 무테스트 모듈, 160L — hidden-startup renderer
query 추출), `terminal-reply-query-scan`(seq-좌표 스캐너, extraction+osc-color-reply 소비). 총 ~594L + 오라클 3.

**크레이트: 기존 `suaegi-term`에 신규 서브모듈 `reply_query/`** (파일:
`osc_color_reply.rs`, `query_reply.rs`, `reply_query_extraction.rs`, `reply_query_scan.rs`, `mod.rs`).
신규 외부 의존 없음(내부는 alacritty-vte와 무관 — **다른 레이어**). 이 모듈들은 **PURE 로직**(bytes in → 구조화 out).

**⚠️ 배선은 DEFERRED (사람눈, plan8 daemon/remote transport).** 이 스캐너는 로컬 `grid.feed()`(alacritty CSI
파서와 중복) 경로가 **아니라**, output이 버퍼/드롭/숨김/포워딩되는 transport 경계(daemon/remote)에서 raw query를
salvage하는 용도(`scan.ts`의 seq 좌표가 그 증거). 따라서 **순수 로직 + 오라클만 이식**하고 `session.rs`/`grid.rs`
배선·seq counter 소유자는 이식하지 않는다. Emulator-생성 reply는 기존 bounded `reply_tx` 큐(C7)가 이미 처리.

## 1. Codex 반영 결정/정정 (구현자 필독)

- **C1 — 연산 타입은 전부 `&[u8]` (byte-native, NOT `&str`). 64/4096 상한은 Rust에서 BYTE 단위(JS UTF-16 code-unit과
  의도적 divergence — 문서화).** 모든 escape framing(ESC=0x1B, BEL=0x07, `[`=0x5B, `]`=0x5D, `P`=0x50, `\`=0x5C,
  ASCII 숫자·`;?$+>=`, CSI final `0x40..=0x7e`)은 ASCII이므로 byte 직역이 recognition에서 **동치 + panic-proof**
  (`&str[a..b]`는 multibyte 경계 분할 시 **패닉** = M3-clamp급 cardinal sin). 비동치 지점 4개는 **의도적 byte 채택**:
  ① 64/4096 pending window = **byte** 상한(non-ASCII payload에서 JS보다 적게 보존 — 계약에 "bytes" 명시);
  ② `start_seq`/`end_seq` = **absolute byte** 좌표(`u64`); ③ 입력은 raw PTY bytes(invalid UTF-8 포함 가능 —
  디코드 없이 스캔); ④ end offset은 **public 계약 전체에서 exclusive**. 반환 타입: offset `usize`, 스트림 좌표
  `u64`, 추출 데이터는 **byte range 또는 `Vec<u8>`**(디코드된 `String` 금지 — 이 4모듈 중 title/payload 디코드
  필요 함수 **없음**; `isTerminalQueryReply`는 body 미반환, DCS는 ASCII prefix만 검사).
- **C2 — OSC 종결자(bytes): BEL→exclusive end `off+1`; `b"\x1b\\"`(ST)→`off+2`; ESC 단독 & `off+1>=len`→partial(
  split-ST, PTY 청크가 ESC와 `\` 사이를 쪼갬); ESC-기타→none; `off==len`→partial.** reply 빌더는 **항상 ST**
  (`\x1b]{slot};{color}\x1b\\`, `osc-color-reply.ts:112`). scan은 parser의 exclusive end를 내부에서 inclusive로
  변환 후(`scan.ts:91` `endIndex = osc.endIndex - 1`) 다시 +1(`:117-118`) — **port는 public 계약에서 exclusive로
  일관 유지**(내부 inclusive 왕복 재현 불필요, 단 재현 시 `osc.endIndex`가 exclusive임을 기억). DCS scan은 **ST-only**
  (BEL 불인정, `scan.ts:97` `indexOf(ESC\\)`).
- **C3 — 색상 채널은 2단계 ASCII regex-gated `Number()`(parseInt 0건). `\d`→`[0-9]` ASCII lock.**
  percent `^([0-9]+(?:\.[0-9]+)?)%$`(`:89`), plain `^[0-9]+(?:\.[0-9]+)?$`(`:93`) — JS `\d`는 ASCII이나 Rust
  regex `\d`는 Unicode Nd이므로 **`[0-9]` 강제**(radix/sign/hex/exp/whitespace/Infinity/NaN 유입 경로 없음).
  `f64`, `clampByte`로 `[0,255]` 클램프, JS `Math.round`(nonneg는 Rust `f64::round`와 halfway 동일). **핀: `0`·`255`·
  over-255 클램프·`0%`·`100%`·over-100%·소수·`.5` 스케일 경계(예: percent→255 스케일 후 `.5` 직전/직후/정확).**
- **C4 — `terminal-reply-query-extraction`(무테스트, 오라클은 scan 경유 `findCsiFinalByteIndex` 간접뿐)은 최다 신규
  핀 필요.** exports 8개(`HIDDEN_STARTUP_RENDERER_QUERY_PENDING_CHARS=64`, `ExtractedRendererQueryData`,
  `extractHiddenStartupRendererQueryData`, `containsCsiRendererQuery`, `containsStatefulRendererQuery`,
  `findCsiFinalByteIndex`, `isStatelessRendererReplyCsiQuery`, `isStatefulRendererReplyCsiQuery`) 전부 핀:
  - `findCsiFinalByteIndex`: 첫 ASCII final; final 없음; final 앞 non-ASCII UTF-8 bytes; 경계 `0x3f`/`0x40`/`0x7e`/`0x7f`.
  - `isStatelessRendererReplyCsiQuery`: 리터럴 `5n`·`>q`·`14t`·`16t` 각각; 광범위 `endsWith('c')`(비정상 CSI가 `c`로 끝나는 것 포함); near-miss final.
  - `isStatefulRendererReplyCsiQuery`: 정확 `ESC[6n`; private prefix + `$p`; non-private `$p` 거부; prefix/suffix near-miss.
  - `containsCsiRendererQuery`/`containsStatefulRendererQuery`: 노이즈 뒤 stateless/stateful 발견; 다수 비매칭 완전 CSI 선행; **불완전 첫 CSI는 즉시 false**(뒤에 매치 가능 bytes 있어도).
  - `extractHiddenStartupRendererQueryData`: 세 버킷(stateless/stateful/OSC) 한 입력에 동시 + 버킷-로컬 encounter 순서; 비매칭 CSI 폐기; 완전 OSC10·OSC11·복합; 불완전 CSI 보존+64B cap; 불완전 OSC prefix/body/split-ST 보존+cap; 후행 lone ESC 보존; 기존 pending + new data 연결; **DCS `$q`/`+q` skip**(scan과 반대 — 비대칭 핀); `ESC O` 등 기타 introducer skip; skip 후 다수 candidate.
  - window: Rust 계약 **64 byte**(multibyte UTF-8 payload가 byte 64 걸침 케이스); JS 64 code-unit과 의도적 상이 기록.
- **C5 — extraction `:86` OSC-partial fallback 분기는 UNREACHABLE.** `:33`이 second code unit 존재 보장, `:64`가
  `ESC]` 배제, `parseTerminalOscColorQuery`는 fragment가 `ESC]10;`/`ESC]11;` prefix일 때만 partial 반환 — 그러려면
  second unit이 `]`여야 하나 `:64`에서 배제됨, ESC 단독은 `:33-39`에서 이미 반환. → **port에서 이 분기 제거(구조적
  cleanup, 동작 무변경).** 핀: ESC로 시작하는 짧은 suffix를 second byte 전역으로 열거해 `:86` partial 경로 도달 0건
  증명(structural, 동작 mutation 아님 — mutation 하네스에서 "삭제해도 테스트 안 죽음"은 정상, 대신 port가 애초에
  제거하고 unreachable 주석).
- **C6 — seq 산술은 byte 기반 신규 정의(suaegi에 기존 counter 없음 — `generation`은 read당 1증가지 byte 아님, 사용
  금지).** scanner는 `chunk_start_seq: u64`(현재 read 직전 absolute byte 카운트)를 **파라미터로 받음**(소유자=daemon,
  deferred). 연속성: `pending_start_seq + pending.len() as u64 == chunk_start_seq`(`scan.ts:59`);
  `input_start_seq = chunk_start_seq - pending.len()`(`:62`, checked sub); query 좌표 `start_seq =
  input_start_seq + candidate`, `end_seq = input_start_seq + end + 1`(`:117-118`, **exclusive**); usize↔u64는
  checked 변환. **핀: 한글/이모지 bytes 뒤 청크 분할로 byte-space 연속성 검증.**
- **C7 — 배선 DEFERRED + 중복 회피.** `reply_tx`는 **bounded**(`session.rs:108` `crossbeam_channel::bounded(
  REPLY_QUEUE_CAPACITY)` — 조사의 "unbounded" 오기, 소스 주석도 stale). 로컬 `grid.feed()`(`grid.rs:420`
  `parser.advance`)는 alacritty가 이미 CSI query→`Event::PtyWrite`→bounded reply 큐(`session.rs:225-226`)로 처리
  → **이 스캐너를 로컬 경로에 삽입 금지**(중복). raw OSC/color reply 합성은 alacritty 밖에서 raw query bytes를 받는
  transport/daemon 경로에만. alacritty의 구조적 color-request 이벤트 존재 여부는 **미확인 → 계약에서 조건부**(dep API
  검증 후에만 grid 통합, 이번 이식 범위 아님).
- **C8 — `sendTerminalOscColorQueryReplies`는 순수 아님(주입 콜백 `sendInput(reply)` `:227` = effectful DI).**
  port는 `sink: &mut dyn FnMut(&[u8])` 클로저 주입 또는 `Vec<Vec<u8>>` 반환으로 이식(deterministic-modulo-effect).
  나머지 parser/classifier는 순수. `terminal-query-reply.ts`(`isTerminalQueryReply`)는 `.length`/인덱싱/JS regex —
  regex 내 `\d` 있으면 **`[0-9]` lock**, `\b`→`(?-u:\b)`(house 불변식). 20 true / 24 false example 벡터 **전량 이식**
  (조사의 22/26 오기 — 실제 CPR2·DSR1·DA3·window2·DECRPM2·OSC2·DECXCPR1·textarea1·kitty2·DCS4=20; false 24).

## 2. 마일스톤

### M1 — terminal query/reply/color 4모듈 (`suaegi-term::reply_query` 신규, 단일 마일스톤)
- `mod.rs`: 서브모듈 선언 + re-export(public surface만 pub).
- `osc_color_reply.rs`: `css_color_to_osc_rgb`(C3 2-stage regex), `terminal_osc_color_query_reply`(ST 빌드 C2),
  `terminal_osc_color_query_replies`, `terminal_osc_color_query_slots_for_body`, `parse_terminal_osc_color_query`
  (C2 종결자), `send_terminal_osc_color_query_replies`(**C8 sink 콜백**). private: OSC 종결자 파서·CSS 채널·query-body.
- `query_reply.rs`: `is_terminal_query_reply`(독립, C8 20/24 벡터 오라클, `\d` lock).
- `reply_query_extraction.rs`: 8 exports(C4), **`:86` 분기 제거**(C5), 64-byte window.
- `reply_query_scan.rs`: `scan_terminal_reply_query_sequences`(C6 byte-seq, C2 DCS ST-only), 상태 타입·`EMPTY_*` const.

**오라클(3 테스트 전부 이식):** osc-color-reply 5 `it`(색상 파싱/reply 빌드/slots); query-reply 4 `it`(20 true/24
false 벡터); scan 3 `it`(`ESC[6n`/`ESC[?2031h` 완전 startSeq/endSeq·연속 청크·불연속 청크 연속성 상실).

**추가 핀(오라클 미커버 — C4 extraction 전량 + 아래):** C1 non-ASCII payload byte-boundary(패닉 부재 + 64/4096 byte
cap); C2 split-ST partial·BEL/ST +1/+2·DCS ST-only; C3 `.5` 스케일 경계·클램프; C5 dead-branch unreachable 열거;
C6 한글/이모지 청크 분할 byte 연속성; extraction↔scan **DCS 비대칭**(extraction skip vs scan `$q`/`+q` 수용).

*mutation:* `&str` 슬라이스로(non-ASCII 패닉), 64/4096을 char로(divergence), BEL/ST end +1↔+2 스왑, `[0-9]`→`\d`
(Unicode Nd), `Math.round` 방향, extraction 세 버킷 순서/DCS-skip 반전, `isStateful` private-prefix 요구 제거,
`findCsiFinalByteIndex` 경계 `0x3f`/`0x7f` off-by-one, seq `input_start_seq` 부호, `is_terminal_query_reply` 벡터 누락.

## 3. Deferred (명시)
- **transport/daemon 배선**(scanner→daemon output 경계, seq counter 소유자, reply 합성 송신) = plan8 사람눈.
- **alacritty color-request 이벤트 grid 통합**(C7) — dep API 미확인, 조건부.
- **UTF-16 정확재현** — 64/4096 byte cap은 non-ASCII에서 JS code-unit과 의도적 상이(문서화 수용).
- invalid-UTF-8 raw byte 스캔은 이식하되 디코드 경계(title/UI)는 이식 범위 아님.

## 4. 순서 (확정)
M1 단일 마일스톤(4모듈 + 오라클 3 + C1–C8 핀). 불변식: byte-native `&[u8]` panic-proof(C1), OSC BEL/ST 종결자
exclusive(C2), 색상 `[0-9]` lock + f64 클램프(C3), extraction 8-export 전량 핀(C4), dead-branch 제거+unreachable
증명(C5), byte-seq 파라미터 산술(C6), 배선/중복 회피 deferred(C7), send 콜백 effectful(C8), 매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[suaegi-workflow]], [[subagent-output-untrusted]], [[suaegi-resize-seq-global]]
