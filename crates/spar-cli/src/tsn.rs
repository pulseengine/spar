//! `spar gcl` and `spar yang` — the TSN synthesis and export library calls,
//! reachable without writing Rust (REQ-TSN-CLI-SURFACE-001).
//!
//! Both commands are thin: they parse arguments, make ONE `spar-network` call,
//! and print what it returned. No bound, window size or document is computed
//! here.
//!
//! - `gcl` calls [`synthesize_gcl_split_guarded`] on explicitly given
//!   per-class demands and prints [`GateSchedule::to_gcl_blob`]. Demands are
//!   NOT derived from an AADL model: how an end-to-end deadline is split into
//!   per-hop budgets, and how flows aggregate per class per port, is not
//!   decided, and the synthesizer/analyzer mismatch the requirement scopes out
//!   lives exactly there.
//! - `yang` calls [`GateSchedule::to_qcw_yang`] on a gate schedule read from
//!   a bus's `Spar_TSN::Gate_Control_List` in an AADL model, or given as a
//!   blob (so `spar gcl` output can be exported).

use std::process;

use serde::Serialize;
use spar_network::tsn::{
    ClassDemand, GateSchedule, get_gate_control_list_raw, synthesize_gcl_split_guarded,
};
use spar_network::{ArrivalCurve, ClassOfService};

use crate::{parse_root_ref, read_file};

fn print_gcl_usage() {
    eprintln!(
        "Usage: spar gcl --cycle-ns N --link-rate-bps R --max-windows K --guard-ns G \
         --class COS:BURST_BYTES:RATE_BPS:DEADLINE_NS [--class ...] [--format text|json]"
    );
}

fn print_yang_usage() {
    eprintln!("Usage: spar yang --port NAME --gcl BLOB");
    eprintln!("       spar yang --port NAME --root Package::Type.Impl --bus NAME <file...>");
}

/// The value after flag `args[*i]`, advancing `i`; exit 1 when it is missing.
fn take_value(args: &[String], i: &mut usize, usage: fn()) -> String {
    let flag = &args[*i];
    *i += 1;
    match args.get(*i) {
        Some(v) => v.clone(),
        None => {
            eprintln!("{flag} requires a value");
            usage();
            process::exit(1);
        }
    }
}

fn parse_u64(flag: &str, s: &str) -> u64 {
    s.parse::<u64>().unwrap_or_else(|_| {
        eprintln!("{flag}: not a non-negative integer: {s:?}");
        process::exit(1);
    })
}

/// Nanoseconds to picoseconds; exit 1 rather than wrap on overflow.
fn ns_to_ps(flag: &str, ns: u64) -> u64 {
    ns.checked_mul(1_000).unwrap_or_else(|| {
        eprintln!("{flag}: {ns} ns overflows picoseconds");
        process::exit(1);
    })
}

/// `COS:BURST_BYTES:RATE_BPS:DEADLINE_NS` into a [`ClassDemand`] with an
/// affine arrival curve `burst + rate·t` and a per-hop deadline budget.
fn parse_class(s: &str) -> ClassDemand {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 4 {
        eprintln!("--class: expected COS:BURST_BYTES:RATE_BPS:DEADLINE_NS, got {s:?}");
        process::exit(1);
    }
    let cos_raw = parse_u64("--class COS", parts[0]);
    let Some(cos) = u8::try_from(cos_raw).ok().and_then(ClassOfService::new) else {
        eprintln!("--class: class of service must be 0..=7, got {cos_raw}");
        process::exit(1);
    };
    ClassDemand {
        cos,
        arrival: ArrivalCurve::affine(
            parse_u64("--class BURST_BYTES", parts[1]),
            parse_u64("--class RATE_BPS", parts[2]),
        ),
        deadline_ps: ns_to_ps(
            "--class DEADLINE_NS",
            parse_u64("--class DEADLINE_NS", parts[3]),
        ),
    }
}

#[derive(Serialize)]
struct WindowJson {
    offset_ps: u64,
    duration_ps: u64,
    allowed_cos_mask: u8,
}

#[derive(Serialize)]
struct GclJson {
    cycle_ps: u64,
    gcl: String,
    windows: Vec<WindowJson>,
}

/// `spar gcl` — synthesize a guard-band-aware 802.1Qbv gate-control list.
pub fn cmd_gcl(args: &[String]) {
    let mut cycle_ns = None;
    let mut link_rate_bps = None;
    let mut max_windows = None;
    let mut guard_ns = None;
    let mut demands = Vec::new();
    let mut format = String::from("text");

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cycle-ns" => {
                cycle_ns = Some(parse_u64(
                    "--cycle-ns",
                    &take_value(args, &mut i, print_gcl_usage),
                ))
            }
            "--link-rate-bps" => {
                link_rate_bps = Some(parse_u64(
                    "--link-rate-bps",
                    &take_value(args, &mut i, print_gcl_usage),
                ))
            }
            "--max-windows" => {
                max_windows = Some(parse_u64(
                    "--max-windows",
                    &take_value(args, &mut i, print_gcl_usage),
                ))
            }
            "--guard-ns" => {
                guard_ns = Some(parse_u64(
                    "--guard-ns",
                    &take_value(args, &mut i, print_gcl_usage),
                ))
            }
            "--class" => demands.push(parse_class(&take_value(args, &mut i, print_gcl_usage))),
            "--format" => format = take_value(args, &mut i, print_gcl_usage),
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                print_gcl_usage();
                process::exit(2);
            }
            s => {
                eprintln!("Unexpected argument: {s}");
                print_gcl_usage();
                process::exit(2);
            }
        }
        i += 1;
    }

    if format != "text" && format != "json" {
        eprintln!("Unknown --format: {format} (expected text|json)");
        process::exit(1);
    }
    // Every numeric input is required: there is no agreed default for the
    // window cap or the guard band, and a guess would change the schedule.
    let (Some(cycle_ns), Some(link_rate_bps), Some(max_windows), Some(guard_ns)) =
        (cycle_ns, link_rate_bps, max_windows, guard_ns)
    else {
        eprintln!("--cycle-ns, --link-rate-bps, --max-windows and --guard-ns are all required");
        print_gcl_usage();
        process::exit(1);
    };
    if demands.is_empty() {
        eprintln!("at least one --class is required");
        print_gcl_usage();
        process::exit(1);
    }

    let sched = match synthesize_gcl_split_guarded(
        &demands,
        ns_to_ps("--cycle-ns", cycle_ns),
        link_rate_bps,
        max_windows,
        ns_to_ps("--guard-ns", guard_ns),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: no gate-control list: {e}");
            process::exit(1);
        }
    };

    if format == "json" {
        let out = GclJson {
            cycle_ps: sched.cycle_ps,
            gcl: sched.to_gcl_blob(),
            windows: sched
                .windows
                .iter()
                .map(|w| WindowJson {
                    offset_ps: w.offset_ps,
                    duration_ps: w.duration_ps,
                    allowed_cos_mask: w.allowed_cos_mask,
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("{}", sched.to_gcl_blob());
    }
}

/// The `Spar_TSN::Gate_Control_List` of the component named `bus` in the
/// instance of `root`, parsed. Exits 1 on a parse error in any file, a missing
/// or ambiguous component, an absent property, or a malformed list.
fn schedule_from_model(root: &str, bus: &str, files: &[String]) -> GateSchedule {
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();
    for file_path in files {
        let source = read_file(file_path);
        // The same refusal `spar parse` and `spar analyze` make (#552): a
        // model the parser rejects is never silently lowered.
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
            }
            eprintln!("Cannot export: parse errors in {}", file_path);
            process::exit(1);
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }
    let scope = spar_hir_def::GlobalScope::from_trees(trees);
    let (pkg, ty, imp) = parse_root_ref(root);
    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg),
        &spar_hir_def::Name::new(&ty),
        &spar_hir_def::Name::new(&imp),
    );

    let matches: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.name.as_str().eq_ignore_ascii_case(bus))
        .map(|(idx, _)| idx)
        .collect();
    let idx = match matches.as_slice() {
        [idx] => *idx,
        [] => {
            eprintln!("error: no component named '{bus}' in the instance of {root}");
            process::exit(1);
        }
        _ => {
            eprintln!(
                "error: {} components are named '{bus}' in the instance of {root}",
                matches.len()
            );
            process::exit(1);
        }
    };
    // The raw getter, not `get_gate_schedule`: that one maps a malformed list
    // to None, and an export must say why it has nothing to export.
    let Some(raw) = get_gate_control_list_raw(inst.properties_for(idx)) else {
        eprintln!("error: '{bus}' carries no Spar_TSN::Gate_Control_List");
        process::exit(1);
    };
    GateSchedule::parse(&raw).unwrap_or_else(|e| {
        eprintln!("error: '{bus}' Spar_TSN::Gate_Control_List: {e}");
        process::exit(1);
    })
}

/// `spar yang` — export a gate schedule as an 802.1Qcw YANG instance document.
pub fn cmd_yang(args: &[String]) {
    let mut port = None;
    let mut gcl = None;
    let mut root = None;
    let mut bus = None;
    let mut files = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => port = Some(take_value(args, &mut i, print_yang_usage)),
            "--gcl" => gcl = Some(take_value(args, &mut i, print_yang_usage)),
            "--root" => root = Some(take_value(args, &mut i, print_yang_usage)),
            "--bus" => bus = Some(take_value(args, &mut i, print_yang_usage)),
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                print_yang_usage();
                process::exit(2);
            }
            s => files.push(s.to_string()),
        }
        i += 1;
    }

    let Some(port) = port else {
        eprintln!("--port NAME is required (the interface the schedule is written to)");
        print_yang_usage();
        process::exit(1);
    };
    let sched = match (gcl, root, bus) {
        (Some(blob), None, None) if files.is_empty() => {
            GateSchedule::parse(&blob).unwrap_or_else(|e| {
                eprintln!("error: --gcl: {e}");
                process::exit(1);
            })
        }
        (None, Some(root), Some(bus)) if !files.is_empty() => {
            schedule_from_model(&root, &bus, &files)
        }
        _ => {
            eprintln!("give either --gcl BLOB, or --root, --bus and model file(s)");
            print_yang_usage();
            process::exit(1);
        }
    };

    // REQ-TSN-YANG-DEPLOYABLE-001 (#475): the document carries no base-time
    // and no config-change, so a switch has nothing to activate. Say so on
    // stderr every time, so the XML on stdout is not mistaken for deployable.
    eprintln!(
        "warning: this document has no base-time and no config-change; it is not \
         activatable on a switch as emitted (REQ-TSN-YANG-DEPLOYABLE-001, #475)"
    );
    print!("{}", sched.to_qcw_yang(&port));
}
