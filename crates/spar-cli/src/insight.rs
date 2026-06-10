//! `spar insight verify-trace` subcommand — Track G v0.9.0.
//!
//! Wraps `spar_insight::analyze` over a parsed AADL model + a textual
//! Zephyr CTF trace and prints the [`DiscrepancyReport`] as JSON
//! (default) or text.

use std::fs;
use std::process;

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::{GlobalScope, HirDefDatabase, Name, file_item_tree};
use spar_insight::{analyze, parse_ctf};

use crate::parse_root_ref;

pub fn cmd_insight(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: spar insight <subcommand> [options] <file...>");
        eprintln!();
        eprintln!("Subcommands:");
        eprintln!(
            "  verify-trace --root Pkg::Type.Impl --trace trace.ctf [--format text|json] <file...>"
        );
        process::exit(1);
    }
    match args[0].as_str() {
        "verify-trace" => cmd_verify_trace(&args[1..]),
        other => {
            eprintln!("Unknown insight subcommand: {other}");
            process::exit(1);
        }
    }
}

fn cmd_verify_trace(args: &[String]) {
    let mut root: Option<String> = None;
    let mut trace_path: Option<String> = None;
    let mut format: Option<String> = None;
    let mut files: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    process::exit(1);
                }
                root = Some(args[i].clone());
            }
            "--trace" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--trace requires a path");
                    process::exit(1);
                }
                trace_path = Some(args[i].clone());
            }
            "--format" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--format requires a value (text|json)");
                    process::exit(1);
                }
                format = Some(args[i].clone());
            }
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                process::exit(1);
            }
            s => files.push(s.to_string()),
        }
        i += 1;
    }

    let root = root.unwrap_or_else(|| {
        eprintln!("--root Package::Type.Impl is required");
        process::exit(1);
    });
    let trace_path = trace_path.unwrap_or_else(|| {
        eprintln!("--trace <path> is required");
        process::exit(1);
    });
    if files.is_empty() {
        eprintln!("Missing AADL file argument(s)");
        process::exit(1);
    }

    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    let db = HirDefDatabase::default();
    let mut trees = Vec::new();
    for file_path in &files {
        let source = fs::read_to_string(file_path).unwrap_or_else(|e| {
            eprintln!("Cannot read {file_path}: {e}");
            process::exit(1);
        });
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{file_path}:{line}:{col}: {}", err.msg);
            }
            eprintln!("Cannot verify-trace: parse errors in {file_path}");
            process::exit(1);
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(file_item_tree(&db, sf));
    }
    let scope = GlobalScope::from_trees(trees);
    let instance = SystemInstance::instantiate(
        &scope,
        &Name::new(&pkg_name),
        &Name::new(&type_name),
        &Name::new(&impl_name),
    );

    let trace_src = fs::read_to_string(&trace_path).unwrap_or_else(|e| {
        eprintln!("Cannot read trace {trace_path}: {e}");
        process::exit(1);
    });
    let events = match parse_ctf(&trace_src) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Cannot parse trace {trace_path}: {e}");
            process::exit(1);
        }
    };

    let report = analyze(&events, &instance);
    let want_text = format.as_deref() == Some("text");
    if want_text {
        print!("{}", report.to_text());
    } else {
        println!("{}", report.to_json());
    }
    if report.has_errors() {
        process::exit(1);
    }
}
