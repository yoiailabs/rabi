//! Bounded stream-json scanning shared by result-based providers.
use super::io_error;
use crate::error::Result;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub(super) const FINAL_LINE_BYTES: usize = 16 * 1024 * 1024;

pub(super) struct Scanner {
    reader: BufReader<fs::File>,
    provider: &'static str,
}
impl Scanner {
    pub fn open(events: &Path, provider: &'static str) -> Result<Self> {
        Ok(Self {
            reader: BufReader::new(fs::File::open(events).map_err(io_error)?),
            provider,
        })
    }
    pub fn next(&mut self, protocol_error: &mut Option<String>) -> Result<Option<Value>> {
        let reader = &mut self.reader;
        let mut line = Vec::new();
        loop {
            line.clear();
            let mut overlong = false;
            let mut read_any = false;
            loop {
                let available = reader.fill_buf().map_err(io_error)?;
                if available.is_empty() {
                    break;
                }
                read_any = true;
                let newline = available.iter().position(|byte| *byte == b'\n');
                let consumed = newline.map_or(available.len(), |position| position + 1);
                if line.len() + consumed > FINAL_LINE_BYTES {
                    overlong = true;
                }
                if !overlong {
                    line.extend_from_slice(&available[..consumed]);
                }
                reader.consume(consumed);
                if newline.is_some() {
                    break;
                }
            }
            if !read_any {
                return Ok(None);
            }
            if overlong {
                *protocol_error = Some(format!(
                    "{} event exceeds the finalization line limit ({FINAL_LINE_BYTES} bytes)",
                    self.provider
                ));
                continue;
            }
            // A valid final JSON object need not have a trailing newline. A
            // truncated/malformed nonempty line is a protocol error even if an
            // earlier result was valid; never silently accept partial final output.
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let event = match serde_json::from_slice::<Value>(&line) {
                Ok(event) => event,
                Err(_) => {
                    *protocol_error = Some(format!(
                        "{} emitted a malformed or truncated JSON event",
                        self.provider
                    ));
                    continue;
                }
            };
            return Ok(Some(event));
        }
    }
}
