use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root = PathBuf::from(".");
    let mut ledger: Option<PathBuf> = None;
    let mut write = false;
    let mut control = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => match it.next() {
                Some(v) => root = PathBuf::from(v),
                None => usage(),
            },
            "--ledger" => match it.next() {
                Some(v) => ledger = Some(PathBuf::from(v)),
                None => usage(),
            },
            "--write" => write = true,
            "--negative-control" => control = true,
            "--version" | "-V" => {
                println!("check-kani-ledger {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--help" | "-h" => {
                println!(
                    "usage: check-kani-ledger [--root DIR] [--ledger FILE] [--write] [--negative-control]"
                );
                return;
            }
            _ => usage(),
        }
    }
    let ledger = ledger.unwrap_or_else(|| root.join(spar_check_kani_ledger::LEDGER));
    let mut out = std::io::stdout();
    if write {
        match spar_check_kani_ledger::scan_tree(&root) {
            Ok((found, _)) => {
                if let Err(e) =
                    std::fs::write(&ledger, spar_check_kani_ledger::render_ledger(&found))
                {
                    eprintln!("cannot write {}: {e}", ledger.display());
                    std::process::exit(2);
                }
                println!("wrote {} lines to {}", found.len(), ledger.display());
                return;
            }
            Err(e) => {
                eprintln!("cannot scan {}: {e}", root.display());
                std::process::exit(2);
            }
        }
    }
    let mut code = spar_check_kani_ledger::run(&root, &ledger, &mut out);
    if control && code != 2 {
        let c = spar_check_kani_ledger::negative_control(&mut out);
        code = code.max(c);
    }
    std::process::exit(code);
}

fn usage() -> ! {
    eprintln!(
        "usage: check-kani-ledger [--root DIR] [--ledger FILE] [--write] [--negative-control]"
    );
    std::process::exit(2);
}
