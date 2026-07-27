# Plan — native-chat-agent-support + native-chat-stream-unsubscribe (`suaegi-misc` 모듈 2개, 단일 PR)

조사: Explore 정찰(소스 3개·오라클 3개 통독 + 소비자·`agent-catalog` 인접 id 공간 전수 확인).
출처 `reference/orca/` = **v1.4.146-rc.0**. 소스 62L / 오라클 71L. 둘 다 import 0, 외부 의존 0.

⚠ **원래 `emulator-touch-frame`을 같이 묶으려 했는데 정찰이 반대했고 그게 맞다.**
그쪽은 위험 표면이 **바이트 충실도**(JSON 수치 포맷·키 순서)라 성격이 완전히 다르고,
**사소한 모듈 둘 밑에 유일하게 위험한 모듈을 묻는 건 앞선 바이너리 포트 2건에서 함정을 흘린 바로 그 실패 방식**이다.
→ **별도 PR**로 뺀다(다음 이터레이션).

## 0. 배치 — 둘 다 `suaegi-misc`
[[suaegi-misc-placement-rule]]: 런타임 import 0, 외부 의존 0, `.trim()`도 없어 **`js_ws`조차 불필요**.
에이전트 문자열 4개와 구분자 `':'`는 **명명된 정책 상수라 실격 아님**
(선례: `harness_injected_user_turns`의 19개 태그, `remote_runtime_error`의 대소문자 구분 코드 집합).
`[dependencies]` **계속 빈다**. `regex` 불필요.

## 1. 계약 결정 — `native_chat_agent_support`

- **P1 — ⚠⚠ 집합이 **부분집합도 상위집합도 통과**시킨다.**
  멤버는 정확히 4개(`'claude'`, `'openclaude'`, `'codex'`, `'grok'`, `:5-8`)인데
  **리터럴 내용을 단언하는 테스트가 리포 전체에 없다**(상수는 정의·자체 사용·재export 세 곳이 전부).
  게다가 술어 오라클이 **4개 중 2개만** 통과시킨다 — `'codex'`와 `'grok'`은
  `isNativeChatSupportedAgent`에 **한 번도 넘겨지지 않는다**.
  → `{'claude','openclaude'}`(2개 누락)도, `{…,'openclaw'}`(상위집합)도 **전량 통과**.
  ⚠ 인접 id 공간이 이걸 **실제 위험**으로 만든다(`agent-catalog.tsx`에 34개 id):
  **`'openclaw'`(`:288`)는 `'openclaude'`와 한 글자 차**, **`'claude-agent-teams'`(`:54`)는 `'claude'`로 시작**,
  `'opencode'`(`:88`)도 있다. → `startsWith('claude')` 구현이 **green으로 출하**된다.
  → **핀: 멤버 4개 각각 + 집합 리터럴(원소 수 포함) + 근접 오답 3종**(`openclaw`·`claude-agent-teams`·`opencode`) **거부**.
- **P2 — ⚠ 이 리포 **여덟 번째** 중복 메커니즘: `Set`과 4-리터럴 `===` 체인.**
  `isNativeChatSupportedAgent`는 `Set.has`(`:12`), `resolveNativeChatTranscriptAgent`는
  `===` 4개(`:27,:30`)로 **`NATIVE_CHAT_SUPPORTED_AGENTS`를 아예 안 쓴다**.
  두 수용 도메인이 오늘 **완전히 동일**하고, `is(a) === (resolve(a) !== null)`이 **모든 입력에서 성립**한다
  → 어느 쪽을 다른 쪽으로 재구현해도 **테스트 신호가 0**이다.
  ⚠ 둘은 **독립적으로 확장될 의도**다(집합 = "트랜스크립트 파싱 가능" 게이트, 리졸버 = "어느 포맷" 매핑).
  → **둘 다 유지**하고, 네 멤버 + 음성에서 **일치함을 직접 핀**한다. 한쪽을 지우는 "단순화" 금지.
- **P3 — `shouldStepNativeChatAskAnswer`는 `resolve(agent) === 'claude'`다**(`:19`).
  ⚠ 오라클 7단언을 **세 구현이 전부 통과**한다. 그중 위험한 것:
  `SUPPORTED.has(a) && a !== 'codex' && a !== 'grok'` — **미래에 추가될 집합 멤버를 조용히 상속**한다.
  소비자 주석이 의도를 명시한다(`use-native-chat-interactive-send.ts:97-98`:
  "`=== 'claude'`가 아니라 transcript agent로 게이트해서 OpenClaude도 키스트로크 경로를 타게").
  → `'openclaude'` → **`true`**, `'codex'`/`'grok'` → **`false`**를 각각 핀.
- **P4 — 매칭은 **정확·대소문자 구분·트림 없음***(SameValueZero / `===`). `.trim()`·`.toLowerCase()`·정규식 **전무**.
  → **`js_trim`을 넣지 말 것**. `'Claude'`·`' claude '` 거부를 핀.
- **P5 — `resolve`의 반환은 `Option<NativeChatTranscriptAgent>`**(`:24`, 폴백 `null`, throw 없음).
  `'openclaude'` → **`Claude`로 매핑**된다(`'openclaude'`는 반환 유니온의 멤버가 **아니다**).

## 2. 계약 결정 — `native_chat_stream_unsubscribe`

- **P6 — ⚠⚠ id 합성은 **인코딩이 없다**. 그리고 이건 `agent_notification_id`의 **정확한 역**이다.**
  `` `${agent}:${sessionId}` ``(`:15`) — escape 없음, 구분자 거부 없음.
  비단사다: `('a','b:c')`와 `('a:b','c')`가 **둘 다 `"a:b:c"`**.
  ⚠ **`encode_uri_component`를 "보강"하면 안 된다** — 서버가 이 토큰을
  **인라인·미인코딩으로 재조립**하므로(`native-chat.ts:216`) 인코딩된 토큰은 **영원히 매치되지 않고**
  구독마다 watcher가 샌다. → **`ephemeral_setup_terminal_worktree_id` 선례대로 무가드 이식 + 충돌을 핀**.
  (생산에서 첫 필드가 콜론 없는 집합에서만 오므로 사실상 안전하지만, **그 안전은 다른 모듈에 있고
  여기서는 보이지 않는다** — 두 파라미터 다 맨 `string`이다. 기록만.)
- **P7 — ⚠ `??` vs `||`(`:26`)를 오라클이 구별하지 못한다.**
  픽스처가 `subscriptionId`를 **부재**(`:9`,`:13`) 아니면 **`'pane-2'`**(`:20`, truthy)로만 준다 →
  둘이 **모든 픽스처에서 일치**. 구별 입력은 **`''`뿐**이다.
  ⚠ 서버가 **truthiness로 분기**하므로(`native-chat.ts:292`), `??`를 `||`로 "고치거나"
  Rust에서 **`Some("")`를 `None`으로 접으면** 표적 watcher 해제가 **연결 전체 대량 해제**로 바뀐다.
  → `subscription_id: Option<&str>`로 받고 **`Some("")`는 그대로 `""`를 쓴다**(nullish 의미론). 핀 필수.
- **P8 — 오라클이 **리터럴 id를 진짜로 단언한다****(`:9` `'claude:sess-1'`, `:13-16` `'codex:abc'` + method 문자열).
  → 구분자·필드 순서·method는 **이미 고정돼 있다**(라운드트립 항진명제가 아니다 — 드문 좋은 케이스).
  미고정: escaping(P6), `''`(P7), 충돌(P6).
- **P9 — `NativeChatUnsubscribeRpc`는 필드 2개**: `method`(리터럴 타입 `'nativeChat.unsubscribe'`)와
  `params.subscriptionId: String`. 옵션 없음. Rust에선 `method`를 **연관 상수 + 리터럴 핀**으로.

## 3. 오라클 & 핀
**오라클 전량**: `native-chat-agent-support.test.ts` 46L, `native-chat-stream-unsubscribe.test.ts` 25L.

**추가 핀(오라클 침묵 — 이 PR의 본체):**
**P1 멤버 4개 각각 + 집합 리터럴/원소 수 + 근접 오답 `openclaw`·`claude-agent-teams`·`opencode` 거부**;
**P2 두 메커니즘 일치**(4멤버 + 음성); **P3 `openclaude`→true, `codex`/`grok`→false**;
P4 `'Claude'`·`' claude '`·`''` 거부; P5 `openclaude`→`Claude` 매핑 + 미지 → `None`;
**P6 충돌 `('a','b:c')` == `('a:b','c')`** + 인코딩 부재(콜론/공백/비-ASCII가 **그대로** 나옴);
**P7 `Some("")`가 `""`로 쓰임**(mass-teardown 방지) + `None`이면 합성 id 사용;
P9 method 문자열 리터럴.

*mutation:* P1 멤버 1개씩 제거·`openclaw` 추가·`starts_with("claude")`로, P2 `is`를 `resolve`로 재구현,
P3 `== Claude`를 집합 기반으로·`openclaude` 제외, P4 `trim`/`to_lowercase` 추가,
**P6 `encode_uri_component` 추가**·구분자 변경·필드 순서 교환, **P7 `Some("")`를 `None`으로 접기**,
P9 method 문자열 변경.
**P2의 "한쪽을 다른 쪽으로 재구현"은 등가**지만 **의도적으로 mutation 대상에 넣는다** —
일치 핀(P2)이 그걸 잡아야 하고, 안 잡히면 그 핀이 공허하다는 뜻이다.

## 4. 순서
단일 PR. 같은 도메인·같은 위험 표면(집합 내용 + nullish 의미론)이라 리뷰 컨텍스트가 하나다.
크레이트 헤더 모듈 수(현재 thirty-two)·목록·`Cargo.toml` 설명 반영(신규 2개는 **v1.4.146-rc.0**).
불변식: `suaegi-misc`·`js_ws` 불필요(§0), **집합 리터럴 + 근접 오답 핀**(P1), **두 메커니즘 유지 + 일치 핀**(P2),
`resolve` 경유 게이트(P3), 정확 매칭·트림 없음(P4), `openclaude`→`Claude`(P5),
**무가드 id + 충돌 핀**(P6), **`??` 의미론 유지**(P7), 리터럴 단언 유지(P8), method 상수(P9),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[mutation-harness-mtime-trap]],
[[suaegi-misc-placement-rule]], [[orca-source-location]], [[suaegi-impl-model-sonnet]]
