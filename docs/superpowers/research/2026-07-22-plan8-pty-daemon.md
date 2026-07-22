# Plan 8 리서치: PTY 생존 데몬 (Orca daemon 이식)

작성일: 2026-07-22
대상: suaegi (Orca의 Rust 클론) — MVP에서 post-MVP로 미뤘던 "PTY 생존 데몬"
(`docs/superpowers/specs/2026-07-20-suaegi-mvp-design.md:53,59`)

Orca 소스: `.../scratchpad/orca-src` (v1.4.150-rc.0), 모듈 `src/main/daemon/`.
아래 모든 `파일:줄` 인용은 이 클론 기준이다.

---

## §0 THE 아키텍처 결정 — 이 플랜의 중심 분기

### Orca가 실제로 하는 것

Orca의 데몬은 **Electron 앱과 분리된, detach된 독립 OS 프로세스**다. Electron `fork()`로
`daemon-entry.js`를 띄우되 `detached:true` + `child.unref()` + `child.disconnect()`로
부모 생명주기에서 완전히 떼어낸다(`daemon-init.ts:440-469,566-572`). 데몬은
`ELECTRON_RUN_AS_NODE=1`로 **순수 Node 프로세스**로 돌고(`daemon-init.ts:462-467`),
cwd는 `userData`로 잡아 워크트리/레포가 삭제돼도 살아남는다(`daemon-init.ts:454-455`).

데몬은 **모든 PTY와 터미널 에뮬레이터(스크롤백)를 소유**한다. 앱(렌더러/메인)은
얇은 클라이언트일 뿐이다. 앱이 닫혀도 데몬과 그 자식 PTY는 계속 살아있고,
새 앱은 데몬에 다시 붙어(re-attach) 라이브 세션과 스크롤백을 그대로 되찾는다.
앱 종료 경로는 `disconnectDaemon()` — **kill이 아니라 disconnect**다
(`daemon-init.ts:897-902`, `index.ts:2481`). 실제로 죽이는 건 dev 부모 종료 때뿐이다.

launchd/systemd/서비스 매니저는 **쓰지 않는다.** 순수하게 "detach된 자식 + PID
파일 + 소켓 probe"로 단일 인스턴스와 재부착을 구현한다. 그래서 데몬은 OS 재부팅이나
완전 로그아웃은 넘기지 못하고 — "앱 재시작/크래시 생존"까지가 설계 목표다.

### 왜 이게 suaegi에서 큰 결정인가

Orca는 Electron이라 "이미 Node 런타임이 손 안에 있다". `fork()` 한 번으로 데몬 프로세스가
생기고, 데몬 코드도 앱 코드도 같은 TypeScript/Node다. suaegi(Rust/iced)에는 **기댈
메인 프로세스가 없다.** 데몬을 만들려면:
- 별도의 데몬 바이너리(또는 `suaegi --daemon` 서브커맨드)를 빌드·배포하고,
- Rust로 detach/daemonize를 직접 구현하고(§4),
- 앱↔데몬 IPC 프로토콜(소켓 + 프레이밍 + 핸드셰이크)을 새로 짜고,
- **터미널 그리드(alacritty) 파싱을 앱 밖 데몬 쪽으로 옮겨야** 한다(스크롤백이 데몬에
  살아있어야 재부착이 의미 있으므로).

이건 suaegi에서 지금까지의 어떤 플랜보다 큰 구조 변경이다.

### 세 갈래 선택지

**(a) 충실한 포트 — 분리 데몬 프로세스 (Orca 그대로)**
- `suaegi-daemon` 별도 바이너리. 앱은 클라이언트. PTY·그리드·에이전트 수명 전부 데몬 소유.
- 장점: 앱 크래시/강제종료에도 세션 생존, Orca와 동일한 UX(라이브 재부착 + 스크롤백),
  Windows 업데이터 kill-zone·에이전트 로그인 세션 등 이미 검증된 문제들의 해법을 물려받음.
- 단점: IPC + daemonize + 크로스플랫폼 + 그리드 이전 = 대형 신규 표면적. 디버깅 난이도 급증.
  suaegi 코어(단일 JSON 영속화)와 무관한 새 상태 저장소(온디스크 스크롤백)도 필요.

**(b) 경량 — 세션 홀더 프로세스 / 재부모화(re-parent)**
- 얇은 "세션 홀더" 프로세스가 PTY FD만 붙들고(그리드 파싱 없이 raw 바이트만 링버퍼로 보관),
  앱은 재부착 시 raw tail을 받아 **자기 쪽 alacritty 그리드로 재생**한다. 그리드는 앱에 남는다.
- 장점: alacritty 파싱을 옮기지 않아도 됨(현 `grid.rs` 그대로). IPC 메시지 셋이 훨씬 작음
  (spawn/write/resize/kill/subscribe + raw 바이트 replay). 데몬 코드가 얇음.
- 단점: "정확한 스크롤백 스냅샷"이 아니라 "raw 바이트 재생"이라 대체화면(alt-screen)/
  리사이즈 경계에서 재생 충실도 이슈가 생김 — Orca가 `PendingOutputRecord`에 resize/clear
  레코드를 섞고(`types.ts:244-247`) alt-screen이면 스크롤백 대신 스냅샷을 쓰는(
  `daemon-pty-adapter.ts:49-51`) 이유가 바로 이 문제. 결국 Orca의 온디스크 복원과
  비슷한 복잡도로 수렴할 위험.

**(c) 최소 — "앱 재시작은 생존, 로그아웃은 아님" + 온디스크 스크롤백만**
- 데몬 없음. 대신 (1) PTY를 `setsid`+detach로 앱에서 떼어 **고아 프로세스로 생존**시키고
  PID/재부착 정보를 디스크에 남기거나, (2) 아예 프로세스 생존을 포기하고 스크롤백만
  디스크에 체크포인트해서 재시작 시 "죽은 세션의 마지막 화면 + cwd 복원"만 제공.
- 장점: 가장 작은 변경. (2)는 IPC가 전혀 필요 없음.
- 단점: (1)은 고아 PTY에 재부착할 채널이 없어(부모가 죽으면 소켓/파이프도 끊김) 사실상
  데몬 없이는 라이브 재부착이 불가능 — 홀더 프로세스가 있어야 함. 즉 (1)은 (b)로 붕괴.
  (2)는 "세션이 계속 돌아간다"는 핵심 요구를 충족 못 함(에이전트가 앱 종료와 함께 죽음).

### 권장 (사용자 결정 사항)

**(b) 세션 홀더 프로세스**를 1차 권장한다. 근거:
- Plan 8의 실제 요구는 "앱을 닫아도 에이전트 PTY가 계속 돌고, 다시 열면 이어서 본다"이다.
  이건 **분리 프로세스가 PTY를 소유**해야만 성립한다 — (c)로는 불가능하고, (a)는 과하다.
- suaegi는 이미 alacritty 그리드 파서가 앱 쪽에 잘 자리잡혀 있다(`grid.rs`). 이걸 데몬으로
  옮기는 게 (a)의 가장 큰 비용인데, (b)는 그리드를 앱에 남기고 데몬은 raw 바이트 파이프 +
  링버퍼 replay만 담당하게 해서 그 비용을 피한다.
- 단, "raw replay 충실도"라는 (b)의 약점은 실측으로 검증해야 한다. alt-screen 앱(vim/claude
  TUI)에서 재부착 재생이 깨지면 Orca처럼 "스냅샷 or 스크롤백" 분기를 데몬에 넣어야 하고,
  그러면 그리드 파싱 일부가 데몬으로 넘어가 (a)에 근접한다.

**최종 판단은 사용자 몫**이다: 앱 강제종료(크래시) 생존까지 원하고 Windows까지 1급으로
지원할 거면 (a)의 충실한 포트가 장기적으로 옳다. "MVP+ 수준의 앱 재시작 생존"이 목표면
(b)로 시작해 필요 시 (a)로 승격하는 게 리스크가 낮다.

---

## §1 아키텍처 맵 (Orca, 모두 file:line 인용)

### 프로세스 모델
- **3계층**: Electron 앱(클라이언트) ↔ 데몬 프로세스(TerminalHost + Session + HeadlessEmulator)
  ↔ node-pty 자식(셸). node-pty는 데몬 프로세스 **안에서** 직접 스폰된다
  (`daemon-entry.ts:168` `spawnSubprocess: createPtySubprocess`; `pty-subprocess.ts:2`가
  `node-pty`를 직접 import). 즉 PTY와 에뮬레이터는 같은 데몬 프로세스에 산다.
- **스폰**: `fork(daemon-entry.js, ['--socket',..,'--token',..,'--pid-record',..,
  '--launch-nonce',..,'--log-file',..], {detached:true, stdio:['ignore','ignore','pipe','ipc'],
  cwd:userData, env:{ELECTRON_RUN_AS_NODE:'1', ORCA_USER_DATA_PATH:..}})`
  (`daemon-init.ts:440-469`). readiness는 IPC `{type:'ready', startedAtMs}`로 신호
  (`daemon-entry.ts:178-182`, 수신 `daemon-init.ts:532-573`). ready 후 `disconnect()`+`unref()`.
- **재부착**: 새 앱은 `probeSocket()`으로 기존 소켓이 살아있는지 확인
  (`daemon-init.ts:107-145`) → `checkDaemonHealth` → 건강하면 fork 없이 그대로 **adopt**
  (`daemon-init.ts:349-401`, "healthy daemon from a previous session ... safe to reuse").

### 데몬이 소유하는 것
- **PTY + 에뮬레이터(스크롤백)**: `Session`이 `HeadlessEmulator`를 소유(`session.ts:108`).
  파싱이 **데몬 쪽**에서 일어나므로 앱이 없어도 스크롤백이 산다. `TerminalHost`가 세션 맵을
  소유(`terminal-host.ts:26`).
- **에이전트 수명**: PTY 자식이 곧 에이전트(claude/codex) 셸. 데몬이 살아있는 한 산다.
  죽으면 `onExit`→`reapSession`으로 에뮬레이터 정리(`terminal-host.ts:124-125,197-204`).

### IPC 트랜스포트 + 메시지 셋
- **트랜스포트**: POSIX = Unix 도메인 소켓 `userData/daemon/daemon-v<N>.sock`;
  Windows = 네임드 파이프 `\\?\pipe\orca-terminal-host-v<N>-<hash>`
  (`daemon-spawner.ts:86-98`). 프로토콜 버전을 **소켓 이름에** 박아 구버전 데몬 재사용을 차단.
- **소켓 2개/클라이언트**: `control`(RPC 요청/응답)과 `stream`(데몬→앱 PTY 출력 이벤트).
  hello의 `role`로 구분(`daemon-hello-protocol.ts:1-7`, 서버 분기 `daemon-server.ts:453-484`).
- **프레이밍**: NDJSON(줄바꿈 구분 JSON), 최대 16MB/줄(`ndjson.ts:5`). UTF-8 경계가 소켓
  청크에 걸쳐 쪼개질 수 있어 `StringDecoder`로 스트리밍 디코드(`client.ts:362`).
- **핸드셰이크/인증**: `hello{version, token, clientId, role}`. 토큰은 데몬이 생성해
  `daemon-v<N>.token`(mode 0600)에 쓰고(`daemon-server.ts:205`), 클라가 읽어 보냄
  (`client.ts:121`). 서버는 버전·토큰 검사(`daemon-server.ts:419-434`), 응답에 데몬
  identity(pid/startedAtMs/launchNonce) 포함(`daemon-server.ts:437-451`) → pid 재활용 가드.
- **RPC 요청 셋**(control, `types.ts:279-303`): createOrAttach, cancelCreateOrAttach, write,
  resize, pausePty, resumePty, setSessionBackground, kill, signal, listSessions,
  shutdownIfIdle, detach, getCwd, getForegroundProcess, confirmForegroundProcess,
  clearScrollback, shutdown, ping, systemResolverHealth, ptySpawnHealth, getSnapshot,
  getSize, takePendingOutput, closeStartupQueryAuthority.
- **스트림 이벤트 셋**(stream, `daemon-stream-events.ts:79-85`): data, exit, terminalError,
  sessionBackgroundMarker, dataGap, transientFact.
- **write/resize/pause/resume는 fire-and-forget notify**(`notify_` 접두사, 응답 없음)
  (`types.ts:382-385`, `client.ts:219-227`) — 핫패스라 응답을 기다리지 않음.

### 라이프사이클
- **시작**: lazy. 첫 `initDaemonPtyProvider()`에서 spawn/adopt(`daemon-init.ts:628-643`,
  호출부 `index.ts:663`).
- **유휴 종료**: 첫 클라 페어가 2분 내 안 붙으면 자멸(INITIAL_ADOPTION_TIMEOUT
  `daemon-server.ts:88`). 마지막 완전 클라가 떠나면 `retirementRequested`
  (`daemon-server.ts:516-526`). 종료하는 앱은 `shutdownIfIdle` RPC로 **빈** 데몬만 은퇴
  시킴(`daemon-server.ts:895-921`). **라이브 세션이 있으면 데몬은 계속 산다.**
- **앱 종료 시**: `disconnectDaemon()`(kill 아님)로 세션을 warm하게 남김
  (`daemon-init.ts:897-902`; will-quit 배선 `index.ts:2428-2481`).
- **크래시 복구**: 어댑터가 죽은 소켓을 감지해 `respawn()`으로 새 데몬 fork
  (`daemon-init.ts:663-669`). 새 데몬은 인메모리 세션이 없으니 **온디스크 콜드 복원**으로 감.
- **고아 정리 / 단일 인스턴스**: 고정 소켓 경로에 bind + probe 선행. 죽은 데몬은
  PID 파일(pid+launchNonce+startedAtMs)로 kill(`killStaleDaemon`, `daemon-init.ts:433`).
  `launchNonce`는 죽어가는 부모의 정리 루틴이 **교체된** 데몬의 파일을 지우는 걸 방지
  (`daemon-spawner.ts:56-63,112-161`).

### 세션 identity & 핸드오프/리플레이
- **sessionId**: 클라가 만드는 안정 문자열(예: `ORCA_TERMINAL_HANDLE`, `terminal-host.ts:112`).
  앱 재시작을 가로질러 같은 id.
- **발견**: 앱은 재기동 시 `listSessions`로 라이브 세션 목록을 받음
  (`daemon-server.ts:892`, `SessionInfo` 스키마 `types.ts:340-351`).
- **웜 재부착**: `createOrAttach(sessionId)`가 살아있는 세션이면 `{isNew:false, snapshot}`
  반환 — snapshot은 **인메모리 에뮬레이터 직렬화**(정확한 스크롤백)
  (`terminal-host.ts:58-70`). 이후 라이브 tail은 stream 소켓으로.
- **콜드 복원**(데몬이 죽었던 경우): 앱의 `DaemonPtyAdapter`가 디스크에서
  `checkpoint.json` + `output.log`를 읽어(`HistoryManager`) `historySeed` ANSI 블롭을 만들고
  createOrAttach에 실어 보냄 → 새 셸이 복원된 스크롤백을 다시 그림
  (`daemon-pty-adapter.ts:47-51,195-320`, `types.ts:73-74`).

### 두 단계 영속화 (핵심)
1. **웜 재부착**(데몬 생존): 인메모리 `HeadlessEmulator` 스냅샷 + 라이브 PTY. 즉시·정확.
2. **콜드 복원**(데몬 사망): 온디스크 증분 로그. 5초 틱마다 `takePendingOutput`으로
   `PendingOutputRecord[]`(output/resize/clear)를 로그에 append(`history-manager.ts:143-175`);
   클린 종료/오버플로/로그 5MB 상한에서 full `checkpoint.json`(`history-manager.ts:178-223`).
   복원은 **새 셸 + 재생된 스크롤백 ANSI seed** — 같은 프로세스는 아니지만 스크롤백은 산다.

---

## §2 suaegi에 없는 것 (구체적 갭)

현 suaegi는 **앱이 PTY를 직접 소유**한다. `SessionStore`가 `TerminalSession`을 들고
(`crates/suaegi-app/src/session_store.rs:239,135`), Drop 시 프로세스 그룹을 kill
(`crates/suaegi-term/src/session.rs` 헤더의 `killpg(SIGKILL)` 설명, `JOIN_DEADLINE` 주석).
앱이 닫히면 세션이 drop → PTY 사망. Plan 8이 바꿔야 할 것들:

| 갭 | 현재 | 데몬 모델에서 필요한 것 |
|---|---|---|
| 데몬 프로세스 자체 | 없음 | detach된 `suaegi-daemon`(또는 `suaegi --daemon`) 바이너리 (§4) |
| IPC | 없음(전부 인프로세스 함수 호출) | Unix 소켓/네임드 파이프 + 프레이밍 + hello/토큰 인증 + control/stream 분리 |
| PTY 소유권 | 앱(`SessionStore`/`TerminalSession`) | 데몬. 앱은 얇은 클라이언트. Drop=kill 규약을 "disconnect=생존"으로 전환 |
| 그리드/스크롤백 위치 | 앱 쪽 alacritty(`grid.rs`, `TerminalGrid`) | (a)면 데몬으로 이전 / (b)면 앱 유지 + 데몬은 raw replay 버퍼 |
| 세션 핸드오프 프로토콜 | 없음 | listSessions → createOrAttach(재부착) → snapshot/tail 재생 |
| 온디스크 스크롤백 복원 | 없음(코어는 단일 JSON) | 데몬 크래시 대비 체크포인트/로그(선택; (a)의 콜드 복원 등가물) |
| daemonize/단일 인스턴스 | 없음 | PID 파일 + launchNonce + 소켓 probe + stale kill |
| 리사이즈 seq 조정 | 전역 AtomicU64(`[[suaegi-resize-seq-global]]`) | 리사이즈가 IPC를 건너 데몬 PTY까지 도달해야 함 — seq 의미 재검토 |

**무엇을 어디로 옮기나 (권장 (b) 기준)**:
- `PtySession`(`pty.rs`)의 스폰/write/resize/kill → 데몬 프로세스로. 앱은 이걸 IPC로 프록시.
- reader/writer 스레드(`session.rs`) → 데몬 쪽. 앱 리더는 소켓에서 raw tail을 받는 것으로 대체.
- `TerminalGrid`(`grid.rs`) → (b)면 앱 유지. 데몬은 세션별 raw 바이트 링버퍼만 보관.
- `SessionStore` → PTY를 직접 들지 않고 **데몬 클라이언트 핸들**을 드는 형태로 재작성.
  Drop=kill 규약(현 `session.rs` 핵심 불변식)을 데몬 경계 밖으로 이동.

---

## §3 마일스톤 분해 (크므로 단계화)

권장 순서(각 단계가 독립적으로 검증 가능하도록):

- **8a — 데몬 프로세스 + IPC 골격 + 프록시 CRUD**
  `suaegi-daemon` 바이너리, Unix 소켓 서버, hello/토큰 핸드셰이크, NDJSON(또는 후술 대안)
  프레이밍, control 소켓 위 spawn(createOrAttach)/write/resize/kill/listSessions,
  stream 소켓 위 data/exit 이벤트. 앱은 인프로세스 PTY 대신 데몬 프록시로 전환.
  **이 단계에서 앱↔데몬이 붙고 에코가 왕복하면 성공.** (아직 생존/재부착 없음 — 데몬을
  앱이 lazy spawn하고 앱 종료 시 죽여도 됨.)

- **8b — 생존 + 웜 재부착 + tail replay**
  앱 종료 = disconnect(kill 아님). 데몬 detach/unref로 앱보다 오래 살기. PID 파일 +
  probe로 재기동 시 기존 데몬 adopt. `listSessions` → `createOrAttach` 재부착 →
  세션별 raw tail(또는 (a)면 emulator snapshot) 재생. **여기서 "앱 껐다 켜도 claude가
  계속 돌고 화면이 이어짐"이 실현됨 — Plan 8의 핵심 데모.**

- **8c — 크래시/고아/단일 인스턴스 하드닝**
  데몬 크래시 시 앱의 respawn + (선택) 온디스크 콜드 복원. 유휴 종료(빈 데몬 은퇴,
  라이브 세션 보존). launchNonce로 교체-데몬 파일 오삭제 방지. 두 앱 인스턴스가 하나의
  데몬을 공유(단일 인스턴스). 좀비/고아 PTY sweep.

(a)를 택하면 8b에 "그리드를 데몬으로 이전 + emulator snapshot 직렬화"가 추가되고,
(b)를 택하면 8b는 "raw 링버퍼 + tail replay"로 더 얇다. **8a는 두 경우 공통.**

---

## §4 크로스플랫폼 리스크 (여기서 문다)

- **Daemonize (가장 어려움)**: Orca는 Electron `fork(detached:true)`+`unref`로 공짜로 얻는다.
  Rust엔 그런 런타임이 없다.
  - **POSIX**: `Command::new(self_exe).arg("--daemon")` + `pre_exec(setsid)` 또는
    double-fork로 세션 리더/부모에서 분리. stdio를 `/dev/null`(또는 로그 파일)로. 부모가
    죽어도 SIGHUP에 안 죽게 새 세션. suaegi는 이미 `setsid` 기반 프로세스 그룹 kill을 알고
    있으니(`session.rs` 주석) 반대 방향(탈출)도 같은 지식 영역.
  - **Windows**: `setsid`/fork 없음. `CREATE_NEW_PROCESS_GROUP` + `DETACHED_PROCESS` +
    `CREATE_BREAKAWAY_FROM_JOB`(잡 오브젝트 탈출)로 detach. Orca도 Windows에서 별도로
    고생함(업데이터 kill-zone 회피용 이미지 relocation `daemon-init.ts:435-460`, ConPTY
    warmup, 자식 start-time OS 조회 부재로 데몬이 startedAtMs 자가보고 `daemon-entry.ts:178-182`).
    이 셋 다 Rust 포트에서 재현해야 함.
- **IPC 트랜스포트 차이**: POSIX Unix 소켓 파일 vs Windows 네임드 파이프. Rust에선
  `tokio::net::UnixListener` vs `tokio::net::windows::named_pipe`(별도 API) — 추상화 레이어
  필요. Orca가 소켓 이름에 프로토콜 버전+경로 해시를 박는 패턴(`daemon-spawner.ts:86-98`)을
  그대로 가져갈 것.
- **파일 권한/보안**: 소켓·토큰 파일 0600(`daemon-server.ts:205-207`). Windows 네임드 파이프는
  ACL로 현재 사용자 제한. 토큰 인증은 두 플랫폼 공통으로 필요(소켓 접근만으론 부족).
- **PID 재활용 가드**: POSIX는 `kill(pid,0)`로 생존 확인 가능하지만 pid 재활용 위험 →
  Orca는 startedAtMs+launchNonce 조합으로 방어. Windows는 자식 start-time 싼 조회가 없어
  데몬이 스스로 보고. Rust 포트도 동일 가드 필요.
- **PTY 백엔드**: suaegi는 `portable-pty`가 ConPTY/openpty를 추상화하므로 데몬 쪽에서도
  동일하게 쓰면 됨 — 이건 오히려 Orca(node-pty)보다 유리. 단 데몬이 순수 Rust 바이너리라
  GPU/디스플레이 init 간섭은 애초에 없음(Orca가 `ELECTRON_RUN_AS_NODE`로 피하려던 문제
  `daemon-init.ts:461`가 suaegi엔 없음).

---

## §5 플랜 작성자를 위한 열린 질문

1. **(a) vs (b) vs (c)** — 앱 강제종료(크래시) 생존까지 목표인가, 아니면 정상적인 앱
   재시작 생존이면 충분한가? 이게 그리드를 데몬으로 옮길지(a)/앱에 남길지(b)를 가른다.
2. **그리드 위치** — (b) 선택 시 raw tail replay의 alt-screen(claude TUI/vim) 충실도를
   실측으로 먼저 검증할 것인가? 깨지면 Orca식 "snapshot or scrollback" 분기가 데몬에 필요.
3. **온디스크 콜드 복원(8c)을 MVP+에 넣을 것인가** — 데몬이 절대 안 죽는다고 가정하고
   웜 재부착만 지원(8b)해도 Plan 8의 요구는 충족된다. 콜드 복원은 큰 추가 표면적.
4. **프레이밍 프로토콜** — NDJSON(Orca)을 그대로 쓸까, 아니면 Rust다운 length-prefixed
   바이너리 프레임을 쓸까? PTY 출력은 바이너리(비UTF-8 가능)라 NDJSON+JSON-string이
   비효율적일 수 있음 — control은 JSON, stream data는 length-prefixed raw가 자연스러움.
5. **데몬 배포/버전** — 데몬 바이너리를 앱과 별도로 배포할지, 같은 바이너리의 `--daemon`
   서브커맨드로 할지. 후자면 self-exe 경로 안정성(업데이트 중 교체) 문제(Orca의 Windows
   relocation과 동형)를 어떻게 다룰지.
6. **suaegi-core 영속화와의 관계** — 데몬은 별도 상태(라이브 세션, 온디스크 스크롤백)를
   가진다. 코어의 단일 JSON과 어떻게 조율하나? 세션 sessionId를 코어의 worktree/탭
   레이아웃과 어떻게 매핑하나(재기동 시 어느 탭이 어느 데몬 세션인지)?
7. **리사이즈 seq** — `[[suaegi-resize-seq-global]]`의 전역 AtomicU64 로직이 IPC 경계를
   건너면서도 성립하나? 리사이즈가 데몬 PTY에 실제 적용됐는지 `getSize`류 readback이
   필요한가(Orca는 `getSize`로 드리프트 체크 `types.ts:230-236`)?

---

### 요약 (반환값)

- **Orca 데몬 모델**:
  - 프로세스: Electron이 `fork(detached:true)`로 띄운 **순수 Node 독립 프로세스**가 모든
    PTY+에뮬레이터(스크롤백)를 소유하고 앱보다 오래 산다. launchd/systemd 안 씀 —
    detach+PID파일+소켓probe로 단일인스턴스/재부착.
  - IPC: **Unix 소켓(POSIX)/네임드 파이프(Win)** 위 NDJSON, 클라마다 control(RPC)+stream
    (PTY 출력) **소켓 2개**, hello+토큰(0600 파일) 인증, 프로토콜 버전을 소켓 이름에 박음.
- **suaegi 권장 아키텍처**: **(b) 세션 홀더 프로세스** — 분리 데몬이 PTY를 소유하되
  alacritty 그리드는 앱에 남기고 데몬은 raw 바이트 링버퍼 + tail replay만 담당. 이유:
  Plan 8의 핵심("앱 껐다 켜도 에이전트 생존+화면 이어짐")은 분리 프로세스가 PTY를
  소유해야만 되고((c) 불가), suaegi의 잘 자리잡은 앱쪽 그리드 파서를 데몬으로 옮기는
  (a)의 최대 비용을 (b)가 회피한다. 단 raw replay 충실도는 실측 검증 필요 —
  깨지면 (a)로 승격. **최종 선택은 사용자 몫.**
- **상위 3개 갭**: (1) 데몬 프로세스+daemonize 자체가 없음(Rust엔 Electron fork가 없어
  POSIX double-fork/setsid, Windows DETACHED_PROCESS+잡 탈출을 직접 구현), (2) IPC가
  전무(소켓+프레이밍+control/stream 분리+토큰 인증 신규), (3) 앱이 PTY를 직접 소유하고
  Drop=kill이 핵심 불변식이라 이를 "disconnect=생존"으로 뒤집고 SessionStore를 데몬
  클라이언트로 재작성해야 함.
