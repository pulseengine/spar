//! Minimal CTF (Common Trace Format) text-stream parser.
//!
//! We support the textual CTF subset that Zephyr's
//! `subsys/tracing/format/format_common.c` produces with
//! `tracing_format_string`:
//!
//! ```text
//! <timestamp_ns>: <event_name>(<arg1>=<v1>, <arg2>=<v2>, ...)
//! ```
//!
//! Each non-empty, non-`#`-prefixed line is one [`CtfEvent`].
//!
//! Full binary CTF + babeltrace2 is intentionally out-of-scope here —
//! it is queued as a v0.9.x follow-up. Tier 1 of the Gale tracing
//! strategy only needs timestamp + event-class + payload, and the
//! textual subset is everything we need from Zephyr to detect
//! Expected_BCET/WCET/Mean discrepancies.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single parsed CTF event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtfEvent {
    /// Timestamp in nanoseconds (origin is opaque; only deltas matter).
    pub timestamp_ns: u64,
    /// Event class name, e.g. `k_sem_give`, `probe_point_enter`.
    pub event_name: String,
    /// `key=value` payload, in declaration order. We use a BTreeMap so
    /// equality / debug output is deterministic across runs.
    pub args: BTreeMap<String, String>,
}

/// A parsed CTF stream: a chronological list of events.
pub type CtfStream = Vec<CtfEvent>;

/// Errors raised while parsing a CTF text stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtfError {
    /// Line was non-empty but did not match the expected grammar.
    Malformed {
        line_no: usize,
        line: String,
        reason: String,
    },
}

impl std::fmt::Display for CtfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CtfError::Malformed {
                line_no,
                line,
                reason,
            } => {
                write!(f, "ctf parse error at line {line_no}: {reason}: {line:?}")
            }
        }
    }
}

impl std::error::Error for CtfError {}

/// Parse a textual CTF stream into a chronological event list.
///
/// Skips blank lines and `#`-prefixed comment lines. Returns the first
/// [`CtfError::Malformed`] encountered.
pub fn parse_ctf(input: &str) -> Result<CtfStream, CtfError> {
    let mut events = Vec::new();
    for (i, line) in input.lines().enumerate() {
        let line_no = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        events.push(parse_line(trimmed, line_no, line)?);
    }
    Ok(events)
}

fn parse_line(trimmed: &str, line_no: usize, raw: &str) -> Result<CtfEvent, CtfError> {
    // Split off `<timestamp>: ` prefix.
    let (ts_str, rest) = match trimmed.split_once(':') {
        Some(p) => p,
        None => {
            return Err(CtfError::Malformed {
                line_no,
                line: raw.to_string(),
                reason: "missing ':' separator after timestamp".to_string(),
            });
        }
    };
    let timestamp_ns: u64 = ts_str.trim().parse().map_err(|e| CtfError::Malformed {
        line_no,
        line: raw.to_string(),
        reason: format!("timestamp not a u64: {e}"),
    })?;

    let rest = rest.trim();
    let open = rest.find('(').ok_or_else(|| CtfError::Malformed {
        line_no,
        line: raw.to_string(),
        reason: "missing '(' after event name".to_string(),
    })?;
    let close = rest.rfind(')').ok_or_else(|| CtfError::Malformed {
        line_no,
        line: raw.to_string(),
        reason: "missing ')' closing event payload".to_string(),
    })?;
    if close < open {
        return Err(CtfError::Malformed {
            line_no,
            line: raw.to_string(),
            reason: "')' appears before '('".to_string(),
        });
    }

    let event_name = rest[..open].trim().to_string();
    if event_name.is_empty() {
        return Err(CtfError::Malformed {
            line_no,
            line: raw.to_string(),
            reason: "empty event name".to_string(),
        });
    }
    let payload = &rest[open + 1..close];
    let args = parse_args(payload, line_no, raw)?;

    Ok(CtfEvent {
        timestamp_ns,
        event_name,
        args,
    })
}

fn parse_args(
    payload: &str,
    line_no: usize,
    raw: &str,
) -> Result<BTreeMap<String, String>, CtfError> {
    let mut args = BTreeMap::new();
    let payload = payload.trim();
    if payload.is_empty() {
        return Ok(args);
    }
    // A single bare token (no '=') means "no args" — we tolerate this
    // shorthand as the spec example uses `event_name(no_args)`.
    if !payload.contains('=') {
        return Ok(args);
    }
    for part in split_top_level_commas(payload) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.split_once('=') {
            Some(kv) => kv,
            None => {
                return Err(CtfError::Malformed {
                    line_no,
                    line: raw.to_string(),
                    reason: format!("payload fragment {part:?} is not key=value"),
                });
            }
        };
        let k = k.trim().to_string();
        let v = strip_quotes(v.trim()).to_string();
        if k.is_empty() {
            return Err(CtfError::Malformed {
                line_no,
                line: raw.to_string(),
                reason: "empty arg name".to_string(),
            });
        }
        args.insert(k, v);
    }
    Ok(args)
}

/// Split a payload string on top-level `,` — quoted strings keep their
/// commas. We only honour double quotes; Zephyr's tracing format never
/// nests, so this is sufficient for Tier 1.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut in_quote = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b',' if !in_quote => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_ctf_minimal_event() {
        let evs = parse_ctf("1234567890: k_sem_give(sem=0xdeadbeef, count=1)").unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].timestamp_ns, 1234567890);
        assert_eq!(evs[0].event_name, "k_sem_give");
        assert_eq!(
            evs[0].args.get("sem").map(String::as_str),
            Some("0xdeadbeef")
        );
        assert_eq!(evs[0].args.get("count").map(String::as_str), Some("1"));
    }

    #[test]
    fn parse_ctf_multiple_events() {
        let stream = "
            1000: k_sem_give(sem=0xa)
            2000: k_sem_take(sem=0xa, timeout=K_FOREVER)
            3000: probe_point_enter(probe_id=\"Handler.brake\")
            4000: probe_point_exit(probe_id=\"Handler.brake\")
        ";
        let evs = parse_ctf(stream).unwrap();
        assert_eq!(evs.len(), 4);
        assert_eq!(evs[0].timestamp_ns, 1000);
        assert_eq!(evs[3].event_name, "probe_point_exit");
        assert_eq!(
            evs[2].args.get("probe_id").map(String::as_str),
            Some("Handler.brake"),
            "double-quoted args should have their quotes stripped"
        );
    }

    #[test]
    fn parse_ctf_handles_missing_args() {
        let evs = parse_ctf("42: tick(no_args)").unwrap();
        assert_eq!(evs.len(), 1);
        assert!(evs[0].args.is_empty());
        let evs = parse_ctf("99: bare()").unwrap();
        assert!(evs[0].args.is_empty());
    }

    #[test]
    fn parse_ctf_skips_blanks_and_comments() {
        let stream = "
            # comment line
            100: a()

            200: b()
        ";
        let evs = parse_ctf(stream).unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event_name, "a");
        assert_eq!(evs[1].event_name, "b");
    }

    #[test]
    fn parse_ctf_reports_malformed_line() {
        let err = parse_ctf("not-a-real-ctf-line").unwrap_err();
        match err {
            CtfError::Malformed { line_no, .. } => assert_eq!(line_no, 1),
        }
    }
}
