mod assertion;
mod diff;
mod lsp;
mod refactor;
mod sarif;
mod verify;

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
        "allocate" => cmd_allocate(&args[2..]),
        "diff" => cmd_diff(&args[2..]),
        "modes" => cmd_modes(&args[2..]),
        "render" => cmd_render(&args[2..]),
        "verify" => cmd_verify(&args[2..]),
        "codegen" => cmd_codegen(&args[2..]),
        "sysml2" => cmd_sysml2(&args[2..]),
        "extract" => cmd_sysml2_extract(&args[2..]),
        "generate" => cmd_sysml2_generate(&args[2..]),
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
    eprintln!("  allocate   Allocate threads to processors via bin-packing");
    eprintln!("  diff       Compare two model versions and report changes");
    eprintln!("  modes      Mode reachability analysis and SMV/DOT export");
    eprintln!("  render     Render architecture SVG from an instantiated system");
    eprintln!("  verify     Verify requirements against analysis results");
    eprintln!("  codegen    Generate code from an instantiated system model");
    eprintln!("  lsp        Start Language Server Protocol server (stdin/stdout)");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  parse    [--tree] <file...>");
    eprintln!("  items    [--format text|json] <file...>");
    eprintln!("  instance --root Package::Type.Impl [--format text|json] [--analyze] <file...>");
    eprintln!("  analyze  --root Package::Type.Impl [--format text|json|sarif] <file...>");
    eprintln!(
        "  allocate --root Package::Type.Impl [--strategy ffd|bfd] [--format text|json] [--apply] <file...>"
    );
    eprintln!(
        "  diff     --root Package::Type.Impl [--base ref] [--head ref] [--old dir] [--new dir] [--format text|json|sarif] <file...>"
    );
    eprintln!("  modes    --root Package::Type.Impl [--format text|smv|dot] <file...>");
    eprintln!("  render   --root Package::Type.Impl [-o output.svg] <file...>");
    eprintln!(
        "  verify   --root Package::Type.Impl [--format text|json] requirements.toml <file...>"
    );
    eprintln!(
        "  codegen  --root Package::Type.Impl [--output dir] [--format rust|wit|both] [--verify all|build|test|proof] [--rivet] [--dry-run] <file...>"
    );
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
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
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

    // JSON output path: use spar-hir facade for clean serialization
    if format.as_deref() == Some("json") {
        let sources: Vec<_> = files.iter().map(|f| (f.clone(), read_file(f))).collect();
        let hir_db = spar_hir::Database::from_aadl(&sources);
        let instance_tree = hir_db.instantiate(&root).map(|i| i.to_serializable());
        println!("{}", serde_json::to_string_pretty(&instance_tree).unwrap());
        return;
    }

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
    let mut per_som = false;

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
            "--per-som" => {
                per_som = true;
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
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
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
    if per_som {
        diagnostics.extend(run_all_analyses_per_som(&inst));
    } else {
        diagnostics.extend(run_all_analyses(&inst));
    }

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

    // SARIF output path
    if format.as_deref() == Some("sarif") {
        let sarif_output = sarif::to_sarif(&diagnostics, &files);
        println!("{}", serde_json::to_string_pretty(&sarif_output).unwrap());
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

fn cmd_allocate(args: &[String]) {
    let mut root = None;
    let mut strategy = None;
    let mut format = None;
    let mut apply = false;
    let mut files = Vec::new();

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
            "--strategy" => {
                i += 1;
                if i < args.len() {
                    strategy = Some(args[i].clone());
                } else {
                    eprintln!("--strategy requires a value (ffd|bfd)");
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
            "--apply" => {
                apply = true;
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
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
            }
            eprintln!("Cannot allocate: parse errors in {}", file_path);
            process::exit(1);
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    // Build global scope and instantiate
    let scope = spar_hir_def::GlobalScope::from_trees(trees);
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

    // Extract constraints
    let constraints = spar_solver::constraints::ModelConstraints::from_instance(&inst);

    if !constraints.warnings.is_empty() {
        for w in &constraints.warnings {
            eprintln!("warning: {}", w);
        }
        eprintln!();
    }

    // Run allocator
    let result = match strategy.as_deref() {
        Some("bfd") => spar_solver::allocate::Allocator::bfd(&constraints),
        Some("ffd") | None => spar_solver::allocate::Allocator::ffd(&constraints),
        Some(other) => {
            eprintln!("Unknown strategy: {other} (expected ffd or bfd)");
            process::exit(1);
        }
    };

    // Print warnings from allocation
    for w in &result.warnings {
        eprintln!("warning: {}", w);
    }

    // Output results
    match format.as_deref() {
        Some("json") => {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
        _ => {
            // Text table output
            if result.bindings.is_empty() && result.unallocated.is_empty() {
                eprintln!("No threads to allocate.");
            } else {
                println!("{:<30} {:<20} {:>12}", "Thread", "Processor", "Utilization");
                println!("{}", "-".repeat(64));
                for binding in &result.bindings {
                    println!(
                        "{:<30} {:<20} {:>11.4}",
                        binding.thread, binding.processor, binding.utilization
                    );
                }
                if !result.unallocated.is_empty() {
                    println!();
                    println!("Unallocated threads:");
                    for name in &result.unallocated {
                        println!("  {}", name);
                    }
                }
                println!();
                println!("Per-processor utilization:");
                for (name, util) in &result.per_processor_utilization {
                    let bar_len = (*util * 40.0) as usize;
                    let bar: String = "#".repeat(bar_len);
                    println!("  {:<20} {:>6.1}% [{}]", name, util * 100.0, bar);
                }
            }
        }
    }

    // Impact analysis
    let impact = result.impact(&constraints);
    match format.as_deref() {
        Some("json") => {
            // Impact already included if we serialize the full result
        }
        _ => {
            println!();
            println!("Impact analysis:");
            for pi in &impact.processor_utilization {
                let status = if pi.feasible { "OK" } else { "OVERLOADED" };
                println!(
                    "  {:<20} {:>6.1}% util, RMA bound {:>5.1}%, {} threads [{}]",
                    pi.name,
                    pi.utilization * 100.0,
                    pi.rma_bound * 100.0,
                    pi.thread_count,
                    status,
                );
            }
            if !impact.deadline_violations.is_empty() {
                println!();
                println!("Deadline violations:");
                for v in &impact.deadline_violations {
                    println!("  {}", v);
                }
            }
            println!();
            if impact.schedulable {
                println!("Result: \x1b[1;32mSCHEDULABLE\x1b[0m");
            } else {
                println!("Result: \x1b[1;31mNOT SCHEDULABLE\x1b[0m");
            }
        }
    }

    // Apply source rewrites if requested
    if apply {
        if !result.unallocated.is_empty() {
            eprintln!(
                "warning: {} thread(s) could not be allocated; skipping --apply for those",
                result.unallocated.len()
            );
        }

        // Group bindings by source file: we need to figure out which file
        // contains which component implementation. For simplicity, we try
        // each file for each binding edit.
        let mut edits_applied = 0;

        // Detect hierarchical models: if any thread is nested more than one
        // level below the root (i.e., its parent's parent exists), bindings
        // placed on the root implementation may be incorrect.
        let is_hierarchical = constraints.threads.iter().any(|t| {
            let comp = inst.component(t.idx);
            if let Some(parent_idx) = comp.parent {
                inst.component(parent_idx).parent.is_some()
            } else {
                false
            }
        });
        if is_hierarchical {
            eprintln!(
                "warning: --apply places all bindings on the root implementation \
                 ({}.{}). For hierarchical models, manual placement may be needed.",
                type_name, impl_name,
            );
        }

        // Build edits from bindings (only new ones, not pre-existing)
        let binding_edits: Vec<refactor::BindingEdit> = result
            .bindings
            .iter()
            .filter(|b| {
                // Only apply edits for threads that were NOT pre-bound
                constraints
                    .threads
                    .iter()
                    .find(|t| t.name == b.thread)
                    .map(|t| t.current_binding.is_none())
                    .unwrap_or(false)
            })
            .map(|b| {
                // The component_impl is the parent of the thread in the instance hierarchy.
                // For now, use the root implementation since bindings are typically set there.
                let impl_ref = format!("{}.{}", type_name, impl_name);
                refactor::BindingEdit {
                    component_impl: impl_ref,
                    property: "Deployment_Properties::Actual_Processor_Binding".to_string(),
                    value: format!("(reference ({})) applies to {}", b.processor, b.thread),
                }
            })
            .collect();

        for edit in &binding_edits {
            let mut applied = false;
            for file_path in &files {
                let source = read_file(file_path);
                match refactor::apply_binding_edit(&source, edit) {
                    Ok(new_source) => {
                        fs::write(file_path, &new_source).unwrap_or_else(|e| {
                            eprintln!("Cannot write {}: {}", file_path, e);
                            process::exit(1);
                        });
                        eprintln!(
                            "Applied: {} => {} (in {})",
                            edit.property, edit.value, file_path
                        );
                        edits_applied += 1;
                        applied = true;
                        break;
                    }
                    Err(_) => continue, // Try next file
                }
            }
            if !applied {
                eprintln!(
                    "warning: could not apply binding edit for {} in any source file",
                    edit.value
                );
            }
        }

        eprintln!("{} binding edit(s) applied.", edits_applied);
    }

    // Exit with non-zero if there are unallocated threads
    if !result.unallocated.is_empty() {
        process::exit(1);
    }
}

fn cmd_diff(args: &[String]) {
    let mut root = None;
    let mut base_ref = None;
    let mut head_ref = None;
    let mut old_dir = None;
    let mut new_dir = None;
    let mut format = None;
    let mut files = Vec::new();

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
            "--base" => {
                i += 1;
                if i < args.len() {
                    base_ref = Some(args[i].clone());
                } else {
                    eprintln!("--base requires a git ref");
                    process::exit(1);
                }
            }
            "--head" => {
                i += 1;
                if i < args.len() {
                    head_ref = Some(args[i].clone());
                } else {
                    eprintln!("--head requires a git ref");
                    process::exit(1);
                }
            }
            "--old" => {
                i += 1;
                if i < args.len() {
                    old_dir = Some(args[i].clone());
                } else {
                    eprintln!("--old requires a directory path");
                    process::exit(1);
                }
            }
            "--new" => {
                i += 1;
                if i < args.len() {
                    new_dir = Some(args[i].clone());
                } else {
                    eprintln!("--new requires a directory path");
                    process::exit(1);
                }
            }
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                } else {
                    eprintln!("--format requires a value (text|json|sarif)");
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

    // Resolve base and head sources
    let (base_sources, head_sources) = if old_dir.is_some() || new_dir.is_some() {
        // Directory-based comparison
        let old = old_dir.unwrap_or_else(|| {
            eprintln!("--old requires --new (or use --base/--head for git refs)");
            process::exit(1);
        });
        let new = new_dir.unwrap_or_else(|| {
            eprintln!("--new requires --old (or use --base/--head for git refs)");
            process::exit(1);
        });
        (
            diff::resolve_dir_sources(&old),
            diff::resolve_dir_sources(&new),
        )
    } else if base_ref.is_some() {
        // Git ref-based comparison
        if files.is_empty() {
            eprintln!("Missing .aadl file argument(s)");
            process::exit(1);
        }
        let base = base_ref.unwrap();
        let head = head_ref.unwrap_or_else(|| "HEAD".to_string());
        (
            diff::resolve_git_sources(&base, &files),
            diff::resolve_git_sources(&head, &files),
        )
    } else {
        eprintln!("Either --base/--head (git refs) or --old/--new (directories) is required");
        process::exit(1);
    };

    eprintln!("Building base model...");
    let (base_inst, base_diags) = diff::build_model(&base_sources, &root);
    eprintln!(
        "  Base: {} components, {} diagnostics",
        base_inst.component_count(),
        base_diags.len()
    );

    eprintln!("Building head model...");
    let (head_inst, head_diags) = diff::build_model(&head_sources, &root);
    eprintln!(
        "  Head: {} components, {} diagnostics",
        head_inst.component_count(),
        head_diags.len()
    );

    // Compare
    let structural = diff::compare_structure(&base_inst, &head_inst);
    let (analysis_impact, regressions) = diff::compare_diagnostics(&base_diags, &head_diags);

    let result = diff::DiffResult {
        structural,
        analysis_impact,
        regressions,
    };

    // Output
    match format.as_deref() {
        Some("json") => {
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
        }
        Some("sarif") => {
            let all_files: Vec<String> = base_sources
                .files
                .iter()
                .chain(head_sources.files.iter())
                .map(|(f, _)| f.clone())
                .collect();
            let sarif = diff::format_sarif(&result, &all_files);
            println!("{}", serde_json::to_string_pretty(&sarif).unwrap());
        }
        _ => {
            print!("{}", diff::format_text(&result));

            // Exit with non-zero if there are regressions
            if !result.regressions.is_empty() {
                eprintln!("\n{} regression(s) detected.", result.regressions.len());
                process::exit(1);
            }
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
                    eprintln!("--format requires a value (text|smv|dot)");
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
        Some("dot") => {
            print!("{}", spar_analysis::mode_reachability::export_dot(&inst));
        }
        _ => {
            // Text output: show reachability matrices
            let matrices = spar_analysis::mode_reachability::compute_reachability_matrices(&inst);

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
                    println!("Unreachable modes: {}", matrix.unreachable.join(", "));
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

fn cmd_render(args: &[String]) {
    let mut root = None;
    let mut output = None;
    let mut format = None;
    let mut files = Vec::new();

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
                    eprintln!("--format requires a value (svg|html)");
                    process::exit(1);
                }
            }
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                } else {
                    eprintln!("-o requires an output file path");
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

    eprintln!(
        "Rendering {}::{}. ({} components)",
        pkg_name,
        type_name,
        inst.component_count()
    );

    let render_opts = spar_render::RenderOptions {
        interactive: true,
        ..Default::default()
    };

    let content = match format.as_deref() {
        Some("html") => {
            let html_opts = etch::html::HtmlOptions {
                title: format!("{pkg_name}::{type_name}"),
                ..Default::default()
            };
            spar_render::render_instance_html(&inst, &render_opts, &html_opts)
        }
        _ => spar_render::render_instance(&inst, &render_opts),
    };

    match output {
        Some(path) => {
            fs::write(&path, &content).unwrap_or_else(|e| {
                eprintln!("Cannot write {path}: {e}");
                process::exit(1);
            });
            eprintln!("Wrote {}", path);
        }
        None => print!("{content}"),
    }
}

fn cmd_verify(args: &[String]) {
    let mut root = None;
    let mut format = None;
    let mut positional = Vec::new();

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
            s => positional.push(s.to_string()),
        }
        i += 1;
    }

    let root = root.unwrap_or_else(|| {
        eprintln!("--root Package::Type.Impl is required");
        process::exit(1);
    });

    if positional.is_empty() {
        eprintln!("Missing requirements.toml argument");
        process::exit(1);
    }

    let req_path = &positional[0];
    let files = &positional[1..];

    if files.is_empty() {
        eprintln!("Missing AADL file argument(s)");
        process::exit(1);
    }

    // Parse requirements
    let req_file = verify::parse_requirements(req_path);

    if req_file.requirement.is_empty() && req_file.assertion.is_empty() {
        eprintln!("No requirements or assertions found in {req_path}");
        process::exit(0);
    }

    // Parse root reference
    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    // Parse all AADL files and build item trees
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for file_path in files {
        let source = read_file(file_path);
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
            }
            eprintln!("Cannot verify: parse errors in {}", file_path);
            process::exit(1);
        }
        let sf = spar_base_db::SourceFile::new(&db, file_path.clone(), source);
        trees.push(spar_hir_def::file_item_tree(&db, sf));
    }

    // Build global scope and run ItemTree-level checks
    let scope = spar_hir_def::GlobalScope::from_trees(trees.clone());
    let mut diagnostics = Vec::new();

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

    // Run all analyses
    diagnostics.extend(run_all_analyses(&inst));

    // Evaluate requirements against diagnostics
    let mut report = verify::evaluate(&req_file.requirement, &diagnostics, &root);

    // Evaluate assertions against the instance model + diagnostics
    if !req_file.assertion.is_empty() {
        let ctx = assertion::EvalContext {
            instance: &inst,
            diagnostics: &diagnostics,
        };
        let assertion_results = assertion::evaluate_assertions(&req_file.assertion, &ctx);

        // Count assertion pass/fail and merge into report totals
        let assertion_passed = assertion_results
            .iter()
            .filter(|r| r.status == verify::Status::Pass)
            .count();
        let assertion_failed = assertion_results
            .iter()
            .filter(|r| r.status == verify::Status::Fail)
            .count();

        report.total += assertion_results.len();
        report.passed += assertion_passed;
        report.failed += assertion_failed;
        report.assertions = assertion_results;
    }

    // Output
    if format.as_deref() == Some("json") {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        verify::print_text_report(&report);
    }

    if report.failed > 0 {
        process::exit(1);
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

/// Create an AnalysisRunner and run mode-independent analyses once plus
/// mode-dependent analyses per System Operation Mode.
fn run_all_analyses_per_som(
    inst: &spar_hir_def::instance::SystemInstance,
) -> Vec<spar_analysis::AnalysisDiagnostic> {
    let mut runner = spar_analysis::AnalysisRunner::new();
    runner.register_all();
    runner.run_all_per_som(inst)
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

fn cmd_codegen(args: &[String]) {
    let mut root = None;
    let mut output = None;
    let mut format = None;
    let mut verify = None;
    let mut rivet = false;
    let mut dry_run = false;
    let mut files = Vec::new();

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
            "--output" | "-o" => {
                i += 1;
                if i < args.len() {
                    output = Some(args[i].clone());
                } else {
                    eprintln!("--output requires a directory path");
                    process::exit(1);
                }
            }
            "--format" => {
                i += 1;
                if i < args.len() {
                    format = Some(args[i].clone());
                } else {
                    eprintln!("--format requires a value (rust|wit|both)");
                    process::exit(1);
                }
            }
            "--verify" => {
                i += 1;
                if i < args.len() {
                    verify = Some(args[i].clone());
                } else {
                    eprintln!("--verify requires a value (all|build|test|proof)");
                    process::exit(1);
                }
            }
            "--rivet" => rivet = true,
            "--dry-run" => dry_run = true,
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

    let output_format = match format.as_deref() {
        Some("rust") => spar_codegen::OutputFormat::Rust,
        Some("wit") => spar_codegen::OutputFormat::Wit,
        Some("both") => spar_codegen::OutputFormat::Both,
        None => spar_codegen::OutputFormat::Both,
        Some(other) => {
            eprintln!("Unknown format: {other} (expected rust|wit|both)");
            process::exit(1);
        }
    };

    let verify_mode = match verify.as_deref() {
        Some("all") => Some(spar_codegen::VerifyMode::All),
        Some("build") => Some(spar_codegen::VerifyMode::Build),
        Some("test") => Some(spar_codegen::VerifyMode::Test),
        Some("proof") => Some(spar_codegen::VerifyMode::Proof),
        None => None,
        Some(other) => {
            eprintln!("Unknown verify mode: {other} (expected all|build|test|proof)");
            process::exit(1);
        }
    };

    let (pkg_name, type_name, impl_name) = parse_root_ref(&root);

    // Parse all files and build item trees
    let db = spar_hir_def::HirDefDatabase::default();
    let mut trees = Vec::new();

    for file_path in &files {
        let source = read_file(file_path);
        let parsed = spar_syntax::parse(&source);
        if !parsed.ok() {
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
            }
            eprintln!("Cannot codegen: parse errors in {}", file_path);
            process::exit(1);
        }
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

    eprintln!(
        "Generating code for {}::{}. ({} components)",
        pkg_name,
        type_name,
        inst.component_count()
    );

    let output_dir = output.unwrap_or_else(|| "generated".to_string());

    let config = spar_codegen::CodegenConfig {
        root_name: format!("{pkg_name}_{type_name}"),
        output_dir: output_dir.clone(),
        format: output_format,
        verify: verify_mode,
        rivet,
        dry_run,
    };

    let result = spar_codegen::generate(&inst, &config);

    if dry_run {
        eprintln!("Dry run: {} files would be generated", result.files.len());
        for file in &result.files {
            println!("{}/{}", output_dir, file.path);
        }
    } else {
        let mut count = 0;
        for file in &result.files {
            // Validate that the generated file path does not escape the
            // output directory via path traversal (e.g., "../" components).
            if !is_safe_generated_path(&file.path) {
                eprintln!("Refusing to write file with unsafe path: {}", file.path);
                process::exit(1);
            }
            let full_path = format!("{}/{}", output_dir, file.path);
            // Create parent directories
            if let Some(parent) = std::path::Path::new(&full_path).parent() {
                fs::create_dir_all(parent).unwrap_or_else(|e| {
                    eprintln!("Cannot create directory {}: {e}", parent.display());
                    process::exit(1);
                });
            }
            fs::write(&full_path, &file.content).unwrap_or_else(|e| {
                eprintln!("Cannot write {full_path}: {e}");
                process::exit(1);
            });
            count += 1;
        }
        eprintln!("Generated {count} files in {output_dir}/");
    }
}

/// Check that a generated file path is safe to write under the output directory.
///
/// Rejects paths that could escape the output directory:
/// - Paths containing `..` components (directory traversal)
/// - Absolute paths starting with `/` or `\`
fn is_safe_generated_path(path: &str) -> bool {
    !std::path::Path::new(path)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
        && !path.starts_with('/')
        && !path.starts_with('\\')
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
fn cmd_sysml2_parse(args: &[String]) {
    let mut files = Vec::new();
    for arg in args {
        match arg.as_str() {
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
        let parsed = spar_sysml2::parse(&source);

        println!("=== {} ===", file_path);
        println!("{:#?}", parsed.syntax_node());

        if parsed.ok() {
            eprintln!("{}: OK", file_path);
        } else {
            has_errors = true;
            for err in parsed.errors() {
                let (line, col) = spar_base_db::offset_to_line_col(&source, err.offset);
                eprintln!("{}:{}:{}: {}", file_path, line, col, err.msg);
            }
        }
    }

    if has_errors {
        process::exit(1);
    }
}

fn cmd_sysml2_lower(args: &[String]) {
    let mut output_path: Option<String> = None;
    let mut files = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                } else {
                    eprintln!("-o requires a value");
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

    if files.is_empty() {
        eprintln!("Missing file argument");
        process::exit(1);
    }

    // Parse and lower all files (concatenate source)
    let mut all_source = String::new();
    for file_path in &files {
        let source = read_file(file_path);
        if !all_source.is_empty() {
            all_source.push('\n');
        }
        all_source.push_str(&source);
    }

    let parsed = spar_sysml2::parse(&all_source);
    if !parsed.ok() {
        for err in parsed.errors() {
            let (line, col) = spar_base_db::offset_to_line_col(&all_source, err.offset);
            eprintln!("parse error at {}:{}: {}", line, col, err.msg);
        }
        process::exit(1);
    }

    let tree = spar_sysml2::lower::lower_to_aadl(&parsed);
    let aadl = spar_sysml2::lower::item_tree_to_aadl(&tree);

    match output_path {
        Some(path) => {
            fs::write(&path, &aadl).unwrap_or_else(|e| {
                eprintln!("Cannot write {path}: {e}");
                process::exit(1);
            });
            eprintln!("Wrote AADL output to {path}");
        }
        None => {
            print!("{aadl}");
        }
    }
}

fn cmd_sysml2_extract(args: &[String]) {
    let mut output_path: Option<String> = None;
    let mut files = Vec::new();
    let mut include_architecture = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--requirements" => { /* default, kept for backwards compat */ }
            "--include-architecture" | "--arch" => {
                include_architecture = true;
            }
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                } else {
                    eprintln!("-o requires a value");
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

    if files.is_empty() {
        eprintln!("Missing file argument");
        process::exit(1);
    }

    // Parse all files
    let mut all_source = String::new();
    for file_path in &files {
        let source = read_file(file_path);
        if !all_source.is_empty() {
            all_source.push('\n');
        }
        all_source.push_str(&source);
    }

    let parsed = spar_sysml2::parse(&all_source);
    if !parsed.ok() {
        for err in parsed.errors() {
            let (line, col) = spar_base_db::offset_to_line_col(&all_source, err.offset);
            eprintln!("parse error at {}:{}: {}", line, col, err.msg);
        }
        process::exit(1);
    }

    let yaml = if include_architecture {
        spar_sysml2::extract::extract_all_yaml(&parsed, true)
    } else {
        spar_sysml2::extract::extract_requirements(&parsed)
    };

    match output_path {
        Some(path) => {
            fs::write(&path, &yaml).unwrap_or_else(|e| {
                eprintln!("Cannot write {path}: {e}");
                process::exit(1);
            });
            let kind = if include_architecture {
                "requirements + architecture"
            } else {
                "requirements"
            };
            eprintln!("Wrote {kind} YAML to {path}");
        }
        None => {
            print!("{yaml}");
        }
    }
}

fn cmd_sysml2_generate(args: &[String]) {
    let mut output_path: Option<String> = None;
    let mut files = Vec::new();
    let mut from_rivet = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--from-rivet" => {
                from_rivet = true;
            }
            "-o" | "--output" => {
                i += 1;
                if i < args.len() {
                    output_path = Some(args[i].clone());
                } else {
                    eprintln!("-o requires a value");
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

    if !from_rivet {
        eprintln!("Usage: spar sysml2 generate --from-rivet <rivet-yaml-file> [-o output.sysml]");
        process::exit(1);
    }

    if files.is_empty() {
        eprintln!("Missing rivet YAML file argument");
        process::exit(1);
    }

    // Read and parse rivet YAML
    let mut all_yaml = String::new();
    for file_path in &files {
        let source = read_file(file_path);
        all_yaml.push_str(&source);
        all_yaml.push('\n');
    }

    let artifacts = spar_sysml2::generate::parse_rivet_yaml(&all_yaml);
    if artifacts.is_empty() {
        eprintln!("No artifacts found in rivet YAML");
        process::exit(1);
    }

    let sysml = spar_sysml2::generate::generate_sysml2(&artifacts);

    match output_path {
        Some(path) => {
            fs::write(&path, &sysml).unwrap_or_else(|e| {
                eprintln!("Cannot write {path}: {e}");
                process::exit(1);
            });
            eprintln!(
                "Generated SysML v2 from {} artifacts → {path}",
                artifacts.len()
            );
        }
        None => {
            print!("{sysml}");
        }
    }
}

fn cmd_sysml2(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: spar sysml2 <subcommand> [options] <file>");
        eprintln!();
        eprintln!("Subcommands:");
        eprintln!("  parse    Parse a SysML v2 file and show the syntax tree");
        eprintln!("  lower    Lower SysML v2 to AADL");
        eprintln!("  extract  Extract requirements to rivet YAML");
        eprintln!("  generate Generate SysML v2 from rivet YAML (--from-rivet)");
        process::exit(1);
    }

    match args[0].as_str() {
        "parse" => cmd_sysml2_parse(&args[1..]),
        "lower" => cmd_sysml2_lower(&args[1..]),
        "extract" => cmd_sysml2_extract(&args[1..]),
        "generate" => cmd_sysml2_generate(&args[1..]),
        other => {
            eprintln!("Unknown sysml2 subcommand: {other}");
            process::exit(1);
        }
    }
}
