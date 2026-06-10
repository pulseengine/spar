//! spar-insight — statistical-discrepancy assistant.
//!
//! Pipeline: CTF trace + AADL model → per-probe-point observed timing
//! distributions → comparison vs `Spar_Trace::Expected_*` →
//! [`DiscrepancyReport`].
//!
//! Tier 1 (this commit): Zephyr CTF events from `k_sem_give`,
//! `k_sem_take`, `k_timer_expiry`, plus the user-defined
//! `probe_point_enter` / `probe_point_exit` markers used to bracket
//! a probe point's body. Rules-based statistical thresholds — the
//! formal-statistics foundation (Hoeffding etc.) is deferred per
//! project memory's R3 proof-assistant decision being parked.
//!
//! Tier 1 corresponds to Zephyr's textual CTF subset emitted by
//! `subsys/tracing/format/format_common.c`'s `tracing_format_string`.
//! Full binary CTF + babeltrace2 ingestion is a v0.9.x follow-up.

pub mod ctf;
pub mod discrepancy;
pub mod report;
pub mod timing;
pub mod zephyr_events;

pub use ctf::{CtfError, CtfEvent, CtfStream, parse_ctf};
pub use discrepancy::{
    Discrepancy, DiscrepancyKind, DiscrepancySeverity, ExpectedTiming, ProbeCoverage, TraceSummary,
    analyze, expected_timings_from_instance,
};
pub use report::DiscrepancyReport;
pub use timing::{ObservedTiming, extract_timings};
pub use zephyr_events::{ZephyrEventClass, classify_event};
