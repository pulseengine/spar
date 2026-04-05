//! Code generation from AADL instance models.
//!
//! This crate generates implementation artifacts from a fully instantiated
//! AADL system model:
//!
//! - **wit_gen**: WIT interface definitions per process
//! - **rust_gen**: Rust component skeletons per thread
//! - **config_gen**: TOML/JSON configuration files
//! - **test_gen**: Test harnesses per thread
//! - **proof_gen**: Lean4 scheduling proofs + Kani harnesses
//! - **doc_gen**: Rivet design documents with YAML frontmatter
//! - **workspace_gen**: Cargo.toml + BUILD.bazel workspace generation

pub mod config_gen;
pub mod doc_gen;
pub mod proof_gen;
pub mod rust_gen;
pub mod test_gen;
pub mod wit_gen;
pub mod workspace_gen;

use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::ComponentCategory;

/// Which output format(s) to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Rust,
    Wit,
    Both,
}

/// Which verification artifacts to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    All,
    Build,
    Test,
    Proof,
}

/// Configuration for code generation.
#[derive(Debug, Clone)]
pub struct CodegenConfig {
    /// Root component name (for naming the generated workspace).
    pub root_name: String,
    /// Output directory path.
    pub output_dir: String,
    /// Which format(s) to generate.
    pub format: OutputFormat,
    /// Which verification artifacts to generate.
    pub verify: Option<VerifyMode>,
    /// Whether to generate rivet design documents.
    pub rivet: bool,
    /// Dry-run mode: collect output but do not write files.
    pub dry_run: bool,
}

/// A single generated file.
#[derive(Debug, Clone)]
pub struct GeneratedFile {
    /// Relative path within the output directory.
    pub path: String,
    /// File content.
    pub content: String,
}

/// Result of code generation.
#[derive(Debug, Clone)]
pub struct CodegenOutput {
    /// All generated files.
    pub files: Vec<GeneratedFile>,
}

/// Extract timing properties from a component instance's property map.
///
/// Returns (period_ps, deadline_ps, wcet_ps) in picoseconds, or None for each
/// property that is not set.
pub fn extract_timing(
    inst: &SystemInstance,
    idx: ComponentInstanceIdx,
) -> (Option<u64>, Option<u64>, Option<u64>) {
    let props = inst.properties_for(idx);

    let period = props
        .get("Timing_Properties", "Period")
        .or_else(|| props.get("", "Period"))
        .and_then(parse_time_to_ps);

    let deadline = props
        .get("Timing_Properties", "Deadline")
        .or_else(|| props.get("", "Deadline"))
        .and_then(parse_time_to_ps);

    let wcet = props
        .get("Timing_Properties", "Compute_Execution_Time")
        .or_else(|| props.get("", "Compute_Execution_Time"))
        .and_then(parse_time_range_max_to_ps);

    (period, deadline, wcet)
}

/// Parse a time value string (e.g., "10 ms", "500 us") to picoseconds.
fn parse_time_to_ps(s: &str) -> Option<u64> {
    let s = s.trim();
    // Try to parse "number unit" format
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() == 2 {
        let num: f64 = parts[0].parse().ok()?;
        let multiplier = time_unit_to_ps(parts[1])?;
        Some((num * multiplier as f64) as u64)
    } else {
        // Try bare number (assume ms)
        let num: f64 = s.parse().ok()?;
        Some((num * 1_000_000_000.0) as u64) // default ms
    }
}

/// Parse a time range value, extracting the max (WCET).
/// Format: "min .. max" or just a single value.
fn parse_time_range_max_to_ps(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some((_min, max)) = s.split_once("..") {
        parse_time_to_ps(max.trim())
    } else {
        parse_time_to_ps(s)
    }
}

/// Convert a time unit string to its value in picoseconds.
fn time_unit_to_ps(unit: &str) -> Option<u64> {
    match unit.to_lowercase().as_str() {
        "ps" => Some(1),
        "ns" => Some(1_000),
        "us" | "\u{00b5}s" => Some(1_000_000),
        "ms" => Some(1_000_000_000),
        "sec" | "s" => Some(1_000_000_000_000),
        "min" => Some(60 * 1_000_000_000_000),
        "hr" => Some(3600 * 1_000_000_000_000),
        _ => None,
    }
}

/// Format picoseconds as a human-readable time string.
fn format_time_ps(ps: u64) -> String {
    if ps >= 1_000_000_000_000 {
        format!("{} sec", ps / 1_000_000_000_000)
    } else if ps >= 1_000_000_000 {
        format!("{} ms", ps / 1_000_000_000)
    } else if ps >= 1_000_000 {
        format!("{} us", ps / 1_000_000)
    } else if ps >= 1_000 {
        format!("{} ns", ps / 1_000)
    } else {
        format!("{} ps", ps)
    }
}

/// Maximum length for generated identifiers.
///
/// Prevents excessively long identifiers from propagating into generated
/// Rust, WIT, Lean4, TOML, and Bazel files.
const MAX_IDENT_LEN: usize = 64;

/// Rust keywords (2021 edition + `async`/`await`/`dyn`).
///
/// If a sanitized identifier matches one of these, we suffix it with `_`
/// so the generated code remains valid Rust.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

/// Sanitize a name for use as a Rust/WIT/Lean identifier.
///
/// Applies the following safety checks in order:
/// 1. Replace non-alphanumeric/underscore characters with `_`
/// 2. Lowercase the result
/// 3. Trim leading/trailing underscores
/// 4. If the result is empty, return `"unnamed"`
/// 5. If the result starts with a digit, prefix with `_`
/// 6. If the result is a Rust keyword, suffix with `_`
/// 7. Truncate to [`MAX_IDENT_LEN`] characters
fn sanitize_ident(name: &str) -> String {
    let mut s = name
        .replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
        .trim_matches('_')
        .to_lowercase();

    // Empty after sanitization (e.g. input was "..." or "")
    if s.is_empty() {
        return "unnamed".to_string();
    }

    // Identifiers must not start with a digit
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }

    // Escape Rust keywords by appending `_`
    if RUST_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }

    // Truncate to maximum length
    if s.len() > MAX_IDENT_LEN {
        s.truncate(MAX_IDENT_LEN);
        // Ensure we don't end mid-character (all chars are ASCII after lowercasing,
        // but guard anyway)
        while !s.is_char_boundary(s.len()) {
            s.pop();
        }
    }

    s
}

/// Convert a snake/kebab/dot name to PascalCase.
///
/// Applies the same safety invariants as [`sanitize_ident`]: empty names
/// become `Unnamed`, digit-leading names are prefixed with `_`, Rust
/// keywords are suffixed with `_`, and length is capped at [`MAX_IDENT_LEN`].
fn to_pascal_case(s: &str) -> String {
    let mut result: String = s
        .split(['_', '-', '.'])
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
            }
        })
        .collect();

    // Empty after conversion
    if result.is_empty() {
        return "Unnamed".to_string();
    }

    // PascalCase identifiers must not start with a digit
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    // Keyword check (PascalCase rarely collides, but `Self` is a keyword)
    if RUST_KEYWORDS.contains(&result.as_str()) {
        result.push('_');
    }

    // Truncate to maximum length
    if result.len() > MAX_IDENT_LEN {
        result.truncate(MAX_IDENT_LEN);
        while !result.is_char_boundary(result.len()) {
            result.pop();
        }
    }

    result
}

/// Collect all thread instances bound to a given processor.
///
/// Looks at `Deployment_Properties::Actual_Processor_Binding` on each thread
/// to see if it references the given processor. If no binding properties exist,
/// falls back to collecting all threads that are children of processes under
/// the processor's parent system.
fn threads_for_processor(
    inst: &SystemInstance,
    proc_idx: ComponentInstanceIdx,
) -> Vec<ComponentInstanceIdx> {
    let proc_comp = inst.component(proc_idx);
    let proc_name = proc_comp.name.as_str();

    let mut bound_threads = Vec::new();

    for (idx, comp) in inst.all_components() {
        if comp.category != ComponentCategory::Thread {
            continue;
        }
        let props = inst.properties_for(idx);
        if let Some(binding) = props
            .get("Deployment_Properties", "Actual_Processor_Binding")
            .or_else(|| props.get("", "Actual_Processor_Binding"))
        {
            // Extract the reference target and compare the last component
            // against the processor name (case-insensitive). Using
            // extract_reference_target avoids substring false-positives
            // (e.g. "CPU" matching "CPU_hot_spare").
            if let Some(target) =
                spar_analysis::property_accessors::extract_reference_target(binding)
            {
                // The target may be a qualified path like "parent.cpu"; compare
                // just the last segment against the processor name.
                let last_segment = target.rsplit('.').next().unwrap_or(target);
                if last_segment.eq_ignore_ascii_case(proc_name) {
                    bound_threads.push(idx);
                }
            } else if binding.eq_ignore_ascii_case(proc_name) {
                // Fallback: bare name without reference(...) wrapper
                bound_threads.push(idx);
            }
        }
    }

    // If no explicit bindings found, collect all threads as a fallback
    if bound_threads.is_empty() {
        for (idx, comp) in inst.all_components() {
            if comp.category == ComponentCategory::Thread {
                bound_threads.push(idx);
            }
        }
    }

    bound_threads
}

/// Generate all code artifacts from an AADL instance model.
pub fn generate(inst: &SystemInstance, config: &CodegenConfig) -> CodegenOutput {
    let mut files = Vec::new();

    // Collect processes and threads
    let processes: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Process)
        .collect();

    let threads: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Thread)
        .collect();

    let processors: Vec<_> = inst
        .all_components()
        .filter(|(_, c)| c.category == ComponentCategory::Processor)
        .collect();

    // Generate WIT files
    if config.format == OutputFormat::Wit || config.format == OutputFormat::Both {
        for &(idx, _comp) in &processes {
            files.push(wit_gen::generate_wit(inst, idx));
        }
    }

    // Generate Rust files
    if config.format == OutputFormat::Rust || config.format == OutputFormat::Both {
        for &(idx, _comp) in &threads {
            files.push(rust_gen::generate_rust_component(inst, idx));
        }
    }

    // Generate config files
    for &(idx, _comp) in &processes {
        files.push(config_gen::generate_config(inst, idx));
    }

    // Generate test harnesses
    if let Some(verify) = config.verify
        && (verify == VerifyMode::All || verify == VerifyMode::Test)
    {
        for &(idx, _comp) in &threads {
            files.push(test_gen::generate_test_harness(inst, idx));
        }
    }

    // Generate proof artifacts
    if let Some(verify) = config.verify
        && (verify == VerifyMode::All || verify == VerifyMode::Proof)
    {
        for &(proc_idx, _proc_comp) in &processors {
            files.push(proof_gen::generate_lean4_proof(inst, proc_idx));
            let thread_idxs = threads_for_processor(inst, proc_idx);
            for t_idx in &thread_idxs {
                files.push(proof_gen::generate_kani_harness(inst, *t_idx));
            }
        }
        // If no explicit processors, still generate per-thread Kani harnesses
        if processors.is_empty() {
            for &(idx, _comp) in &threads {
                files.push(proof_gen::generate_kani_harness(inst, idx));
            }
        }
    }

    // Generate rivet design docs
    if config.rivet {
        for &(idx, _comp) in &processes {
            let (doc, verification) = doc_gen::generate_design_doc(inst, idx);
            files.push(doc);
            files.push(verification);
        }
    }

    // Generate workspace files
    let process_names: Vec<String> = processes
        .iter()
        .map(|(_, c)| sanitize_ident(c.name.as_str()))
        .collect();

    files.extend(workspace_gen::generate_workspace(
        &config.root_name,
        &process_names,
    ));

    CodegenOutput { files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_values() {
        assert_eq!(parse_time_to_ps("10 ms"), Some(10_000_000_000));
        assert_eq!(parse_time_to_ps("500 us"), Some(500_000_000));
        assert_eq!(parse_time_to_ps("1 sec"), Some(1_000_000_000_000));
    }

    #[test]
    fn parse_time_range() {
        assert_eq!(
            parse_time_range_max_to_ps("1 ms .. 5 ms"),
            Some(5_000_000_000)
        );
        assert_eq!(parse_time_range_max_to_ps("10 ms"), Some(10_000_000_000));
    }

    #[test]
    fn format_time_values() {
        assert_eq!(format_time_ps(10_000_000_000), "10 ms");
        assert_eq!(format_time_ps(500_000_000), "500 us");
        assert_eq!(format_time_ps(1_000_000_000_000), "1 sec");
    }

    #[test]
    fn sanitize_ident_basic() {
        assert_eq!(sanitize_ident("Ctrl.Impl"), "ctrl_impl");
        assert_eq!(sanitize_ident("My-Thread"), "my_thread");
        assert_eq!(sanitize_ident("simple"), "simple");
    }

    #[test]
    fn sanitize_ident_empty_result() {
        assert_eq!(sanitize_ident(""), "unnamed");
        assert_eq!(sanitize_ident("..."), "unnamed");
        assert_eq!(sanitize_ident("___"), "unnamed");
        assert_eq!(sanitize_ident("---"), "unnamed");
        assert_eq!(sanitize_ident("@#$"), "unnamed");
    }

    #[test]
    fn sanitize_ident_digit_prefix() {
        assert_eq!(sanitize_ident("123thread"), "_123thread");
        assert_eq!(sanitize_ident("0start"), "_0start");
        assert_eq!(sanitize_ident("9x"), "_9x");
        // Embedded digits are fine
        assert_eq!(sanitize_ident("thread1"), "thread1");
        assert_eq!(sanitize_ident("a2b"), "a2b");
    }

    #[test]
    fn sanitize_ident_rust_keywords() {
        assert_eq!(sanitize_ident("type"), "type_");
        assert_eq!(sanitize_ident("fn"), "fn_");
        assert_eq!(sanitize_ident("struct"), "struct_");
        assert_eq!(sanitize_ident("mod"), "mod_");
        assert_eq!(sanitize_ident("impl"), "impl_");
        assert_eq!(sanitize_ident("self"), "self_");
        assert_eq!(sanitize_ident("async"), "async_");
        assert_eq!(sanitize_ident("await"), "await_");
        assert_eq!(sanitize_ident("dyn"), "dyn_");
        assert_eq!(sanitize_ident("pub"), "pub_");
        assert_eq!(sanitize_ident("use"), "use_");
        assert_eq!(sanitize_ident("let"), "let_");
        assert_eq!(sanitize_ident("match"), "match_");
        assert_eq!(sanitize_ident("return"), "return_");
        // Non-keyword similar names should be unaffected
        assert_eq!(sanitize_ident("types"), "types");
        assert_eq!(sanitize_ident("module"), "module");
    }

    #[test]
    fn sanitize_ident_truncation() {
        let long = "a".repeat(100);
        let result = sanitize_ident(&long);
        assert_eq!(result.len(), MAX_IDENT_LEN);
        assert_eq!(result, "a".repeat(MAX_IDENT_LEN));
    }

    #[test]
    fn sanitize_ident_keyword_after_sanitization() {
        // "TYPE" lowercases to "type" which is a keyword
        assert_eq!(sanitize_ident("TYPE"), "type_");
        // "FN" lowercases to "fn"
        assert_eq!(sanitize_ident("FN"), "fn_");
        // "Struct" lowercases to "struct"
        assert_eq!(sanitize_ident("Struct"), "struct_");
    }

    #[test]
    fn to_pascal_case_basic() {
        assert_eq!(to_pascal_case("ctrl_thread"), "CtrlThread");
        assert_eq!(to_pascal_case("my-component.impl"), "MyComponentImpl");
        assert_eq!(to_pascal_case("Sensor"), "Sensor");
    }

    #[test]
    fn to_pascal_case_empty_result() {
        assert_eq!(to_pascal_case(""), "Unnamed");
        assert_eq!(to_pascal_case("___"), "Unnamed");
        assert_eq!(to_pascal_case("..."), "Unnamed");
        assert_eq!(to_pascal_case("-_-."), "Unnamed");
    }

    #[test]
    fn to_pascal_case_digit_prefix() {
        assert_eq!(to_pascal_case("123_thread"), "_123Thread");
        assert_eq!(to_pascal_case("0start"), "_0start");
    }

    #[test]
    fn to_pascal_case_self_keyword() {
        // "Self" is a Rust keyword
        assert_eq!(to_pascal_case("Self"), "Self_");
        // Other PascalCase results that aren't keywords pass through
        assert_eq!(to_pascal_case("my_type"), "MyType");
    }

    #[test]
    fn to_pascal_case_truncation() {
        let long = (0..40)
            .map(|i| format!("seg{i}"))
            .collect::<Vec<_>>()
            .join("_");
        let result = to_pascal_case(&long);
        assert!(
            result.len() <= MAX_IDENT_LEN,
            "PascalCase result length {} exceeds max {}",
            result.len(),
            MAX_IDENT_LEN,
        );
    }

    #[test]
    fn processor_binding_exact_match_not_substring() {
        // Verify the extract_reference_target + last-segment comparison logic
        // that threads_for_processor uses to avoid substring false-positives.
        use spar_analysis::property_accessors::extract_reference_target;

        let proc_name = "CPU";

        // Should match: reference wraps exactly "CPU"
        let binding = "reference(CPU)";
        let target = extract_reference_target(binding).unwrap();
        let last = target.rsplit('.').next().unwrap_or(target);
        assert!(
            last.eq_ignore_ascii_case(proc_name),
            "reference(CPU) should match processor CPU"
        );

        // Should match: qualified path ending with "cpu"
        let binding = "reference(Top_impl.cpu)";
        let target = extract_reference_target(binding).unwrap();
        let last = target.rsplit('.').next().unwrap_or(target);
        assert!(
            last.eq_ignore_ascii_case(proc_name),
            "reference(Top_impl.cpu) should match processor CPU"
        );

        // Must NOT match: "CPU_hot_spare" is a different processor
        let binding = "reference(CPU_hot_spare)";
        let target = extract_reference_target(binding).unwrap();
        let last = target.rsplit('.').next().unwrap_or(target);
        assert!(
            !last.eq_ignore_ascii_case(proc_name),
            "reference(CPU_hot_spare) must NOT match processor CPU"
        );

        // Must NOT match: "MyCPU" is also different
        let binding = "reference(sys.MyCPU)";
        let target = extract_reference_target(binding).unwrap();
        let last = target.rsplit('.').next().unwrap_or(target);
        assert!(
            !last.eq_ignore_ascii_case(proc_name),
            "reference(sys.MyCPU) must NOT match processor CPU"
        );
    }
}
