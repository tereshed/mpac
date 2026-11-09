//! Phase 0C candidate, not a selected production terminal architecture.
//! avt::Terminal::dump intentionally omits retained scrollback. The corpus
//! must expose that limitation rather than turning a visible grid into a
//! claimed full snapshot.
use anyhow::{Context, Result};
use avt::Terminal;
use serde_json::{Value, json};

pub struct Utf8Feed {
    terminal: Terminal,
    pending: Vec<u8>,
}

impl Utf8Feed {
    pub fn new(cols: usize, rows: usize) -> Result<Self> {
        Ok(Self {
            terminal: Terminal::with_scrollback(cols, rows, Some(1000))
                .map_err(|_| anyhow::anyhow!("invalid_size"))?,
            pending: Vec::new(),
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    self.terminal.feed(text);
                    self.pending.clear();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    if valid > 0 {
                        // `valid_up_to` guarantees that this prefix is valid.
                        self.terminal.feed(
                            std::str::from_utf8(&self.pending[..valid]).expect("valid prefix"),
                        );
                        self.pending.drain(..valid);
                    }
                    match error.error_len() {
                        Some(invalid) => {
                            self.terminal.feed('\u{fffd}');
                            self.pending.drain(..invalid);
                        }
                        None => break,
                    }
                }
            }
        }
    }

    pub fn dump(&self) -> String {
        self.terminal.dump()
    }
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.terminal.resize(cols, rows);
    }
}

pub fn export_fixtures(fixtures: &Value) -> Result<Value> {
    let mut snapshots = Vec::new();
    for f in fixtures.as_array().context("fixtures_must_be_array")? {
        let cols = f.get("cols").and_then(Value::as_u64).unwrap_or(80) as usize;
        let mut terminal = Utf8Feed::new(cols, 12)?;
        let bytes: Vec<u8> = f["beforeBytes"]
            .as_array()
            .context("bytes_missing")?
            .iter()
            .map(|v| {
                v.as_u64()
                    .filter(|b| *b <= 255)
                    .map(|b| b as u8)
                    .context("invalid_byte")
            })
            .collect::<Result<_>>()?;
        if f["splitBytes"].as_bool().unwrap_or(false) {
            for byte in bytes {
                terminal.feed(&[byte]);
            }
        } else {
            terminal.feed(&bytes);
        }
        if let Some(size) = f["resize"].as_array() {
            terminal.resize(
                size.first()
                    .and_then(Value::as_u64)
                    .context("cols_missing")? as usize,
                size.get(1)
                    .and_then(Value::as_u64)
                    .context("rows_missing")? as usize,
            );
        }
        snapshots.push(json!({"name":f["name"],"snapshot":terminal.dump(),"engine":"avt-0.19.0","scrollback_included":false}));
    }
    Ok(Value::Array(snapshots))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_unicode_is_identical_to_whole_feed() {
        let text = "Привет 你好 👋 café\r\n\x1b[31mred\x1b[0m";
        let mut a = Utf8Feed::new(80, 24).unwrap();
        a.feed(text.as_bytes());
        let mut b = Utf8Feed::new(80, 24).unwrap();
        for byte in text.as_bytes() {
            b.feed(&[*byte]);
        }
        assert_eq!(a.dump(), b.dump());
        assert!(b.pending.is_empty());
    }

    #[test]
    fn invalid_input_does_not_discard_valid_following_bytes() {
        let mut a = Utf8Feed::new(80, 24).unwrap();
        a.feed(&[b'A', 0xff, b'B']);
        let mut b = Utf8Feed::new(80, 24).unwrap();
        b.feed("A\u{fffd}B".as_bytes());
        assert_eq!(a.dump(), b.dump());
        assert!(a.pending.is_empty());
    }

    #[test]
    fn native_dump_restore_can_continue_alternate_screen() {
        let mut a = Utf8Feed::new(80, 24).unwrap();
        a.feed(b"normal\r\n\x1b[?1049h\x1b[2J\x1b[Halt\x1b[3;5H");
        let mut b = Utf8Feed::new(80, 24).unwrap();
        b.feed(a.dump().as_bytes());
        for t in [&mut a, &mut b] {
            t.feed(b"next\x1b[?1049lreturned");
        }
        assert_eq!(a.dump(), b.dump());
    }
}
