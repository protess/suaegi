# Suaegi MVP 설계 (v0.1)

날짜: 2026-07-20
상태: 승인됨
대상: [stablyai/orca](https://github.com/stablyai/orca) (MIT)의 Rust 클론 — 코어 루프만 추출한 경량판

## 1. 배경과 목표

Orca는 병렬 AI 코딩 에이전트 오케스트레이터(Electron)다. Electron의 메모리(idle 200~300MB)와
렌더링 파이프라인 오버헤드를 제거하는 것이 이 프로젝트의 존재 이유이며, 성능(메모리/속도)이
핵심 차별포인트다. 타겟은 macOS/Windows/Linux 3개 OS 모두 1급 지원이다.

### 스택 결정 (스파이크 검증 완료)

**Rust + iced 0.14 + alacritty_terminal** (iced_term 0.8 포크 기반).

| 근거 | 수치 (macOS 스파이크 실측) |
|---|---|
| 메모리 | iced 단일 프로세스 59MB vs Tauri 4프로세스 98MB vs Electron 200MB+ |
| 번들 | 8.0MB 단일 바이너리 |
| 터미널 | alacritty_terminal 그리드 + GPU 렌더링, 3 OS 동일 (웹뷰 비의존) |

Tauri는 Linux webkitgtk의 WebGL 취약으로 "3 OS 1급 터미널" 요구와 충돌해 탈락.
GPUI는 성능 상한이 더 높지만 문서/API 안정성에서 iced가 우위 (cosmic-term 선례).

### Orca 분석에서 얻은 설계 원칙

1. **Electron 세금 제거**: Orca의 preload 계약 4,500줄, IPC 채널 631개, 메인 프로세스
   `@xterm/headless` 그림자 터미널, 2ms 배칭+ACK 백프레셔는 전부 프로세스 분리의 산물이다.
   단일 프로세스에서는 alacritty_terminal 그리드가 유일한 진실이므로 이 계층이 사라진다.
2. **검증된 쉬운 길 차용**: git은 CLI shell-out (라이브러리 없음, Orca 동일), GitHub은 `gh`
   CLI 위임(인증 통째로 위임), 에이전트는 "터미널 프로세스 + 상태 태그"일 뿐이다.
3. **급진적 축소**: Orca 843 컴포넌트 중 코어 루프(≈10 스토어 슬라이스 상당)만 클론한다.

## 2. MVP 범위

### 포함

1. **Repo 등록**: 로컬 git repo 폴더 선택 → 사이드바에 등록. 영속화.
2. **Worktree 생성**: 이름 입력 → `git worktree add --no-track -b <branch> <path> <base>`.
   이름 새니타이즈 + 충돌 시 숫자 suffix (Orca `worktree-logic.ts` 로직 차용).
   worktree 위치는 설정 가능한 workspace root 아래 (`~/suaegi-workspaces/<repo>/<name>` 기본).
3. **에이전트 실행**: worktree 생성 시 launch 커맨드 주입 (claude / codex / 임의 커맨드).
   에이전트 레지스트리 구조(launch cmd, 감지 프로세스명, 프롬프트 주입 방식)로 확장 가능하게.
4. **터미널 워크벤치**: worktree별 터미널 탭 + 수평/수직 분할(단일 분할 트리),
   GPU 렌더링, 스크롤백(기본 5,000줄), 숨김 탭도 그리드 상태 유지.
5. **에이전트 상태**: working / waiting(입력 대기) / done 뱃지.
   감지 = Claude hooks(로컬 HTTP hook 서버 + 에이전트 설정 자동 주입, 권위) +
   프로세스 테이블 폴링 백스톱 (활성 750ms / 유휴 2s 티어링). OSC 프로토콜은 post-MVP.
6. **Diff 패널**: `git merge-base` 대비 변경 파일 목록(`--name-status --numstat`) +
   파일별 diff 뷰(읽기 전용, 신택스 하이라이트 없음도 허용).
7. **정리/복원**: worktree 삭제(`git worktree remove` + 브랜치 삭제 분리),
   앱 재시작 시 repo/worktree/탭 레이아웃 복원. PTY 프로세스 생존은 post-MVP(데몬 없음).

### 제외 (post-MVP)

SSH 원격 실행, 모바일 컴패니언, GitHub/Linear UI 통합(PR은 터미널에서 `gh`),
Monaco급 에디터/파일 탐색기, 브라우저·에뮬레이터 패인, 자동화, 30종 에이전트 전체 지원,
PTY 생존 데몬, i18n, 온보딩/설정 화면 대부분(최소 설정 파일로 대체).

## 3. 아키텍처

Rust workspace, 4개 크레이트:

```
crates/
├─ suaegi-core   도메인 모델 + 영속화
├─ suaegi-git    git CLI 실행 계층
├─ suaegi-term   PTY + 터미널 그리드 + 에이전트 감지
└─ suaegi-app    iced UI (바이너리)
```

의존 방향: `app → {core, git, term}`, `term → core`, `git → core`. 역방향 금지.

### suaegi-core

- 엔티티: `Repo { id, path, display_name, worktree_base_ref }`,
  `Worktree { id, repo_id, path, branch, display_name, created_with_agent, created_at }`,
  `SessionState { 탭 레이아웃 트리, active 포인터들 }`, `Settings { workspace_root, ... }`.
  Orca의 Project/ProjectHostSetup 이중 구조는 채택하지 않는다 (Repo로 단일화).
- 영속화: 단일 JSON 파일(`~/.config/suaegi/data.json`, serde). 쓰기는 내용 해시 비교 후
  debounce, 롤링 백업 5개(≥1h 간격), 읽기 시 검증 실패 → 백업 폴백 (Orca 패턴 차용).

### suaegi-git

- 모든 git 작업은 `tokio::process`로 git CLI 실행. 라이브러리(git2/gitoxide) 안 씀.
- API: `add_worktree`, `remove_worktree`, `list_worktrees`(porcelain -z 파싱),
  `branch_compare`(merge-base + name-status + numstat), `file_diff`, `repo_status`.
- 영어 출력 강제(`LC_ALL=C`), 타임아웃(worktree add 180s), 에러는 구조화해 UI로 전달.

### suaegi-term

- PTY: `portable-pty`. 셸 선택(POSIX: `$SHELL` + `-l` / Windows: PowerShell→cmd 폴백),
  env 주입(`TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=Suaegi`).
- 그리드: `alacritty_terminal`. 세션당 하나, UI 표시 여부와 무관하게 항상 ingest.
- 에이전트 레지스트리: `AgentKind { launch_cmd, expected_process, prompt_injection }`
  선언 테이블. MVP는 claude/codex/custom 3종.
- 상태 감지: (a) 로컬 HTTP hook 서버(bearer 토큰) + Claude 설정에 hook 자동 설치,
  (b) 프로세스 테이블 폴링 백스톱. 상태는 `working|waiting|done` 3값.
- 종료: 에이전트 세션은 프로세스 그룹 트리 킬.

### suaegi-app (iced)

- Elm 아키텍처: 단일 `App` 상태, 도메인별 메시지 enum 분리
  (`Message::Sidebar(...)`, `Message::Workbench(...)`, `Message::Git(...)` ...).
- 백엔드 작업(git, PTY, hook 서버, 폴링)은 tokio 태스크 → `Subscription`으로 UI 반영.
- 레이아웃: 좌측 사이드바(worktree 카드 + 상태 dot, 리사이즈 가능) /
  중앙 워크벤치(탭바 + 분할 트리 렌더) / 우측 diff 패널(토글).
- 터미널 위젯: iced_term 0.8을 **벤더링(포크)** — 멀티 페인 포커스, 스크롤백 정책,
  외부 그리드 소유(suaegi-term이 소유, 위젯은 뷰만)로 개조. alacritty 코어는 유지.
- Orca의 2중 분할(탭그룹 트리 + 페인 트리)은 **분할 트리 하나로 통일**:
  `LayoutNode = Leaf(TabId) | Split { dir, ratio, children }`.

## 4. 데이터 흐름 (대표 시나리오)

worktree 생성+에이전트 실행:
UI 폼 제출 → `Message::CreateWorktree` → tokio 태스크: suaegi-git `add_worktree`
→ 성공 시 suaegi-term PTY 스폰(launch cmd 주입) → core에 Worktree 추가+영속화
→ UI: 사이드바 카드 추가, 워크벤치 탭 오픈, hook 서버가 상태 push → 카드 dot 갱신.

## 5. 에러 처리

- git/PTY 실패는 해당 worktree 카드에 배지 + 토스트로 표면화. 앱은 계속 동작.
- worktree 생성 실패 시 롤백(생성된 디렉토리/브랜치 정리, Orca sparse 롤백 패턴).
- 영속화 손상 시 백업 폴백, 그래도 실패 시 빈 상태로 기동(크래시 금지).
- PTY 종료(exit)는 탭에 "종료됨" 상태로 표시, 탭은 사용자가 닫을 때까지 유지.

## 6. 테스트

- suaegi-git: 임시 git repo 픽스처 통합 테스트 (생성/삭제/리스팅/diff/에러 케이스).
- suaegi-core: 영속화 라운드트립 + 손상 파일 폴백 테스트.
- suaegi-term: PTY 에코/리사이즈/킬 테스트, 상태 감지 폴링 단위 테스트.
- suaegi-app: 레이아웃 트리 연산(분할/닫기/포커스 이동) 단위 테스트. E2E는 post-MVP.

## 7. 리스크

| 리스크 | 대응 |
|---|---|
| iced 0.14 pre-1.0 breaking change | 버전 고정, 벤더링한 iced_term에서 흡수 |
| iced_term 개조 난이도 | 스파이크로 기본 동작 검증 완료; 초기 마일스톤에 포크 작업 배치 |
| Windows PTY(ConPTY) 품질 | portable-pty가 추상화; Windows 검증 마일스톤 별도 배치 |
| 일반 UI(사이드바 등) 생산성 | cosmic 위젯/iced 생태계 참고, 폴리시보다 동작 우선 |
