use std::{env, fs, process};

use spar_syntax::parse;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: spar <command> [options] <file>");
        eprintln!();
        eprintln!("Commands:");
        eprintln!("  parse    Parse an AADL file and report diagnostics");
        process::exit(1);
    }

    match args[1].as_str() {
        "parse" => cmd_parse(&args[2..]),
        other => {
            eprintln!("Unknown command: {other}");
            process::exit(1);
        }
    }
}

fn cmd_parse(args: &[String]) {
    let mut show_tree = false;
    let mut file_path = None;

    for arg in args {
        match arg.as_str() {
            "--tree" => show_tree = true,
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                process::exit(1);
            }
            s => file_path = Some(s.to_string()),
        }
    }

    let file_path = file_path.unwrap_or_else(|| {
        eprintln!("Missing file argument");
        process::exit(1);
    });

    let source = fs::read_to_string(&file_path).unwrap_or_else(|e| {
        eprintln!("Cannot read {file_path}: {e}");
        process::exit(1);
    });

    let parsed = parse(&source);

    if show_tree {
        println!("{:#?}", parsed.syntax_node());
    }

    if parsed.ok() {
        eprintln!("OK: no errors");
    } else {
        for err in parsed.errors() {
            eprintln!("error at offset {}: {}", err.offset, err.msg);
        }
        process::exit(1);
    }
}
