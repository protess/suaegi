//! Modifier double-tap detection: a deterministic, stateful state machine that
//! turns a stream of physical modifier key events into "double-tap" gestures
//! (e.g. tap-and-release `Shift`, then press `Shift` again within a short
//! window). Time is *injected* via the `timestamp_ms` parameter of
//! [`ModifierDoubleTapDetector::process`] (never read from a clock), so the whole
//! thing is a pure state transition and mutation-verifiable in isolation.
//!
//! Verbatim port of Orca `src/shared/modifier-double-tap-detector.ts`
//! (@ v1.4.150-rc.0). Two cruxes preserved exactly:
//!   - the window check is `<=` **inclusive** (`ts <= deadline`, TS `:125`), so a
//!     second press at exactly `first_release + 300ms` still completes;
//!   - the deadline is **precomputed at keyUp** as `ts + DOUBLE_TAP_WINDOW_MS`
//!     (TS `:142`), so the measured gap is `(second keyDown) - (first keyUp)`,
//!     never first-down-to-second-down.
//!
//! `PhysicalModifierToken` is Orca's `Exclude<ModifierToken, 'Mod'>`
//! (`keybindings.ts:153-154`): the detector never sees the virtual `Mod` token,
//! only a resolved physical modifier. It is defined standalone here (Rust cannot
//! "exclude" a variant) and is intentionally *not* the same type as this crate's
//! chord-level [`crate::PhysicalModifier`] (`Meta`/`Control`/…), which uses the
//! resolved DOM names; here the tokens keep their `Cmd`/`Ctrl` spelling.

/// Max gap between the first release and the second press. Internal — not
/// user-configurable — and tight enough that normal fast typing never triggers.
/// Mirror of Orca `DOUBLE_TAP_WINDOW_MS` (TS `:5`). Kept as `i64` because the
/// only arithmetic on timestamps is this single `ts + DOUBLE_TAP_WINDOW_MS` add
/// and one `<=` compare — no subtraction, so no overflow/underflow surface.
const DOUBLE_TAP_WINDOW_MS: i64 = 300;

/// A physical modifier — Orca's `PhysicalModifierToken` = `Exclude<ModifierToken,
/// 'Mod'>` (`keybindings.ts:153-154`). The detector's output is always a physical
/// token, never `'Mod'` (TS `:51`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalModifierToken {
    Cmd,
    Ctrl,
    Alt,
    Shift,
}

/// Whether a normalized event is a key press or release. Mirror of Orca
/// `ModifierDoubleTapEventType` (TS `:7`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModifierDoubleTapEventType {
    #[default]
    KeyDown,
    KeyUp,
}

/// A keyboard event normalized to just what the detector needs. Mirror of Orca
/// `ModifierDoubleTapEvent` (TS `:10-17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModifierDoubleTapEvent {
    pub event_type: ModifierDoubleTapEventType,
    /// Which physical modifier this event is about, or `None` for any other key.
    pub modifier: Option<PhysicalModifierToken>,
    /// True only for a bare modifier press/release with no OTHER modifier held.
    pub is_modifier_only: bool,
    pub is_auto_repeat: bool,
}

/// The gesture output. Mirror of Orca `DetectedDoubleTap` (TS `:19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectedDoubleTap {
    pub modifier: PhysicalModifierToken,
}

/// A platform (DOM or Electron) key event, as consumed by
/// [`to_modifier_double_tap_event`]. Mirror of Orca `ModifierKeyEventLike`
/// (TS `:21-30`). Note the flag is `control`, not `ctrl`, matching Orca.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifierKeyEventLike {
    pub event_type: ModifierDoubleTapEventType,
    pub code: Option<String>,
    pub key: Option<String>,
    pub shift: Option<bool>,
    pub control: Option<bool>,
    pub alt: Option<bool>,
    pub meta: Option<bool>,
    pub is_auto_repeat: Option<bool>,
}

/// `MODIFIER_BY_CODE` (TS `:32-41`): `KeyboardEvent.code` -> physical token.
/// Exact match, no case-folding (the JS object index is exact too); a Rust
/// `match` also makes the JS prototype-key footgun (`__proto__` etc.) structurally
/// impossible. `Meta*` is renamed to `Cmd`.
fn modifier_by_code(code: &str) -> Option<PhysicalModifierToken> {
    Some(match code {
        "ShiftLeft" | "ShiftRight" => PhysicalModifierToken::Shift,
        "ControlLeft" | "ControlRight" => PhysicalModifierToken::Ctrl,
        "AltLeft" | "AltRight" => PhysicalModifierToken::Alt,
        "MetaLeft" | "MetaRight" => PhysicalModifierToken::Cmd,
        _ => return None,
    })
}

/// `MODIFIER_BY_KEY` (TS `:43-48`): `KeyboardEvent.key` -> physical token. `Meta`
/// -> `Cmd`, `Control` -> `Ctrl`. Exact match, no case-folding.
fn modifier_by_key(key: &str) -> Option<PhysicalModifierToken> {
    Some(match key {
        "Shift" => PhysicalModifierToken::Shift,
        "Control" => PhysicalModifierToken::Ctrl,
        "Alt" => PhysicalModifierToken::Alt,
        "Meta" => PhysicalModifierToken::Cmd,
        _ => return None,
    })
}

/// Maps a physical key event to the modifier it represents, or `None` for any
/// non-modifier key. Mirror of Orca `modifierFromKeyEvent` (TS `:52-60`): `code`
/// takes priority, then `key`; exact string match, never case-folded.
pub fn modifier_from_key_event(
    code: Option<&str>,
    key: Option<&str>,
) -> Option<PhysicalModifierToken> {
    // TS `:56`: `if (code && MODIFIER_BY_CODE[code])`. An empty `code` is falsy in
    // JS and skips the table; here an unmapped `code` (incl. `""`) simply misses
    // and falls through to the `key` lookup — same observable result.
    if let Some(code) = code {
        if let Some(modifier) = modifier_by_code(code) {
            return Some(modifier);
        }
    }
    // TS `:59`: `key ? (MODIFIER_BY_KEY[key] ?? null) : null`.
    key.and_then(modifier_by_key)
}

/// Whether a modifier OTHER than `modifier` is held in this raw event. Mirror of
/// Orca `otherModifierHeld` (TS `:62-76`). Note the flag names (`control`/`meta`).
fn other_modifier_held(event: &ModifierKeyEventLike, modifier: PhysicalModifierToken) -> bool {
    if modifier != PhysicalModifierToken::Shift && event.shift.unwrap_or(false) {
        return true;
    }
    if modifier != PhysicalModifierToken::Ctrl && event.control.unwrap_or(false) {
        return true;
    }
    if modifier != PhysicalModifierToken::Alt && event.alt.unwrap_or(false) {
        return true;
    }
    if modifier != PhysicalModifierToken::Cmd && event.meta.unwrap_or(false) {
        return true;
    }
    false
}

/// Normalizes a platform key event (DOM or Electron) into the detector input.
/// Mirror of Orca `toModifierDoubleTapEvent` (TS `:79-87`).
pub fn to_modifier_double_tap_event(event: &ModifierKeyEventLike) -> ModifierDoubleTapEvent {
    let modifier = modifier_from_key_event(event.code.as_deref(), event.key.as_deref());
    ModifierDoubleTapEvent {
        event_type: event.event_type,
        modifier,
        // TS `:84`: modifier present AND no other modifier chorded with it.
        is_modifier_only: match modifier {
            Some(m) => !other_modifier_held(event, m),
            None => false,
        },
        is_auto_repeat: event.is_auto_repeat.unwrap_or(false),
    }
}

/// The detector's internal state. Mirror of Orca's `DetectorState` tagged union
/// (TS `:89-92`): idle / first-tap-down / armed-and-waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetectorState {
    Idle,
    Down1 {
        modifier: PhysicalModifierToken,
    },
    Armed {
        modifier: PhysicalModifierToken,
        deadline_ms: i64,
    },
}

/// Deterministic double-tap detector. Mirror of Orca `ModifierDoubleTapDetector`
/// (TS `:94-153`). Feed it normalized [`ModifierDoubleTapEvent`]s plus the event
/// timestamp; it emits a [`DetectedDoubleTap`] on the completing press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifierDoubleTapDetector {
    state: DetectorState,
}

impl Default for ModifierDoubleTapDetector {
    fn default() -> Self {
        // TS `:95`: initial state is idle.
        ModifierDoubleTapDetector {
            state: DetectorState::Idle,
        }
    }
}

impl ModifierDoubleTapDetector {
    /// A fresh detector in the idle state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one normalized event at `timestamp_ms`. Returns the detected
    /// gesture on the completing press, else `None`. Mirror of Orca `process`
    /// (TS `:97-110`).
    pub fn process(
        &mut self,
        event: &ModifierDoubleTapEvent,
        timestamp_ms: i64,
    ) -> Option<DetectedDoubleTap> {
        // TS `:101-104`: a non-modifier key, or a modifier chorded with another,
        // breaks the gesture. (On keyUp, is_modifier_only:false means another
        // modifier is still held — the gesture was already reset at that keyDown.)
        let Some(modifier) = event.modifier.filter(|_| event.is_modifier_only) else {
            self.state = DetectorState::Idle;
            return None;
        };
        match event.event_type {
            // TS `:105-107`: keyUp never emits.
            ModifierDoubleTapEventType::KeyUp => {
                self.on_modifier_up(modifier, timestamp_ms);
                None
            }
            // TS `:109`.
            ModifierDoubleTapEventType::KeyDown => {
                self.on_modifier_down(modifier, event.is_auto_repeat, timestamp_ms)
            }
        }
    }

    /// Force back to idle. Mirror of Orca `reset` (TS `:112-114`).
    pub fn reset(&mut self) {
        self.state = DetectorState::Idle;
    }

    /// Mirror of Orca `onModifierDown` (TS `:116-138`). The single emit point.
    fn on_modifier_down(
        &mut self,
        modifier: PhysicalModifierToken,
        is_auto_repeat: bool,
        timestamp_ms: i64,
    ) -> Option<DetectedDoubleTap> {
        // TS `:121-129`: completion needs armed + SAME modifier + not auto-repeat
        // + within the (inclusive) window. `<=` is load-bearing — a press at
        // exactly the deadline still completes.
        if let DetectorState::Armed {
            modifier: armed_modifier,
            deadline_ms,
        } = self.state
        {
            if armed_modifier == modifier && !is_auto_repeat && timestamp_ms <= deadline_ms {
                self.state = DetectorState::Idle;
                return Some(DetectedDoubleTap { modifier });
            }
        }
        // TS `:131-134`: auto-repeat means the key is held, not tapped.
        if is_auto_repeat {
            self.state = DetectorState::Idle;
            return None;
        }
        // TS `:136-137`: any other fresh bare-modifier press (re)starts the first
        // tap.
        self.state = DetectorState::Down1 { modifier };
        None
    }

    /// Mirror of Orca `onModifierUp` (TS `:140-152`). Arms after the first tap's
    /// release, or clears a stale armed state; every other case (idle, or a
    /// different modifier's release) falls through leaving state unchanged.
    fn on_modifier_up(&mut self, modifier: PhysicalModifierToken, timestamp_ms: i64) {
        match self.state {
            // TS `:141-143`: first-tap release arms; the deadline is anchored HERE
            // at keyUp — `ts + DOUBLE_TAP_WINDOW_MS`.
            DetectorState::Down1 {
                modifier: down_modifier,
            } if down_modifier == modifier => {
                self.state = DetectorState::Armed {
                    modifier,
                    deadline_ms: timestamp_ms + DOUBLE_TAP_WINDOW_MS,
                };
            }
            // TS `:149-151`: a keyup of the armed modifier with no intervening
            // second keydown means the second press was consumed elsewhere (the
            // main process suppresses it for an allowlisted action). Clear armed so
            // a later lone press of the same modifier can't phantom-complete.
            DetectorState::Armed {
                modifier: armed_modifier,
                ..
            } if armed_modifier == modifier => {
                self.state = DetectorState::Idle;
            }
            // Everything else (idle, or a mismatched modifier): unchanged.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ModifierDoubleTapEventType::{KeyDown, KeyUp};
    use PhysicalModifierToken::{Alt, Cmd, Ctrl, Shift};

    // --- Oracle helpers (mirror .test.ts `down`/`up`/`otherKey`) ------------

    /// A bare `keyDown` for `modifier` (is_modifier_only, not auto-repeat).
    /// Mirror of `.test.ts:9-14`.
    fn down(modifier: PhysicalModifierToken) -> ModifierDoubleTapEvent {
        ModifierDoubleTapEvent {
            event_type: KeyDown,
            modifier: Some(modifier),
            is_modifier_only: true,
            is_auto_repeat: false,
        }
    }

    /// A bare `keyUp` for `modifier`. Mirror of `.test.ts:16-21`.
    fn up(modifier: PhysicalModifierToken) -> ModifierDoubleTapEvent {
        ModifierDoubleTapEvent {
            event_type: KeyUp,
            modifier: Some(modifier),
            is_modifier_only: true,
            is_auto_repeat: false,
        }
    }

    /// A non-modifier key event. Mirror of `.test.ts:23-28`.
    const OTHER_KEY: ModifierDoubleTapEvent = ModifierDoubleTapEvent {
        event_type: KeyDown,
        modifier: None,
        is_modifier_only: false,
        is_auto_repeat: false,
    };

    // --- Ported oracle cases (all 10 `it()`s) -------------------------------

    // .test.ts:31-36
    #[test]
    fn emits_when_second_press_lands_inside_window() {
        let mut d = ModifierDoubleTapDetector::new();
        assert_eq!(d.process(&down(Shift), 0), None);
        assert_eq!(d.process(&up(Shift), 10), None);
        assert_eq!(
            d.process(&down(Shift), 200),
            Some(DetectedDoubleTap { modifier: Shift })
        );
    }

    // .test.ts:38-43
    #[test]
    fn does_not_emit_when_second_press_past_window() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10);
        assert_eq!(d.process(&down(Shift), 400), None);
    }

    // .test.ts:45-51
    #[test]
    fn resets_on_intervening_non_modifier_key() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10);
        assert_eq!(d.process(&OTHER_KEY, 20), None);
        assert_eq!(d.process(&down(Shift), 100), None);
    }

    // .test.ts:53-61
    #[test]
    fn treats_different_modifier_as_fresh_gesture_not_completion() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10);
        // Wrong modifier: no emit, but it begins a new first tap.
        assert_eq!(d.process(&down(Alt), 100), None);
        assert_eq!(d.process(&up(Alt), 110), None);
        assert_eq!(
            d.process(&down(Alt), 150),
            Some(DetectedDoubleTap { modifier: Alt })
        );
    }

    // .test.ts:63-70
    #[test]
    fn does_not_treat_auto_repeat_hold_as_tap() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        // Holding the key emits auto-repeat keyDowns — this must cancel the gesture.
        let repeat = ModifierDoubleTapEvent {
            is_auto_repeat: true,
            ..down(Shift)
        };
        assert_eq!(d.process(&repeat, 30), None);
        d.process(&up(Shift), 500);
        assert_eq!(d.process(&down(Shift), 520), None);
    }

    // .test.ts:72-77
    #[test]
    fn does_not_emit_when_another_modifier_is_held() {
        let mut d = ModifierDoubleTapDetector::new();
        let coded = ModifierDoubleTapEvent {
            is_modifier_only: false,
            ..down(Shift)
        };
        assert_eq!(d.process(&coded, 0), None);
        d.process(&up(Shift), 10);
        assert_eq!(d.process(&down(Shift), 100), None);
    }

    // .test.ts:79-88
    #[test]
    fn handles_second_key_down_without_intervening_key_up() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        // Missed keyUp — a fresh (non-repeat) keyDown for the same modifier just
        // restarts the first tap rather than emitting.
        d.process(&down(Shift), 50);
        d.process(&up(Shift), 60);
        // The next press within the window still completes the gesture.
        assert_eq!(
            d.process(&down(Shift), 200),
            Some(DetectedDoubleTap { modifier: Shift })
        );
    }

    // .test.ts:90-100
    #[test]
    fn clears_armed_state_when_second_key_down_was_suppressed() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0); // first tap down -> down1
        d.process(&up(Shift), 10); // first tap up -> armed
                                   // The main process suppressed the second keydown (an allowlisted action
                                   // fired there), but the second tap's keyup still reaches this detector.
        d.process(&up(Shift), 20);
        // A later lone Shift press must NOT phantom-complete from stale armed.
        assert_eq!(d.process(&down(Shift), 200), None);
    }

    // .test.ts:102-108
    #[test]
    fn clears_state_on_reset() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10);
        d.reset();
        assert_eq!(d.process(&down(Shift), 100), None);
    }

    // .test.ts:110-135
    #[test]
    fn normalizes_platform_key_events() {
        assert_eq!(
            modifier_from_key_event(Some("ShiftLeft"), Some("Shift")),
            Some(Shift)
        );
        assert_eq!(
            modifier_from_key_event(Some("MetaRight"), Some("Meta")),
            Some(Cmd)
        );
        assert_eq!(
            modifier_from_key_event(Some("ControlLeft"), Some("Control")),
            Some(Ctrl)
        );
        assert_eq!(modifier_from_key_event(Some("KeyA"), Some("a")), None);

        assert_eq!(
            to_modifier_double_tap_event(&ModifierKeyEventLike {
                event_type: KeyDown,
                code: Some("ShiftLeft".into()),
                key: Some("Shift".into()),
                shift: Some(true),
                ..Default::default()
            }),
            ModifierDoubleTapEvent {
                event_type: KeyDown,
                modifier: Some(Shift),
                is_modifier_only: true,
                is_auto_repeat: false,
            }
        );

        // Another modifier held -> not a bare modifier event.
        let chorded = to_modifier_double_tap_event(&ModifierKeyEventLike {
            event_type: KeyDown,
            code: Some("ShiftLeft".into()),
            key: Some("Shift".into()),
            shift: Some(true),
            meta: Some(true),
            ..Default::default()
        });
        assert_eq!(chorded.modifier, Some(Shift));
        assert!(!chorded.is_modifier_only);

        let non_modifier = to_modifier_double_tap_event(&ModifierKeyEventLike {
            event_type: KeyDown,
            code: Some("KeyA".into()),
            key: Some("a".into()),
            ..Default::default()
        });
        assert_eq!(non_modifier.modifier, None);
        assert!(!non_modifier.is_modifier_only);
    }

    // --- E1-E5: the research's uncovered edge pins --------------------------

    // E1: exact window boundary. deadline = first_up_ts + 300. A press at exactly
    // the deadline still emits (proves `<=`, not `<`); one ms later does not.
    #[test]
    fn e1_exact_window_boundary_is_inclusive() {
        let mut at = ModifierDoubleTapDetector::new();
        at.process(&down(Shift), 0);
        at.process(&up(Shift), 0); // deadline = 0 + 300 = 300
        assert_eq!(
            at.process(&down(Shift), 300),
            Some(DetectedDoubleTap { modifier: Shift }),
            "ts == deadline must complete (<= inclusive)"
        );

        let mut past = ModifierDoubleTapDetector::new();
        past.process(&down(Shift), 0);
        past.process(&up(Shift), 0); // deadline = 300
        assert_eq!(
            past.process(&down(Shift), 301),
            None,
            "one ms past the deadline must not complete"
        );
    }

    // E2: an auto-repeat keyDown on the would-be completing press does NOT emit;
    // it cancels to idle (completion branch requires `!is_auto_repeat` first).
    #[test]
    fn e2_auto_repeat_on_completing_press_does_not_emit() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10); // armed, deadline 310
        let repeat = ModifierDoubleTapEvent {
            is_auto_repeat: true,
            ..down(Shift)
        };
        // Within the window, same modifier, but auto-repeat -> idle, no emit.
        assert_eq!(d.process(&repeat, 100), None);
        // Proof it reset to idle: a lone same-modifier press now does not emit.
        assert_eq!(d.process(&down(Shift), 120), None);
    }

    // E3: a keyUp for a DIFFERENT modifier than the pending one leaves state
    // unchanged (the :140-152 fall-through). down1{Shift} survives up{Alt}, so the
    // subsequent up{Shift}+down{Shift} still completes.
    #[test]
    fn e3_different_modifier_key_up_leaves_state_unchanged() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0); // down1{Shift}
        assert_eq!(d.process(&up(Alt), 5), None); // mismatched up: no change
                                                  // Still down1{Shift}: releasing Shift arms, next Shift press completes.
        d.process(&up(Shift), 10); // armed{Shift, 310}
        assert_eq!(
            d.process(&down(Shift), 200),
            Some(DetectedDoubleTap { modifier: Shift }),
            "down1 Shift must survive an unrelated up Alt"
        );
    }

    // E4: non-monotonic / earlier-than-anchor timestamps behave per the pure
    // arithmetic — deadline is `up_ts + 300`, and any down with `ts <= deadline`
    // completes, even a down whose ts precedes the keyUp anchor.
    #[test]
    fn e4_non_monotonic_timestamps_follow_pure_arithmetic() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 1000);
        d.process(&up(Shift), 1000); // deadline = 1300
                                     // A second down with an EARLIER timestamp than the anchor still satisfies
                                     // `ts <= deadline`, so it completes (no subtraction is performed).
        assert_eq!(
            d.process(&down(Shift), 500),
            Some(DetectedDoubleTap { modifier: Shift })
        );
    }

    // E5: a code present but unmapped falls through to key; a code+key both
    // unmapped -> None.
    #[test]
    fn e5_code_present_but_unmapped() {
        // Unmapped code, unmapped key -> None.
        assert_eq!(modifier_from_key_event(Some("Space"), Some("Space")), None);
        // Unmapped code but a mapped key -> falls through to the key table.
        assert_eq!(
            modifier_from_key_event(Some("Space"), Some("Shift")),
            Some(Shift)
        );
        // Empty code is skipped (JS-falsy parity) -> key table used.
        assert_eq!(modifier_from_key_event(Some(""), Some("Meta")), Some(Cmd));
        // Nothing at all -> None.
        assert_eq!(modifier_from_key_event(None, None), None);
    }

    // --- Extra crux pins ----------------------------------------------------

    // Crux (T2/keyUp-anchor): the deadline is measured from the FIRST KEYUP, not
    // the first keyDown. Here first-down@0, first-up@250; a second down at 500 is
    // within `250 + 300 = 550`, so it completes. If the anchor were the keyDown
    // (`0 + 300 = 300`), 500 would be past and this would NOT emit.
    #[test]
    fn crux_deadline_anchored_at_key_up_not_key_down() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 250); // deadline = 550
        assert_eq!(
            d.process(&down(Shift), 500),
            Some(DetectedDoubleTap { modifier: Shift }),
            "gap is measured from keyUp (250), so 500 <= 550 completes"
        );
    }

    // Crux (same-modifier check): a second press of a DIFFERENT modifier while
    // armed must not complete — it restarts as down1. Distinct from the oracle's
    // full re-gesture, this pins the emit guard's `armed_modifier == modifier`.
    #[test]
    fn crux_different_modifier_while_armed_does_not_complete() {
        let mut d = ModifierDoubleTapDetector::new();
        d.process(&down(Shift), 0);
        d.process(&up(Shift), 10); // armed{Shift, 310}
                                   // Different modifier within the window: must NOT emit.
        assert_eq!(d.process(&down(Cmd), 100), None);
    }
}
