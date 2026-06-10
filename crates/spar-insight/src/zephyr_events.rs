//! Zephyr CTF event-class catalog (Tier 1 of the Gale tracing strategy).
//!
//! Tier 1 covers the small set of Zephyr kernel primitives that are
//! emitted unconditionally by `subsys/tracing` plus the user-defined
//! `probe_point_enter` / `probe_point_exit` markers we synthesise from
//! generated probe-point bodies (see `Spar_Trace::Probe_Point`):
//!
//! * `k_sem_give` / `k_sem_take`
//! * `k_timer_expiry`
//! * `probe_point_enter` / `probe_point_exit`
//! * everything else is `Custom`.

use crate::ctf::CtfEvent;

/// Coarse classification of a [`CtfEvent`] for downstream timing
/// extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZephyrEventClass {
    /// `k_sem_give(sem=..., count=...)`.
    SemGive { sem: String, count: Option<u64> },
    /// `k_sem_take(sem=..., timeout=...)`.
    SemTake {
        sem: String,
        timeout: Option<String>,
    },
    /// `k_timer_expiry(timer=...)`.
    TimerExpiry { timer: String },
    /// `probe_point_enter(probe_id=...)`.
    ProbePointEnter { probe_id: String },
    /// `probe_point_exit(probe_id=...)`.
    ProbePointExit { probe_id: String },
    /// Any other event — preserved by name for higher tiers.
    Custom { name: String },
}

/// Classify a [`CtfEvent`] into a [`ZephyrEventClass`].
///
/// Unknown event names are returned as [`ZephyrEventClass::Custom`].
/// Known events with missing required args fall back to `Custom` rather
/// than panic — Tier 1 is best-effort, not a strict validator.
pub fn classify_event(event: &CtfEvent) -> ZephyrEventClass {
    match event.event_name.as_str() {
        "k_sem_give" => match event.args.get("sem") {
            Some(sem) => ZephyrEventClass::SemGive {
                sem: sem.clone(),
                count: event.args.get("count").and_then(|c| c.parse().ok()),
            },
            None => ZephyrEventClass::Custom {
                name: event.event_name.clone(),
            },
        },
        "k_sem_take" => match event.args.get("sem") {
            Some(sem) => ZephyrEventClass::SemTake {
                sem: sem.clone(),
                timeout: event.args.get("timeout").cloned(),
            },
            None => ZephyrEventClass::Custom {
                name: event.event_name.clone(),
            },
        },
        "k_timer_expiry" => match event.args.get("timer") {
            Some(timer) => ZephyrEventClass::TimerExpiry {
                timer: timer.clone(),
            },
            None => ZephyrEventClass::Custom {
                name: event.event_name.clone(),
            },
        },
        "probe_point_enter" => match event.args.get("probe_id") {
            Some(id) => ZephyrEventClass::ProbePointEnter {
                probe_id: id.clone(),
            },
            None => ZephyrEventClass::Custom {
                name: event.event_name.clone(),
            },
        },
        "probe_point_exit" => match event.args.get("probe_id") {
            Some(id) => ZephyrEventClass::ProbePointExit {
                probe_id: id.clone(),
            },
            None => ZephyrEventClass::Custom {
                name: event.event_name.clone(),
            },
        },
        _ => ZephyrEventClass::Custom {
            name: event.event_name.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctf::parse_ctf;

    #[test]
    fn classify_zephyr_sem_give() {
        let evs = parse_ctf("1: k_sem_give(sem=0x1, count=2)").unwrap();
        assert_eq!(
            classify_event(&evs[0]),
            ZephyrEventClass::SemGive {
                sem: "0x1".into(),
                count: Some(2)
            }
        );
    }

    #[test]
    fn classify_zephyr_sem_take() {
        let evs = parse_ctf("1: k_sem_take(sem=0x1, timeout=K_FOREVER)").unwrap();
        assert_eq!(
            classify_event(&evs[0]),
            ZephyrEventClass::SemTake {
                sem: "0x1".into(),
                timeout: Some("K_FOREVER".into()),
            }
        );
    }

    #[test]
    fn classify_zephyr_probe_point_enter() {
        let evs = parse_ctf("1: probe_point_enter(probe_id=\"Handler.brake\")").unwrap();
        assert_eq!(
            classify_event(&evs[0]),
            ZephyrEventClass::ProbePointEnter {
                probe_id: "Handler.brake".into()
            }
        );
    }

    #[test]
    fn classify_unknown_falls_back_to_custom() {
        let evs = parse_ctf("1: my_app_event(x=1)").unwrap();
        assert_eq!(
            classify_event(&evs[0]),
            ZephyrEventClass::Custom {
                name: "my_app_event".into()
            }
        );
    }
}
