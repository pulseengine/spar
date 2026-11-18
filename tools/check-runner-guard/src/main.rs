fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = match args.as_slice() {
        [] => ".".to_owned(),
        [flag, dir] if flag == "--root" => dir.clone(),
        _ => {
            eprintln!("usage: check-runner-guard [--root DIR]");
            std::process::exit(2);
        }
    };
    let code = spar_check_runner_guard::run(std::path::Path::new(&root), &mut std::io::stdout());
    std::process::exit(code);
}
