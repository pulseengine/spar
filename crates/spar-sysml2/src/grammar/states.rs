//! Grammar rules for SysML v2 state-based behaviour (REQ-SYSML2-FLOWS-MODES-001).
//!
//! The shapes are taken from the official corpus (`training/23. State
//! Definitions`, `24. States`, `26. State Exhibition`,
//! `examples/Simple Tests/StateTest.sysml`), not from memory:
//!
//! ```text
//! state def VehicleStates {
//!     first start then off;                    // SUCCESSION_USAGE (initial)
//!     entry; then off;                         // STATE_ACTION + TARGET_SUCCESSION
//!     state off;
//!     accept VehicleStartSignal then starting; // TARGET_TRANSITION_USAGE
//!     transition T
//!         first S2.S3
//!         accept s : Sig via p                 // TRIGGER, TRIGGER_VIA
//!         if true                              // GUARD
//!         do send s to p                       // EFFECT
//!         then S1;                             // TRANSITION_TARGET
//!     state s parallel { ... }
//!     exit act;                                // STATE_ACTION
//! }
//! exhibit state s : VehicleStates;             // STATE_USAGE
//! ```
//!
//! Triggers, guards, effects and state actions are kept as opaque token runs:
//! lowering reads the transition's source, target and `via` port and nothing
//! else, and an expression grammar that lowering never consumes would be a
//! claim with no reader.

use crate::parser::Parser;
use crate::syntax_kind::SyntaxKind;

/// `accept X ...` begins a target transition. `accept : T;` and `accept;` stay
/// feature declarations named `accept`.
pub(crate) fn at_accept(p: &Parser) -> bool {
    p.at_contextual_kw("accept") && starts_operand(p.nth(1))
}

/// `do action A;` / `do send ...;` begins a state action. `do : T;` stays a
/// feature declaration named `do`.
pub(crate) fn at_do_action(p: &Parser) -> bool {
    p.at_contextual_kw("do") && starts_operand(p.nth(1))
}

fn starts_operand(k: SyntaxKind) -> bool {
    k == SyntaxKind::IDENT || k.is_keyword()
}

/// `state name [parallel] [: T] { ... }` and `exhibit [state] name ...`.
pub(crate) fn state_usage(p: &mut Parser) {
    let m = p.start();
    let exhibit = p.eat(SyntaxKind::EXHIBIT_KW);
    let has_state_kw = p.eat(SyntaxKind::STATE_KW);
    if exhibit && !has_state_kw {
        // `exhibit vehicleStates { ... }` exhibits a state by reference.
        let r = p.start();
        super::feature_chain(p);
        r.complete(p, SyntaxKind::NAME_REF);
    } else if super::at_ident_or_kw(p)
        && !p.at(SyntaxKind::DEF_KW)
        && !p.at_contextual_kw("parallel")
    {
        super::name(p);
    }
    eat_parallel(p);
    super::parts::opt_typing_or_specialization(p);
    eat_parallel(p);
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }
    m.complete(p, SyntaxKind::STATE_USAGE);
}

fn eat_parallel(p: &mut Parser) {
    if p.at_contextual_kw("parallel") {
        p.bump_any();
    }
}

/// `transition [name] first S <tail>` or, after a state, `accept ... <tail>`.
pub(crate) fn transition_usage(p: &mut Parser) {
    let m = p.start();
    let kind = if p.eat(SyntaxKind::TRANSITION_KW) {
        // `transition T first S ...` is named; `transition first S ...` and
        // `transition S then D;` (Annex: `transition initial then off;`) are not.
        if p.nth(1) == SyntaxKind::FIRST_KW {
            super::name(p);
        }
        p.eat(SyntaxKind::FIRST_KW);
        let s = p.start();
        super::feature_chain(p);
        s.complete(p, SyntaxKind::TRANSITION_SOURCE);
        SyntaxKind::TRANSITION_USAGE
    } else {
        SyntaxKind::TARGET_TRANSITION_USAGE
    };
    if p.at_contextual_kw("accept") {
        trigger(p);
    }
    if p.at(SyntaxKind::IF_KW) {
        let g = p.start();
        p.bump(SyntaxKind::IF_KW);
        skip_until(p, &[SyntaxKind::THEN_KW], &["do"]);
        g.complete(p, SyntaxKind::GUARD);
    }
    if p.at_contextual_kw("do") {
        let e = p.start();
        p.bump_any();
        skip_until(p, &[SyntaxKind::THEN_KW], &[]);
        e.complete(p, SyntaxKind::EFFECT);
    }
    p.expect(SyntaxKind::THEN_KW);
    let t = p.start();
    super::feature_chain(p);
    t.complete(p, SyntaxKind::TRANSITION_TARGET);
    end_of_member(p);
    m.complete(p, kind);
}

fn trigger(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // `accept`
    let mut depth = 0usize;
    while !p.at_end() {
        let k = p.current();
        if depth == 0
            && (k == SyntaxKind::THEN_KW
                || k == SyntaxKind::IF_KW
                || k == SyntaxKind::SEMICOLON
                || k == SyntaxKind::R_CURLY
                || p.at_contextual_kw("do"))
        {
            break;
        }
        if depth == 0 && p.at_contextual_kw("via") {
            let v = p.start();
            p.bump_any();
            super::feature_chain(p);
            v.complete(p, SyntaxKind::TRIGGER_VIA);
            continue;
        }
        depth = track_depth(k, depth);
        p.bump_any();
    }
    m.complete(p, SyntaxKind::TRIGGER);
}

/// `first S then D;`
pub(crate) fn succession_usage(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::FIRST_KW);
    let s = p.start();
    super::feature_chain(p);
    s.complete(p, SyntaxKind::TRANSITION_SOURCE);
    p.expect(SyntaxKind::THEN_KW);
    let t = p.start();
    super::feature_chain(p);
    t.complete(p, SyntaxKind::TRANSITION_TARGET);
    end_of_member(p);
    m.complete(p, SyntaxKind::SUCCESSION_USAGE);
}

/// `then D;`
pub(crate) fn target_succession(p: &mut Parser) {
    let m = p.start();
    p.bump(SyntaxKind::THEN_KW);
    let t = p.start();
    super::feature_chain(p);
    t.complete(p, SyntaxKind::TRANSITION_TARGET);
    end_of_member(p);
    m.complete(p, SyntaxKind::TARGET_SUCCESSION);
}

/// `entry ...`, `do ...`, `exit ...`: up to a `;` or through one balanced
/// `{ ... }` body, whichever comes first at depth 0.
pub(crate) fn state_action(p: &mut Parser) {
    let m = p.start();
    p.bump_any(); // `entry` / `exit` / `do`
    let mut depth = 0usize;
    while !p.at_end() {
        let k = p.current();
        if depth == 0 && k == SyntaxKind::SEMICOLON {
            p.bump_any();
            break;
        }
        if depth == 0 && k == SyntaxKind::R_CURLY {
            break; // the enclosing body ends; let it report the missing `;`
        }
        let closes = depth == 1 && k == SyntaxKind::R_CURLY;
        depth = track_depth(k, depth);
        p.bump_any();
        if closes {
            break;
        }
    }
    m.complete(p, SyntaxKind::STATE_ACTION);
}

fn end_of_member(p: &mut Parser) {
    if p.at(SyntaxKind::L_CURLY) {
        super::packages::namespace_body(p);
    } else {
        p.expect(SyntaxKind::SEMICOLON);
    }
}

/// Consume tokens up to (not including) a depth-0 stop token, `;`, or the
/// enclosing body's `}`.
fn skip_until(p: &mut Parser, stops: &[SyntaxKind], contextual: &[&str]) {
    let mut depth = 0usize;
    while !p.at_end() {
        let k = p.current();
        if depth == 0
            && (stops.contains(&k)
                || k == SyntaxKind::SEMICOLON
                || k == SyntaxKind::R_CURLY
                || contextual.iter().any(|c| p.at_contextual_kw(c)))
        {
            break;
        }
        depth = track_depth(k, depth);
        p.bump_any();
    }
}

fn track_depth(k: SyntaxKind, depth: usize) -> usize {
    match k {
        SyntaxKind::L_PAREN | SyntaxKind::L_BRACKET | SyntaxKind::L_CURLY => depth + 1,
        SyntaxKind::R_PAREN | SyntaxKind::R_BRACKET | SyntaxKind::R_CURLY => {
            depth.saturating_sub(1)
        }
        _ => depth,
    }
}
