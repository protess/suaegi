//! Bounded OSC 52 clipboard-write detector.

use base64::Engine;

const MAX_PAYLOAD: usize = 128 * 1024;

#[derive(Debug, Default)]
pub struct Osc52Detector {
    state: State,
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc(Vec<u8>),
    OscEscape(Vec<u8>),
    DiscardOsc,
    DiscardOscEscape,
}

impl Osc52Detector {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut writes = Vec::new();
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                State::Ground if byte == 0x1b => State::Escape,
                State::Ground => State::Ground,
                State::Escape if byte == b']' => State::Osc(Vec::new()),
                State::Escape if byte == 0x1b => State::Escape,
                State::Escape => State::Ground,
                State::Osc(data) if byte == 0x07 => {
                    if let Some(text) = parse_osc52(&data) {
                        writes.push(text);
                    }
                    State::Ground
                }
                State::Osc(data) if byte == 0x1b => State::OscEscape(data),
                State::Osc(mut data) => {
                    if data.len() >= MAX_PAYLOAD + 64 {
                        State::DiscardOsc
                    } else {
                        data.push(byte);
                        State::Osc(data)
                    }
                }
                State::OscEscape(data) if byte == b'\\' => {
                    if let Some(text) = parse_osc52(&data) {
                        writes.push(text);
                    }
                    State::Ground
                }
                State::OscEscape(mut data) => {
                    data.push(0x1b);
                    if byte == 0x1b {
                        State::OscEscape(data)
                    } else {
                        data.push(byte);
                        State::Osc(data)
                    }
                }
                State::DiscardOsc if byte == 0x07 => State::Ground,
                State::DiscardOsc if byte == 0x1b => State::DiscardOscEscape,
                State::DiscardOsc => State::DiscardOsc,
                State::DiscardOscEscape if byte == b'\\' => State::Ground,
                State::DiscardOscEscape if byte == 0x1b => State::DiscardOscEscape,
                State::DiscardOscEscape => State::DiscardOsc,
            };
        }
        writes
    }
}

fn parse_osc52(data: &[u8]) -> Option<String> {
    let payload = data.strip_prefix(b"52;")?;
    let separator = payload.iter().position(|byte| *byte == b';')?;
    let selections = &payload[..separator];
    if selections.is_empty()
        || !selections
            .iter()
            .all(|byte| matches!(byte, b'c' | b'p' | b'q' | b's' | b'0'..=b'7'))
    {
        return None;
    }
    let encoded = &payload[separator + 1..];
    if encoded == b"?" || encoded.len() > MAX_PAYLOAD {
        return None;
    }
    let compact = encoded
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()?;
    Some(String::from_utf8_lossy(&decoded).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bel_and_split_st_terminated_writes() {
        let mut detector = Osc52Detector::default();
        assert_eq!(detector.feed(b"\x1b]52;c;aGVsbG8=\x07"), vec!["hello"]);
        assert!(detector.feed(b"\x1b]52;c;7IS4").is_empty());
        assert_eq!(detector.feed(b"6rOE\x1b\\"), vec!["세계"]);
    }

    #[test]
    fn rejects_queries_bad_selections_and_oversized_payloads() {
        let mut detector = Osc52Detector::default();
        assert!(detector.feed(b"\x1b]52;c;?\x07").is_empty());
        assert!(detector.feed(b"\x1b]52;x;aGk=\x07").is_empty());
        let mut huge = b"\x1b]52;c;".to_vec();
        huge.extend(std::iter::repeat_n(b'A', MAX_PAYLOAD + 1));
        huge.push(0x07);
        assert!(detector.feed(&huge).is_empty());
    }
}
