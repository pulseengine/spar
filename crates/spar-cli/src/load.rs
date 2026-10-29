//! The parse gate every model-loading subcommand goes through (#552).
//!
//! ## The defect
//!
//! `spar parse` and `spar analyze` refused a model whose feature was named
//! `out` (an AS-5506B Annex A reserved word) and exited 1. `spar instance`
//! loaded the SAME file, exited 0, and printed an instance model with the
//! feature silently removed. So did `modes`, `emit`, `render` and `diff`. Each
//! subcommand carried its own copy of the read-parse-lower loop, and only some
//! copies checked `parsed.ok()`. A caller that consumed `instance` output (an
//! analysis, a feature count) passed over a model missing the feature.
//!
//! ## The fix
//!
//! One check, used by every command that loads an AADL model. A parse error is
//! fatal: the command prints exactly the lines `spar parse` prints, then
//! `Cannot <verb>: parse errors in <file>`, and exits 1 before it builds
//! anything or writes any output file.
//!
//! This module is the library half: it renders the diagnostic and does not
//! exit, because `moves` lives in the library and `spar-mcp` calls it
//! in-process. The binary's `load_sources_or_exit` wraps it.

/// The parse errors in `source`, rendered exactly as `spar parse` prints them:
/// one `path:line:col: message` line per error, each ending in `\n`.
///
/// `None` when the file parses cleanly.
pub fn parse_error_report(path: &str, source: &str) -> Option<String> {
    let parsed = spar_syntax::parse(source);
    if parsed.ok() {
        return None;
    }
    let mut report = String::new();
    for err in parsed.errors() {
        let (line, col) = spar_base_db::offset_to_line_col(source, err.offset);
        report.push_str(&format!("{path}:{line}:{col}: {}\n", err.msg));
    }
    Some(report)
}

/// The first source that does not parse, with its rendered report.
///
/// Sources are checked in the order given; the first failure wins, which is
/// what `spar analyze` has always reported.
pub fn first_parse_failure(sources: &[(String, String)]) -> Option<(&str, String)> {
    sources.iter().find_map(|(path, source)| {
        parse_error_report(path, source).map(|report| (path.as_str(), report))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
    #[test]
    fn clean_source_has_no_report() {
        let src = "package P\npublic\n  system S\n  end S;\nend P;\n";
        assert_eq!(parse_error_report("p.aadl", src), None);
    }

    // rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
    #[test]
    fn reserved_word_feature_is_reported_with_line_and_column() {
        let src = "package P\npublic\n  device D\n  features\n    out : provides bus access;\n  end D;\nend P;\n";
        let report = parse_error_report("p.aadl", src).expect("`out` is reserved");
        assert!(
            report.starts_with("p.aadl:5:5: "),
            "line:col, not a byte offset: {report}"
        );
        assert!(report.ends_with('\n'));
    }

    // rivet: verifies REQ-CLI-LOAD-REFUSES-PARSE-ERRORS-001
    #[test]
    fn first_failure_is_the_first_bad_file_in_order() {
        let good = (
            "g.aadl".to_string(),
            "package G\npublic\nend G;\n".to_string(),
        );
        let bad1 = (
            "b1.aadl".to_string(),
            "package B1\npublic\n  system S\nend B1;\n".to_string(),
        );
        let bad2 = (
            "b2.aadl".to_string(),
            "package B2\npublic\n  system S\nend B2;\n".to_string(),
        );
        let srcs = vec![good.clone(), bad1, bad2];
        let (path, report) = first_parse_failure(&srcs).expect("a bad file is present");
        assert_eq!(path, "b1.aadl");
        assert!(report.starts_with("b1.aadl:"));
        assert!(first_parse_failure(&[good]).is_none());
    }
}
