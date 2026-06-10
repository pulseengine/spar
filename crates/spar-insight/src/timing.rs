//! Observed-timing extraction from a CTF event stream.
//!
//! For each `probe_point_enter(probe_id=X)` event we look for the next
//! `probe_point_exit(probe_id=X)` and compute the duration in
//! nanoseconds. Orphan enters (no matching exit) and orphan exits (no
//! preceding enter) are silently dropped — Tier 1 is best-effort.
//!
//! The resulting [`ObservedTiming`] keys by `probe_id` (the FQN written
//! into the trace by codegen, e.g. `Handler.brake`) and is what the
//! discrepancy layer compares to `Spar_Trace::Expected_*`.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::ctf::CtfEvent;
use crate::zephyr_events::{ZephyrEventClass, classify_event};

/// Observed-timing distribution for one probe point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedTiming {
    pub probe_id: String,
    /// Per-pair durations, in nanoseconds, in arrival order.
    pub samples_ns: Vec<u64>,
}

impl ObservedTiming {
    /// Smallest observed duration. Returns `None` if there are no samples.
    pub fn min_ns(&self) -> Option<u64> {
        self.samples_ns.iter().copied().min()
    }

    /// Largest observed duration. Returns `None` if there are no samples.
    pub fn max_ns(&self) -> Option<u64> {
        self.samples_ns.iter().copied().max()
    }

    /// Arithmetic mean of observed durations, rounded down. Returns
    /// `None` if there are no samples.
    pub fn mean_ns(&self) -> Option<u64> {
        if self.samples_ns.is_empty() {
            return None;
        }
        let sum: u128 = self.samples_ns.iter().map(|&n| n as u128).sum();
        Some((sum / self.samples_ns.len() as u128) as u64)
    }

    /// Number of recorded samples.
    pub fn count(&self) -> usize {
        self.samples_ns.len()
    }
}

/// Extract per-probe timing distributions from a chronological event
/// stream.
///
/// Pairing rule: for each `probe_id`, scan in arrival order and pair
/// each `Enter` with the *next* `Exit` for that same id. An unmatched
/// `Enter` left over at end-of-stream is dropped (no half-samples).
pub fn extract_timings(events: &[CtfEvent]) -> HashMap<String, ObservedTiming> {
    // Per probe_id, FIFO of pending Enter timestamps.
    let mut pending: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut by_probe: HashMap<String, ObservedTiming> = HashMap::new();

    for ev in events {
        match classify_event(ev) {
            ZephyrEventClass::ProbePointEnter { probe_id } => {
                pending.entry(probe_id).or_default().push(ev.timestamp_ns);
            }
            ZephyrEventClass::ProbePointExit { probe_id } => {
                let Some(stack) = pending.get_mut(&probe_id) else {
                    continue;
                };
                let Some(enter_ts) = stack.pop() else {
                    continue;
                };
                let dur = ev.timestamp_ns.saturating_sub(enter_ts);
                let entry = by_probe.entry(probe_id.clone()).or_insert(ObservedTiming {
                    probe_id,
                    samples_ns: Vec::new(),
                });
                entry.samples_ns.push(dur);
            }
            _ => {}
        }
    }
    by_probe
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctf::parse_ctf;
    use pretty_assertions::assert_eq;

    #[test]
    fn extract_timings_pairs_enter_exit() {
        let stream = "
            1000: probe_point_enter(probe_id=\"P\")
            1500: probe_point_exit(probe_id=\"P\")
            2000: probe_point_enter(probe_id=\"P\")
            2300: probe_point_exit(probe_id=\"P\")
        ";
        let evs = parse_ctf(stream).unwrap();
        let t = extract_timings(&evs);
        let p = t.get("P").unwrap();
        assert_eq!(p.samples_ns, vec![500, 300]);
        assert_eq!(p.min_ns(), Some(300));
        assert_eq!(p.max_ns(), Some(500));
        assert_eq!(p.mean_ns(), Some(400));
        assert_eq!(p.count(), 2);
    }

    #[test]
    fn extract_timings_unbalanced_drops_orphan() {
        // Enter without Exit → no sample is emitted.
        let stream = "
            1000: probe_point_enter(probe_id=\"P\")
            1500: probe_point_enter(probe_id=\"P\")
            2000: probe_point_exit(probe_id=\"P\")
        ";
        let evs = parse_ctf(stream).unwrap();
        let t = extract_timings(&evs);
        let p = t.get("P").unwrap();
        // LIFO pairing: the last Enter (1500) matches Exit (2000).
        // The earlier orphan Enter (1000) is silently dropped.
        assert_eq!(p.samples_ns, vec![500]);
        assert_eq!(p.count(), 1);
    }

    #[test]
    fn extract_timings_orphan_exit_dropped() {
        let stream = "9000: probe_point_exit(probe_id=\"P\")";
        let evs = parse_ctf(stream).unwrap();
        let t = extract_timings(&evs);
        assert!(t.is_empty());
    }

    #[test]
    fn extract_timings_independent_probes() {
        let stream = "
            10: probe_point_enter(probe_id=\"A\")
            20: probe_point_enter(probe_id=\"B\")
            30: probe_point_exit(probe_id=\"B\")
            40: probe_point_exit(probe_id=\"A\")
        ";
        let evs = parse_ctf(stream).unwrap();
        let t = extract_timings(&evs);
        assert_eq!(t["A"].samples_ns, vec![30]);
        assert_eq!(t["B"].samples_ns, vec![10]);
    }
}
