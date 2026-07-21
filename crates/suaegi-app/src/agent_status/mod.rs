//! 에이전트 상태 배지 — 훅 인입, 배지 리듀서, 하이드레이션 게이트.
//!
//! **훅과 폴링은 다른 질문에 답한다.** `suaegi_term::presence`는 "PTY
//! 포그라운드에 에이전트 프로세스가 있나"만 답하고, 훅은 "그 에이전트가 일하는
//! 중인가 사람을 기다리는가"를 답한다. 둘 중 하나만으로는 배지를 만들 수 없다 —
//! 크래시한 에이전트는 `Stop`을 내지 않고(폴링만 본다), 권한 프롬프트에 막힌
//! `claude`와 추론 중인 `claude`는 `ps`에서 바이트 단위로 같다(훅만 본다).
//! 합성 규칙이 [`contract::reduce`]이고, 그 결정표가 이 모듈의 핵심이다.
//!
//! | 모듈 | 채우는 태스크 |
//! |------|---------------|
//! | `contract` | Task 0 — 타입과 상수 (이 파일 기준 유일한 산출물) |
//! | (미생성) `server` | Task 2 — 루프백 HTTP 수신, `parse_hook` |
//! | (미생성) `inject` | Task 3 — `--settings` JSON·훅 스크립트 생성, `reduce` 구현 |

pub mod contract;
pub mod inject;
pub mod parse;
pub mod server;
pub mod subscription;
