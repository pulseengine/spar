//! SysML v2 state machines lower to AADL modes and mode transitions
//! (REQ-SYSML2-FLOWS-MODES-001, the modes half).
//!
//! The oracle is the lowered AADL text plus the lowering diagnostics: every
//! construct AADL cannot hold must be REPORTED, and nothing absent from the
//! model (an initial mode, a trigger) may appear in the output.

use spar_sysml2::lower::{item_tree_to_aadl, lower_to_aadl_with_diagnostics};

/// A bare snippet is wrapped in a package: the AADL printer renders a part def
/// outside one as a comment only.
fn lower(src: &str) -> (String, Vec<String>) {
    let wrapped;
    let src = if src.starts_with("package") {
        src
    } else {
        wrapped = format!("package T {{ {src} }}");
        &wrapped
    };
    let parse = spar_sysml2::parse(src);
    assert!(parse.ok(), "`{src}` must parse: {:?}", parse.errors());
    let (tree, diags) = lower_to_aadl_with_diagnostics(&parse);
    (
        item_tree_to_aadl(&tree),
        diags.into_iter().map(|d| d.msg).collect(),
    )
}

fn has(msgs: &[String], needle: &str) -> bool {
    msgs.iter().any(|m| m.contains(needle))
}

/// A part with one exhibited machine: the canonical shape of the official
/// training files (`23. State Definitions`, `26. State Exhibition`).
const CANONICAL: &str = "package Modal {
  port def Cmd;
  part def Ctrl {
    port cmd : Cmd;
    exhibit state modes {
      entry; then off;
      state off;
      accept Start via cmd then on;
      state on;
      transition on_off first on accept Stop via cmd then off;
      state isolated;
    }
  }
  part def Top { part c : Ctrl; }
}";

// REQ-SYSML2-FLOWS-MODES-001, modes half only (no `verifies` marker: the flows half is open)
#[test]
fn the_substates_of_the_machine_are_the_modes_and_entry_names_the_initial_one() {
    let (aadl, d) = lower(CANONICAL);
    assert!(aadl.contains("    off : initial mode;\n"), "{aadl}");
    assert!(aadl.contains("    on : mode;\n"), "{aadl}");
    assert!(aadl.contains("    isolated : mode;\n"), "{aadl}");
    assert!(
        !aadl.contains("modes : "),
        "the container is not a mode:\n{aadl}"
    );
    assert_eq!(aadl.matches("initial mode").count(), 1, "{aadl}");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn an_accept_after_a_state_starts_from_that_state_and_via_names_the_trigger() {
    let (aadl, _) = lower(CANONICAL);
    assert!(aadl.contains("    off -[ cmd ]-> on;\n"), "{aadl}");
}

#[test]
fn a_named_transition_keeps_its_name_source_and_target() {
    let (aadl, _) = lower(CANONICAL);
    assert!(aadl.contains("    on_off : on -[ cmd ]-> off;\n"), "{aadl}");
}

#[test]
fn first_start_then_names_the_initial_mode() {
    let (aadl, d) = lower("part def P { state m { first start then b; state a; state b; } }");
    assert!(aadl.contains("    a : mode;\n"), "{aadl}");
    assert!(aadl.contains("    b : initial mode;\n"), "{aadl}");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn a_machine_that_names_no_initial_state_gets_none_and_is_reported() {
    let (aadl, d) = lower("part def P { state m { state a; state b; } }");
    assert!(
        !aadl.contains("initial"),
        "no initial mode may be invented:\n{aadl}"
    );
    assert!(has(&d, "no initial mode"), "{d:?}");
}

#[test]
fn a_second_initial_state_is_reported_and_the_first_is_kept() {
    let (aadl, d) =
        lower("part def P { state m { entry; then a; first start then b; state a; state b; } }");
    assert!(aadl.contains("    a : initial mode;\n"), "{aadl}");
    assert!(aadl.contains("    b : mode;\n"), "{aadl}");
    assert!(has(&d, "a second initial state"), "{d:?}");
}

#[test]
fn an_initial_state_that_is_not_declared_is_reported_and_not_lowered() {
    let (aadl, d) = lower("part def P { state m { entry; then ghost; state a; } }");
    assert!(!aadl.contains("initial"), "{aadl}");
    assert!(has(&d, "initial state `ghost` is not a state"), "{d:?}");
}

#[test]
fn a_trigger_without_via_is_reported_and_the_transition_has_no_trigger() {
    let (aadl, d) = lower(
        "part def P { port p; state m { entry; then a; state a; accept Go then b; state b; } }",
    );
    assert!(aadl.contains("    a -[  ]-> b;\n"), "{aadl}");
    assert!(has(&d, "add `via <port>`"), "{d:?}");
}

#[test]
fn a_via_that_is_not_a_port_is_reported_and_not_a_trigger() {
    let (aadl, d) = lower(
        "part def P { port p; state m { entry; then a; state a; accept Go via q then b; state b; } }",
    );
    assert!(aadl.contains("    a -[  ]-> b;\n"), "{aadl}");
    assert!(has(&d, "`q` is not a port"), "{d:?}");
}

#[test]
fn a_guard_and_an_effect_are_reported() {
    let (aadl, d) = lower(
        "part def P { port p; state m { entry; then a; state a; \
         transition first a accept E via p if ok do send e to p then b; state b; } }",
    );
    assert!(aadl.contains("    a -[ p ]-> b;\n"), "{aadl}");
    assert!(has(&d, "guard `if ok`"), "{d:?}");
    assert!(has(&d, "effect `do send e to p`"), "{d:?}");
}

#[test]
fn a_then_after_a_state_is_a_triggerless_transition_from_it() {
    let (aadl, d) = lower("part def P { state m { entry; then a; state a; then b; state b; } }");
    assert!(aadl.contains("    a -[  ]-> b;\n"), "{aadl}");
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn a_transition_to_an_undeclared_or_nested_state_is_reported_and_dropped() {
    let (aadl, d) = lower(
        "part def P { state m { entry; then a; state a; state b { state c; } \
         transition first a then nowhere; transition first a then b.c; } }",
    );
    assert!(!aadl.contains("nowhere") && !aadl.contains("b.c"), "{aadl}");
    assert!(has(&d, "`nowhere` is not a state"), "{d:?}");
    assert!(has(&d, "`b.c` is not a state"), "{d:?}");
    assert!(has(&d, "the substates of `state b`"), "{d:?}");
}

#[test]
fn a_succession_that_does_not_start_at_start_is_reported() {
    let (_, d) =
        lower("part def P { state m { entry; then a; state a; first a then b; state b; } }");
    assert!(has(&d, "only `first start then S;` is read"), "{d:?}");
}

#[test]
fn a_state_action_is_reported_but_a_bare_entry_is_not() {
    let (_, d) =
        lower("part def P { state m { entry; then a; state a; do action run; state b; } }");
    assert!(has(&d, "`do action run` (not lowered"), "{d:?}");
    assert!(!has(&d, "`entry`"), "{d:?}");
}

#[test]
fn a_second_machine_and_a_stray_member_are_reported() {
    let (aadl, d) = lower(
        "part def P { state m { entry; then a; state a; } state n { state x; } state loose; }",
    );
    assert!(aadl.contains("    a : initial mode;\n"), "{aadl}");
    assert!(
        !aadl.contains("    x : mode;") && !aadl.contains("loose"),
        "{aadl}"
    );
    assert!(has(&d, "a second state machine `state n`"), "{d:?}");
    assert!(has(&d, "`state loose` beside the state machine"), "{d:?}");
}

#[test]
fn bare_states_in_a_part_def_are_still_its_modes() {
    let (aadl, d) =
        lower("part def P { port p; entry; then a; state a; accept E via p then b; state b; }");
    assert!(aadl.contains("    a : initial mode;\n"), "{aadl}");
    assert!(aadl.contains("    a -[ p ]-> b;\n"), "{aadl}");
    assert!(d.is_empty(), "{d:?}");
}

/// REGRESSION: before the state grammar, the `transition` was a parse error
/// whose recovery swallowed `part def Top`.
#[test]
fn a_part_def_after_a_state_machine_survives() {
    let (aadl, _) = lower(CANONICAL);
    assert!(aadl.contains("  system Top\n"), "{aadl}");
}
