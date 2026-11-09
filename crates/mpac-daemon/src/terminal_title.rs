//! A candidate label from this execution's terminal stream, never an automatic
//! rename or an instruction. Ignore other OSCs, cap buffering and allow chunked
//! UTF-8/terminators. Existing manually chosen session titles remain separate.
#[derive(Default)]
pub struct OscTitleParser {
    escape: bool,
    osc: bool,
    osc_escape: bool,
    discard: bool,
    payload: Vec<u8>,
}

impl OscTitleParser {
    pub fn push(&mut self, bytes: &[u8]) -> Option<String> {
        let mut latest = None;
        for &byte in bytes {
            if self.osc {
                if self.osc_escape && byte == b'\\' || byte == 7 {
                    if !self.discard
                        && let Some(title) = candidate(&self.payload)
                    {
                        latest = Some(title);
                    }
                    self.osc = false;
                    self.osc_escape = false;
                    self.discard = false;
                    self.payload.clear();
                } else if byte == 0x1b {
                    if self.osc_escape {
                        self.discard = true;
                    }
                    self.osc_escape = true;
                } else {
                    if self.osc_escape {
                        // An ESC inside the payload was not a terminator.
                        self.discard = true;
                    }
                    self.osc_escape = false;
                    if self.payload.len() < 1024 && !self.discard {
                        self.payload.push(byte);
                    } else {
                        self.discard = true;
                    }
                }
            } else if self.escape && byte == b']' {
                self.escape = false;
                self.osc = true;
                self.payload.clear();
            } else {
                self.escape = byte == 0x1b;
            }
        }
        latest
    }
}

fn candidate(payload: &[u8]) -> Option<String> {
    let title = payload
        .strip_prefix(b"0;")
        .or_else(|| payload.strip_prefix(b"2;"))?;
    let title = std::str::from_utf8(title).ok()?.trim();
    if title.is_empty()
        || title.chars().any(|c| {
            c.is_control() || matches!(c, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return None;
    }
    let mut end = title.len().min(120);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    Some(title[..end].trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_can_span_every_byte_including_utf8_and_st() {
        let mut parser = OscTitleParser::default();
        let stream = "output\x1b]2;Разбор авторизации\x1b\\more";
        let mut titles = Vec::new();
        for byte in stream.as_bytes() {
            if let Some(title) = parser.push(&[*byte]) {
                titles.push(title);
            }
        }
        assert_eq!(titles, ["Разбор авторизации"]);
    }

    #[test]
    fn latest_valid_title_ignores_hyperlinks_clipboard_and_empty_titles() {
        let mut parser = OscTitleParser::default();
        assert_eq!(parser.push(b"\x1b]0;First\x07\x1b]8;;https://example.test\x07\x1b]52;c;payload\x07\x1b]2;Second\x07\x1b]2;\x07"), Some("Second".into()));
    }

    #[test]
    fn oversized_or_malformed_payload_is_discarded_and_parser_recovers() {
        let mut parser = OscTitleParser::default();
        assert_eq!(parser.push(b"\x1b]0;"), None);
        assert_eq!(parser.push(&vec![b'x'; 10_000]), None);
        assert!(parser.payload.len() <= 1024);
        assert_eq!(
            parser.push(b"\x07\x1b]2;Recovered\x07"),
            Some("Recovered".into())
        );
        assert_eq!(
            parser.push(b"\x1b]2;bad\xff\x07\x1b]2;bad\x1b[31m\x07"),
            None
        );
        assert_eq!(parser.push(b"\x1b]0;Good\x1b\\"), Some("Good".into()));
    }

    #[test]
    fn controls_and_direction_overrides_do_not_become_labels() {
        let mut parser = OscTitleParser::default();
        assert_eq!(parser.push(b"\x1b]2;Line\nOther\x07"), None);
        assert_eq!(
            parser.push("\x1b]2;Hidden\u{202e}suffix\x07".as_bytes()),
            None
        );
    }

    #[test]
    fn bounded_label_never_splits_a_utf8_character() {
        let mut parser = OscTitleParser::default();
        let title = parser
            .push(format!("\x1b]2;{}\x07", "я".repeat(100)).as_bytes())
            .unwrap();
        assert_eq!(title, "я".repeat(60));
    }
}
