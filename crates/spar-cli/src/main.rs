mod lsp;

use std::{env, fs, process};

use serde::Serialize;

/// Top-level JSON output for `spar analyze --format json`.
#[derive(Serialize)]
struct AnalyzeJsonOutput {
    root: String,
    packages: Vec<spar_hir::Package>,
    instance: Option<spar_hir::InstanceNode>,
    diagnostics: Vec<spar_analysis::AnalysisDiagnostic>,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "parse" => cmd_parse(&args[2..]),
        "items" => cmd_items(&args[2..]),
        "instance" => cmd_instance(&args[2..]),
        "analyze" => cmd_analyze(&args[2..]),
        "modes" => cmd_modes(&args[2..]),
        "lsp" => cmd_lsp(),
        other => {
            eprintln!("Unknown command: {other}");
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("Usage: spar <command> [options] <file...>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  parse      Parse AADL file(s) and report diagnostics");
    eprintln!("  items      Show item tree (declarations) for file(s)");
    eprintln!("  instance   Instantiate a root system implementation");
    eprintln!("  analyze    Run analyses on an instantiated system model");
    eprintln!("  modes      Mode reachability analysis and SMV export");
    eprintln!("  lsp        Start Language Server Protocol server (stdin/stdout)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  parse    [--tree] <file...>");
    eprintln!("  items    [--format text|json] <file...>");
    eprintln!("  instance --root Package::Type.Impl [--analyze] <file...>");
    eprintln!("  analyze  --root Package::Type.Impl [--format text|json] <file...>");
    eprintln!("  modes    --root Package::Type.Impl [--format text|smv] <file...>");
}

fn cmd_lsp() {
    lsp::run_lsp_server();
}

fn cmd_parse(args: &[String]) {
    let mut show_tree = false;
    let mut files = Vec::new();

    for arg in args {
        match arg.as_str() {
            "--tree" => show_tree = true,
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                process::exit(1);
            }
            s => files.push(s.to_string()),
        }
    }

    if files.is_empty() {
        eprintln!("Missing file argument");
        process::exit(1);
    }

    let mut has_errors = false;
    for file_path in &files {
        let source = read_file(file_path);
        let parsed = spar_syntax::parse(&source);

        if show_tree {
            println!("=== {} ===", file_path);
            println!("{:#?}", parsed.syntax_node());
        }

        if parsed.ok() {
            eprintln!("{}: OK", file_path);
        } else {
            has_errors = true;
            for err in parsed.errors() {
                eprintln!("{}:{}: {}", file_path, err.offset, err.msg);
            }
        }
    }

    if has_errors {
        process::exit(1);
    }
}

fn cmd_items(args: &[String]) {
    let mut format = None;
    let mut file_args = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                } else {
                    eprintln!("--format requires a value (text|json)");
                    process::exit(1);
                }
            }
            s if s.starts_with('-') => {
                eprintln!("Unknown option: {s}");
                process::exit(1);
            }
            s => file_args.push(s.to_string()),
        }
        i += 1;
    }

    if file_args.is_empty() {
        eprintln!("Missing file argument(s)");
        process::exit(1);
    }

    if format.as_deref() == Some("json") {
        let sources: Vec<_> = file_args
            .iter()
            .map(|f| (f.clone(), read_file(f)))
            .collect();
        let hir_db = spar_hir::Database::from_aadl(&sources);
        let pkgs = hir_db.packages();
        println!("{}", serde_json::to_string_pretty(&pkgs).unwrap());
        return;
    }

    // Existing text output path
    let db = spar_hir_def::HirDefDatabase::default();
    for file_path in &file_args {
        let source = read_file(file_path);
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        let tree = spar_hir_def::file_item_tree(&db, sf);

        println!("=== {} ===", file_path);

        for (_idx, pkg) in tree.packages.iter() {
            println!("  package {}", pkg.name);
            println!(
                "    with: {:?}",
                pkg.with_clauses
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
            );
            print_items("    public", &pkg.public_items, &tree);
            print_items("    private", &pkg.private_items, &tree);
        }

        for (_idx, ps) in tree.property_sets.iter() {
            println!("  property set {}", ps.name);
            for d in &ps.property_defs {
                println!("    property {}", d.name);
            }
            for c in &ps.property_constants {
                println!("    constant {}", c.name);
            }
        }
    }
}

fn print_items(
    prefix: &str,
    items: &[spar_hir_def::item_tree::ItemRef],
    tree: &spar_hir_def::ItemTree,
) {
    use spar_hir_def::item_tree::ItemRef;

    for item in items {
        match item {
            ItemRef::ComponentType(idx) => {
                let ct = &tree.component_types[*idx];
                println!("{}: {} type {}", prefix, ct.category, ct.name);
                for &fi in &ct.features {
                    let f = &tree.features[fi];
                    let dir = f.direction.map(|d| format!("{:?} ", d)).unwrap_or_default();
                    let cls = f
                        .classifier
                        .as_ref()
                        .map(|c| format!(" {}", c))
                        .unwrap_or_default();
                    println!(
                        "{}  feature {} : {}{:?}{}",
                        prefix, f.name, dir, f.kind, cls
                    );
                }
                for &fsi in &ct.flow_specs {
                    let fs = &tree.flow_specs[fsi];
                    println!("{}  flow {} : {:?}", prefix, fs.name, fs.kind);
                }
            }
            ItemRef::ComponentImpl(idx) => {
                let ci = &tree.component_impls[*idx];
                println!(
                    "{}: {} implementation {}.{}",
                    prefix, ci.category, ci.type_name, ci.impl_name
                );
                for &si in &ci.subcomponents {
                    let s = &tree.subcomponents[si];
                    let cls = s
                        .classifier
                        .as_ref()
                        .map(|c| format!(" {}", c))
                        .unwrap_or_default();
                    println!(
                        "{}  subcomponent {} : {}{}",
                        prefix, s.name, s.category, cls
                    );
                }
                for &coni in &ci.connections {
                    let c = &tree.connections[coni];
                    let arrow = if c.is_bidirectional { "<->" } else { "->" };
                    println!("{}  connection {} : {:?} {}", prefix, c.name, c.kind, arrow);
                }
            }
            ItemRef::FeatureGroupType(idx) => {
                let fgt = &tree.feature_group_types[*idx];
                println!("{}: feature group type {}", prefix, fgt.name);
            }
            ItemRef::PropertySet(idx) => {
                let ps = &tree.property_sets[*idx];
                println!("{}: property set {}", prefix, ps.name);
            }
            ItemRef::AnnexLibrary => {
                println!("{}: annex library", prefix);
            }
        }
    }
}

fn cmd_instance(args: &[String]) {
    let mut root = None;
    let mut files = Vec::new();
    let mut run_analysis = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i < args.len() {
                    root = Some(args[i].clone());
                } else {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    process::exit(1);
                }
            }
            "--analyze" => {
                run_analysis = true;
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

    // Parse root reference: Package::Type.Impl
    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for file_path in &files {
        let source = read_file(file_path);
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    let scope = spar_hir_def::GlobalScope::from_trees(trees);

    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );

    // Print instance hierarchy
    println!("Instance model: {}::{}.", pkg_name, type_name);
    println!("{} component instances", inst.component_count());
    println!();
    print_instance_tree(&inst, inst.root, 0);

    // Print instantiation diagnostics
    if !inst.diagnostics.is_empty() {
        eprintln!();
        for diag in &inst.diagnostics {
            let path: Vec<_> = diag.path.iter().map(|n| n.as_str()).collect();
            eprintln!("warning: {} (at {})", diag.message, path.join("/"));
        }
    }

    // Run analysis if requested
    if run_analysis {
        eprintln!();
        let diagnostics = run_all_analyses(&inst);
        let has_errors = print_diagnostics(&diagnostics);
        if has_errors {
            process::exit(1);
        }
    }
}

fn cmd_analyze(args: &[String]) {
    let mut root = None;
    let mut files = Vec::new();
    let mut format = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i < args.len() {
                    root = Some(args[i].clone());
                } else {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    process::exit(1);
                }
            }
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                } else {
                    eprintln!("--format requires a value (text|json)");
                    process::exit(1);
                }
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

    if files.is_empty() {
        eprintln!("Missing file argument(s)");
        process::exit(1);
    }

    // Parse root reference: Package::Type.Impl
    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    // Parse all files and build item trees
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for file_path in &files {
        let source = read_file(file_path);
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            for err in parsed.errors() {
                eprintln!("{}:{}: {}", file_path, err.offset, err.msg);
            }
            eprintln!("Cannot analyze: parse errors in {}", file_path);
            process::exit(1);
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    // Build global scope and run ItemTree-level checks
    let scope = spar_hir_def::GlobalScope::from_trees(trees.clone());
    let mut diagnostics = Vec::new();

    // Run declarative model checks on each ItemTree
    for tree in &trees {
        diagnostics.extend(spar_analysis::naming_rules::check_naming_rules(tree));
        diagnostics.extend(spar_analysis::category_check::check_category_rules(tree));
        diagnostics.extend(spar_analysis::extends_rules::check_extends_rules(tree));
    }

    // Instantiate root system
    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );

    eprintln!(
        "Instantiated {}::{}. ({} components)",
        pkg_name,
        type_name,
        inst.component_count()
    );
    eprintln!();

    // Run instance-level analyses
    diagnostics.extend(run_all_analyses(&inst));

    // JSON output path
    if format.as_deref() == Some("json") {
        // Build HIR database for package data
        let sources: Vec<_> = files.iter().map(|f| (f.clone(), read_file(f))).collect();
        let hir_db = spar_hir::Database::from_aadl(&sources);
        let instance_tree = hir_db.instantiate(&root).map(|i| i.to_serializable());
        let output = AnalyzeJsonOutput {
            root: root.clone(),
            packages: hir_db.packages(),
            instance: instance_tree,
            diagnostics: diagnostics.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
        return;
    }

    if diagnostics.is_empty() {
        eprintln!("No diagnostics. Model is clean.");
    } else {
        let has_errors = print_diagnostics(&diagnostics);
        if has_errors {
            process::exit(1);
        }
    }
}

fn cmd_modes(args: &[String]) {
    let mut root = None;
    let mut files = Vec::new();
    let mut format = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                if i < args.len() {
                    root = Some(args[i].clone());
                } else {
                    eprintln!("--root requires a value (Package::Type.Impl)");
                    process::exit(1);
                }
            }
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                } else {
                    eprintln!("--format requires a value (text|smv)");
                    process::exit(1);
                }
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

    if files.is_empty() {
        eprintln!("Missing file argument(s)");
        process::exit(1);
    }

    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for file_path in &files {
        let source = read_file(file_path);
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    let scope = spar_hir_def::GlobalScope::from_trees(trees);
    let inst = spar_hir_def::instance::SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg_name),
        &spar_hir_def::Name::new(&type_name),
        &spar_hir_def::Name::new(&impl_name),
    );

    match format.as_deref() {
        Some("smv") => {
            print!("{}", spar_analysis::mode_reachability::export_smv(&inst));
        }
        _ => {
            // Text output: show reachability matrices
            let matrices =
                spar_analysis::mode_reachability::compute_reachability_matrices(&inst);

            if matrices.is_empty() {
                eprintln!("No modal components found.");
                return;
            }

            for matrix in &matrices {
                println!(
                    "Component: {} (initial: {})",
                    matrix.component_path.join("/"),
                    matrix.initial_mode
                );
                println!("Modes: {}", matrix.modes.join(", "));
                println!();

                // Print matrix header
                let max_len = matrix.modes.iter().map(|m| m.len()).max().unwrap_or(4);
                print!("{:width$} |", "", width = max_len);
                for m in &matrix.modes {
                    print!(" {:^width$} |", m, width = max_len);
                }
                println!();
                println!(
                    "{}+{}",
                    "-".repeat(max_len + 1),
                    ("-".repeat(max_len + 3) + "+").repeat(matrix.modes.len())
                );

                for (i, src) in matrix.modes.iter().enumerate() {
                    print!("{:width$} |", src, width = max_len);
                    for j in 0..matrix.modes.len() {
                        let mark = if i == j {
                            "."
                        } else if matrix.matrix[i][j] {
                            "Y"
                        } else {
                            "-"
                        };
                        print!(" {:^width$} |", mark, width = max_len);
                    }
                    println!();
                }
                println!();

                if !matrix.unreachable.is_empty() {
                    println!(
                        "Unreachable modes: {}",
                        matrix.unreachable.join(", ")
                    );
                }
                if !matrix.dead_transitions.is_empty() {
                    println!("Dead transitions:");
                    for dt in &matrix.dead_transitions {
                        println!(
                            "  {} ({} -> {}): trigger '{}' not connected",
                            dt.name, dt.source, dt.destination, dt.trigger
                        );
                    }
                }
                println!();
            }
        }
    }
}

// ── Analysis helpers ────────────────────────────────────────────────

/// Create an AnalysisRunner with all built-in analyses and run them.
fn run_all_analyses(
    inst: &spar_hir_def::instance::SystemInstance,
) -> Vec<spar_analysis::AnalysisDiagnostic> {
    let mut runner = spar_analysis::AnalysisRunner::new();
    runner.register_all();
    runner.run_all(inst)
}

/// Print diagnostics grouped by severity with colored output.
/// Returns `true` if any errors were found.
fn print_diagnostics(diagnostics: &[spar_analysis::AnalysisDiagnostic]) -> bool {
    use spar_analysis::Severity;

    // Group by severity: errors first, then warnings, then info
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut infos = Vec::new();

    for diag in diagnostics {
        match diag.severity {
            Severity::Error => errors.push(diag),
            Severity::Warning => warnings.push(diag),
            Severity::Info => infos.push(diag),
        }
    }

    // Print errors
    for diag in &errors {
        let path_str = diag.path.join("/");
        eprintln!(
            "\x1b[1;31m[ERROR]\x1b[0m   {}: {} \x1b[2m(at {})\x1b[0m",
            diag.analysis, diag.message, path_str
        );
    }

    // Print warnings
    for diag in &warnings {
        let path_str = diag.path.join("/");
        eprintln!(
            "\x1b[1;33m[WARNING]\x1b[0m {}: {} \x1b[2m(at {})\x1b[0m",
            diag.analysis, diag.message, path_str
        );
    }

    // Print infos
    for diag in &infos {
        let path_str = diag.path.join("/");
        eprintln!(
            "\x1b[1;34m[INFO]\x1b[0m    {}: {} \x1b[2m(at {})\x1b[0m",
            diag.analysis, diag.message, path_str
        );
    }

    // Summary
    eprintln!();
    eprintln!(
        "Analysis complete: {} error(s), {} warning(s), {} info(s)",
        errors.len(),
        warnings.len(),
        infos.len()
    );

    !errors.is_empty()
}

// ── Instance tree printing ──────────────────────────────────────────

fn print_instance_tree(
    inst: &spar_hir_def::instance::SystemInstance,
    idx: spar_hir_def::instance::ComponentInstanceIdx,
    indent: usize,
) {
    let comp = inst.component(idx);
    let pad = " ".repeat(indent);
    let impl_suffix = comp
        .impl_name
        .as_ref()
        .map(|i| format!(".{}", i))
        .unwrap_or_default();
    println!(
        "{}{} : {} {}::{}{}",
        pad, comp.name, comp.category, comp.package, comp.type_name, impl_suffix
    );

    for &fi in &comp.features {
        let f = &inst.features[fi];
        let dir = f.direction.map(|d| format!("{:?} ", d)).unwrap_or_default();
        println!("{}  feature {} : {}{:?}", pad, f.name, dir, f.kind);
    }

    for &ci in &comp.connections {
        let c = &inst.connections[ci];
        let arrow = if c.is_bidirectional { "<->" } else { "->" };
        println!("{}  connection {} : {:?} {}", pad, c.name, c.kind, arrow);
    }

    for &child in &comp.children {
        print_instance_tree(inst, child, indent + 2);
    }
}

fn parse_root_ref(s: &str) -> (String, String, String) {
    // Expected format: Package::Type.Impl
    let parts: Vec<&str> = s.splitn(2, "::").collect();
    if parts.len() != 2 {
        eprintln!("Invalid root reference: expected Package::Type.Impl, got: {s}");
        process::exit(1);
    }
    let pkg = parts[0].to_string();
    let type_impl: Vec<&str> = parts[1].splitn(2, '.').collect();
    if type_impl.len() != 2 {
        eprintln!("Invalid root reference: expected Package::Type.Impl, got: {s}");
        process::exit(1);
    }
    (pkg, type_impl[0].to_string(), type_impl[1].to_string())
}

fn read_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Cannot read {path}: {e}");
        process::exit(1);
    })
}
