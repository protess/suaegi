# Plan — terminal-github-pr-link-detector (M2 of 2; `suaegi-ghlink`에 모듈 추가)

조사: M1과 같은 Explore 정찰. 출처 `reference/orca/` = **v1.4.146-rc.0**.
M1(`github_links.rs`)은 PR #102로 머지 완료. 이번엔 `pr_link_detector.rs`를 같은 크레이트에 추가한다.
**의존 추가 없음**(`url`·`suaegi-misc` 이미 있음). `regex` **여전히 금지**.

## 1. 계약 결정

- **Q1 — ⚠⚠ carry는 **URL 전체를 보존**한다. `seenUrls`는 최적화가 아니라 **필수**다.**
  `getPotentialGitHubPRCarry`는 마지막 스킴 위치부터 **끝까지** 잘라 보관한다(`O:65`) —
  "여기까지 소비했다"는 커서가 **없다**. 그래서 다음 청크의 결합 버퍼가 **이미 방출한 URL을 다시 포함**하고,
  `seenUrls`만이 재방출을 막는다.
  ⚠ carry를 "미소비 접미사"로 **정리하지 말 것** — 오라클은 전부 통과하면서 동작이 바뀐다.
- **Q2 — ⚠⚠ 오라클의 `test:116-121`은 **이름이 말하는 것을 고정하지 못한다**.**
  청크1이 `…/pull/42\n`이라 `hasTerminalUrlWhitespace`가 스킴-tail 안의 `\n`을 보고 carry를 **비운다**.
  결과: **`seenUrls`를 통째로 지워도 통과하고, 공백 규칙을 통째로 지워도 통과한다** —
  두 메커니즘이 그 픽스처에서 **서로를 가린다**.
  교차 호출 dedupe는 **다른 파일**(`terminal-title-tracker-parity.test.ts:265-274`)에서만 진짜로 고정된다.
  → **두 메커니즘을 각각 분리해 죽이는 핀을 직접 쓴다**(공백이 없는 종결자, 예: `>`로 끝나는 URL).
- **Q3 — dedupe 키는 **원문 후보 문자열**이고 집합은 **무한 성장**한다.**
  스킴·호스트 대소문자·후행 경로·선행 0에 **민감**하다 → `https://…/pull/42`, `http://…/pull/42`,
  `…/pull/42/files`, `…/pull/007`이 **네 항목**이고 **네 번 방출**된다(PR은 둘).
  ⚠ `(slug, number)`로 "개선"하지 말 것 — 소비자가 pr-link를 "최신 연관 갱신"으로 다룬다.
  리셋은 `reset()`뿐이고 **carry와 seen 둘 다** 비워야 한다.
- **Q4 — `\s` 두 곳은 ECMAScript 집합**(`O:73`, `O:97`) → `suaegi_misc::is_js_whitespace`.
  Rust `\s`/`char::is_whitespace`는 U+0085를 **포함**하고 U+FEFF를 **제외**한다(정반대).
  도달 가능한 실패 둘: `…/pull/42\u{00A0}x`는 JS가 NBSP에서 종결해 PR 42를 낸다(ASCII 스캔은 삼켜서 링크 소실);
  `…/pull/42\u{FEFF}`가 청크 끝이면 JS는 carry를 비우고 Rust는 유지한다.
- **Q5 — 빠른 경로 게이트는 **원문**에서 돈다**(`O:142`), 스트립 **이전**이다.
  그래서 `/pu\x1b[0mll/`은 스트립하면 붙는데도 **여기서 빠져나간다**. 주석이 의도(핫패스)라고 명시.
  스트립 후 게이트하는 포트는 Orca가 내지 않는 링크를 낸다.
- **Q6 — 스트립 순서와 기준**: SGR 제거(`O:149`) → 커서 제어를 **가드 문자 U+FFFD로 치환**(`O:150`).
  ⚠ carry는 **원문(`rawCombined`)에서** 다시 계산한다(`O:171`), 스트립본이 아니다 —
  이게 청크 경계에 걸친 SGR(`…/pull/10` + `\x1b` ‖ `[22m\n`)을 살린다.
- **Q7 — `[\x08\x0b\x0c]` 세 멤버는 **효과가 서로 다르다**.**
  가드가 없다면 `\x08`(BS)은 JS 공백이 **아니라서** 삼켜져 `…/pull/1\x080`이 `10`으로 **융합**되고,
  `\x0b`/`\x0c`는 JS 공백**이라서** 종결자가 되어 **잘못된 PR #1**을 낸다.
  오라클이 셋을 **한 테스트에 묶어** 검증하므로 `\x08`만 빼는 변이는 **살아남는다** → **셋을 따로 핀**.
- **Q8 — U+FFFD 가드는 **진짜 U+FFFD도 거부**한다**(`O:34`). node-pty의 UTF-8 디코더가 잘못된 바이트에서
  U+FFFD를 내므로 실제로 도달 가능하다. 미검증 → 핀.
- **Q9 — 경계 게이트**: 후보가 결합 버퍼 **끝까지** 닿으면 이번 호출엔 **방출하지 않는다**(`O:159-161`).
  다음 청크가 종결자를 줄 때까지 기다린다.
- **Q10 — 캡 둘은 **UTF-16 code unit**이다**: carry 512(`O:20`), URL 2048(`O:21`).
  오라클이 8000자 오버슛이라 바이트/char/UTF-16 셋을 **구별 못 한다** →
  `encode_utf16().count()`로 쓰고 경계(511/512/513, 2047/2048/2049) 핀.
- **Q11 — `endsWithHttpSchemePrefixFragment`는 **양성 커버리지 0**이다**(`O:45-54`).
  호출은 되지만 어떤 테스트에서도 **비어있지 않은 값을 반환하지 않는다**.
  부분 스킴 carry 메커니즘 **전체가 미고정** → `…creating h` ‖ `ttps://…/pull/1\n` 핀 필수.
  ⚠ `http://` 전용 가지는 접미사가 `http:`·`http:/`일 때만 도달한다(더 짧으면 `https://` 접두가 먼저 맞는다).
- **Q12 — 후행 구두점 트림은 `/pull/`·길이 필터 **뒤**에 온다**(`O:37` 실행 시점 `O:163`).
  2049자 후보가 `))))` 로 끝나면 트림 후 2045여도 **먼저 버려진다**. 순서 유지.
  클래스는 `)`, `,`, `.`, `;`, `]`, `}` — **여는 괄호 없음**, `>`·`"` 없음.
- **Q13 — 문자열 인덱스는 전부 UTF-16이다**(`O:48`,`:49`,`:65`,`:120`). Rust `&str` 바이트 인덱싱은
  **패닉**한다 → `char_indices` 기반 스캔. 바이트 네이티브 정당화(`bell_detector`/`partial_escape_tail`의
  "모든 유의 바이트가 ASCII")가 **여기선 성립하지 않는다**(비-ASCII 공백 + UTF-16 캡).
- **Q14 — `matchAll` 스파이 테스트(`test:165-178`)는 Rust 대응물이 없다.**
  의도만 이식: 노이즈 20 000개 + 진짜 URL 1개 → 결과 1개. 조용히 빠뜨리지 말 것.
- **Q15 — API 모양**: `KittyKeyboardModeTracker` 선례를 따른다 —
  `pub struct TerminalGitHubPRLinkDetector { carry, seen_urls }`, `observe(&mut self, &str) -> Vec<...>`,
  `reset(&mut self)`. `reset`은 Orca의 "클로저 재생성"(`terminal-output-side-effects.ts:314`)에 해당하고
  **dedupe 기억이 사라진다**는 문서화된 결과를 갖는다.

## 2. 오라클 & 핀
**오라클 17케이스 전량**(`terminal-github-pr-link-detector.test.ts`) +
**parity 픽스처**(`terminal-title-tracker-parity.test.ts:265-274` — 유일한 진짜 교차호출 dedupe).

**추가 핀(오라클 침묵):** **Q2 두 메커니즘 분리**(공백 없는 종결자로 carry가 남는 경우 → seen이 죽이고,
seen을 뺐을 때 재방출됨을 증명); **Q11 부분 스킴 carry 양성 케이스** + `http:`/`http:/` 가지;
Q7 `\x08`/`\x0b`/`\x0c` **각각**; Q8 진짜 U+FFFD; Q10 두 캡의 정확 경계 3점씩;
Q4 U+00A0/U+3000/U+FEFF 종결자와 U+0085 **비**종결자; Q3 스킴/대소문자/후행경로/선행0이 **각각 별 항목**;
`isTerminalUrlTerminator`의 `"`·`'`·`<`·`>` 네 arm; Q12 트림 순서(2049자 + `))))`);
`parsed.type !== 'pr'` 가지(`https://…/issues/42/pull/x`); R3 클래스 `,`·`;`·`]`·`}`;
`findNextHttpSchemeIndex`가 한 청크에 `http://`와 `https://`를 **둘 다** 볼 때.

*mutation:* Q1 carry를 미소비 접미사로 트림, Q2 `seenUrls` 제거·공백 규칙 제거(**각각**),
Q3 키를 `(slug, number)`로, Q4 `\s`/`char::is_whitespace`, Q5 스트립 후 게이트, Q6 carry를 스트립본에서,
Q7 클래스에서 한 멤버씩 제거, Q8 가드 검사 제거, Q9 경계 게이트 제거, Q10 `len()`/`chars().count()`,
Q11 부분 스킴 함수 제거·배열 순서 반전, Q12 트림을 필터 앞으로, R1에 `(?i)` 추가.

## 3. 순서
단일 PR. 크레이트는 이미 있으므로 모듈 하나 + `lib.rs` 선언만 추가.
불변식: carry 전체 보존 + seen 필수(Q1), **두 메커니즘 개별 핀**(Q2), 원문 키 dedupe(Q3),
ECMAScript 공백(Q4), **원문 게이트**(Q5), 원문 기준 carry(Q6), 제어문자 셋 개별(Q7),
진짜 U+FFFD 거부(Q8), 경계 게이트(Q9), **UTF-16 캡**(Q10), 부분 스킴 carry(Q11),
트림 순서(Q12), `char_indices`(Q13), 노이즈 케이스 이식(Q14), tracker API(Q15),
매 회귀 mutation 검증.
관련: [[mutation-verify-regression-tests]], [[mutation-survivor-triage]], [[orca-source-location]],
[[suaegi-impl-model-sonnet]]
