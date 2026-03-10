//! BA grammar rules.
//!
//! Recursive descent parser for the Behavior Annex sublanguage.
//! Specification: SAE AS5506/2 Annex D (Behavior Annex)
//! Reference: OSATE2 org.osate.ba

use super::parser::{CompletedMarker, Parser};
use super::syntax_kind::BaKind;

// ── Root ─────────────────────────────────────────────────────────

/// Parse BA content: `[variables ...] [states ...] [transitions ...]`
pub(crate) fn root(p: &mut Parser) {
    let m = p.start();

    // variables section (optional)
    if p.at(BaKind::VARIABLES_KW) {
        behavior_variables_section(p);
    }

    // states section (optional)
    if p.at(BaKind::STATES_KW) {
        behavior_states_section(p);
    }

    // transitions section (optional)
    if p.at(BaKind::TRANSITIONS_KW) {
        behavior_transitions_section(p);
    }

    // Consume any remaining tokens as errors
    while !p.at_end() {
        p.err_and_bump("unexpected token in behavior annex");
    }

    m.complete(p, BaKind::BA_ROOT);
}

// ── Variables Section ────────────────────────────────────────────

/// `variables { behavior_variable }*`
fn behavior_variables_section(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::VARIABLES_KW);

    while !p.at_end() && !p.at(BaKind::STATES_KW) && !p.at(BaKind::TRANSITIONS_KW) {
        behavior_variable(p);
    }

    m.complete(p, BaKind::BEHAVIOR_VARIABLES_SECTION);
}

/// `Name {, Name}* : TypeRef [':=' InitExpr] ;`
fn behavior_variable(p: &mut Parser) {
    let m = p.start();

    // Name list (comma-separated identifiers)
    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected variable name");
    }
    while p.eat(BaKind::COMMA) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected variable name after ','");
        }
    }

    p.expect(BaKind::COLON);

    // Type reference: qualified name, possibly classifier(...)
    type_reference(p);

    // Optional initializer: := expr
    if p.eat(BaKind::COLON_EQ) {
        expression(p);
    }

    p.expect(BaKind::SEMICOLON);
    m.complete(p, BaKind::BEHAVIOR_VARIABLE);
}

// ── States Section ───────────────────────────────────────────────

/// `states { behavior_state }*`
fn behavior_states_section(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::STATES_KW);

    while !p.at_end() && !p.at(BaKind::TRANSITIONS_KW) {
        if p.at(BaKind::IDENT) {
            behavior_state(p);
        } else {
            p.err_and_bump("expected state declaration");
        }
    }

    m.complete(p, BaKind::BEHAVIOR_STATES_SECTION);
}

/// `Name {, Name}* : [initial] [complete] [final] state ;`
fn behavior_state(p: &mut Parser) {
    let m = p.start();

    // Name list
    p.bump(BaKind::IDENT);
    while p.eat(BaKind::COMMA) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected state name after ','");
        }
    }

    p.expect(BaKind::COLON);

    // State kind qualifiers
    let km = p.start();
    let mut has_qualifier = false;
    while matches!(
        p.current(),
        BaKind::INITIAL_KW | BaKind::COMPLETE_KW | BaKind::FINAL_KW
    ) {
        p.bump_any();
        has_qualifier = true;
    }
    if has_qualifier {
        km.complete(p, BaKind::STATE_KIND_LIST);
    } else {
        km.abandon(p);
    }

    p.expect(BaKind::STATE_KW);
    p.expect(BaKind::SEMICOLON);

    m.complete(p, BaKind::BEHAVIOR_STATE);
}

// ── Transitions Section ──────────────────────────────────────────

/// `transitions { behavior_transition }*`
fn behavior_transitions_section(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::TRANSITIONS_KW);

    while !p.at_end() {
        if p.at(BaKind::IDENT) || p.at(BaKind::TRANS_OPEN) {
            // Check if this looks like a transition (has IDENT possibly followed by more
            // transition syntax, or starts with -[)
            behavior_transition(p);
        } else {
            p.err_and_bump("expected transition declaration");
        }
    }

    m.complete(p, BaKind::BEHAVIOR_TRANSITIONS_SECTION);
}

/// `[Name [( Priority )] :] SourceState {, SourceState}* -[ Condition ]-> DestState [{ActionBlock}] ;`
fn behavior_transition(p: &mut Parser) {
    let m = p.start();

    // Detect whether this transition has a name prefix.
    // Pattern: Name ':' or Name '(' INT ')' ':'
    // vs just source states: Name {, Name}* -[
    if p.at(BaKind::IDENT) && has_transition_name(p) {
        // Transition name
        p.bump(BaKind::IDENT);

        // Optional priority: (INT)
        if p.at(BaKind::L_PAREN) && p.nth(1) == BaKind::INT_LIT {
            p.bump(BaKind::L_PAREN);
            p.bump(BaKind::INT_LIT);
            p.expect(BaKind::R_PAREN);
        }

        p.expect(BaKind::COLON);
    }

    // Source states (comma-separated)
    let sl = p.start();
    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    }
    while p.eat(BaKind::COMMA) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected source state name");
        }
    }
    sl.complete(p, BaKind::SOURCE_STATE_LIST);

    // -[ Condition ]->
    p.expect(BaKind::TRANS_OPEN);

    // Guard/condition (may be empty for unconditional transitions)
    if !p.at(BaKind::TRANS_CLOSE) {
        transition_guard(p);
    }

    p.expect(BaKind::TRANS_CLOSE);

    // Destination state
    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected destination state");
    }

    // Optional action block { ... }
    if p.at(BaKind::L_CURLY) {
        action_block(p);
    }

    // Optional timeout clause after action block
    if p.at(BaKind::TIMEOUT_KW) {
        timeout_clause(p);
    }

    p.expect(BaKind::SEMICOLON);

    m.complete(p, BaKind::BEHAVIOR_TRANSITION);
}

/// Look ahead to determine if the current IDENT is a transition name
/// (followed by optional priority and colon) vs a source state name.
fn has_transition_name(p: &Parser) -> bool {
    // Pattern 1: Name ':'  (but not Name '::' which is qualified name)
    if p.nth(1) == BaKind::COLON && p.nth(2) != BaKind::COLON {
        return true;
    }
    // Pattern 2: Name '(' INT ')' ':'
    if p.nth(1) == BaKind::L_PAREN
        && p.nth(2) == BaKind::INT_LIT
        && p.nth(3) == BaKind::R_PAREN
        && p.nth(4) == BaKind::COLON
    {
        return true;
    }
    false
}

// ── Transition Guard ─────────────────────────────────────────────

/// Parse the content inside `-[ ... ]->`, which can be:
/// - dispatch condition: `on dispatch ...`
/// - execute condition: a value expression (on non-complete states)
/// - otherwise
/// - timeout
/// - empty
fn transition_guard(p: &mut Parser) {
    let m = p.start();

    if p.at(BaKind::ON_KW) && p.nth(1) == BaKind::DISPATCH_KW {
        dispatch_condition(p);
    } else if p.at(BaKind::OTHERWISE_KW) {
        p.bump(BaKind::OTHERWISE_KW);
    } else if p.at(BaKind::TIMEOUT_KW) {
        p.bump(BaKind::TIMEOUT_KW);
    } else {
        // Execute condition: value expression
        expression(p);
    }

    m.complete(p, BaKind::TRANSITION_GUARD);
}

// ── Dispatch Condition ───────────────────────────────────────────

/// `on dispatch [trigger_logical_expr] [frozen port_list]`
/// `on dispatch timeout [timeout_value]`
/// `on dispatch stop`
fn dispatch_condition(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::ON_KW);
    p.bump(BaKind::DISPATCH_KW);

    if p.at(BaKind::STOP_KW) {
        p.bump(BaKind::STOP_KW);
    } else if p.at(BaKind::TIMEOUT_KW) {
        p.bump(BaKind::TIMEOUT_KW);
        // Optional timeout value
        if !p.at(BaKind::TRANS_CLOSE) && !p.at(BaKind::FROZEN_KW) {
            expression(p);
        }
    } else if !p.at(BaKind::TRANS_CLOSE) && !p.at(BaKind::FROZEN_KW) {
        // Trigger logical expression (port names with and/or)
        dispatch_trigger_logical_expr(p);
    }

    // Optional frozen ports
    if p.at(BaKind::FROZEN_KW) {
        frozen_port_list(p);
    }

    m.complete(p, BaKind::DISPATCH_CONDITION);
}

/// Trigger logical expression: disjunction of conjunctions.
/// `trigger_conjunction { or trigger_conjunction }*`
fn dispatch_trigger_logical_expr(p: &mut Parser) {
    let m = p.start();
    dispatch_trigger_conjunction(p);

    while p.at(BaKind::OR_KW) {
        p.bump(BaKind::OR_KW);
        dispatch_trigger_conjunction(p);
    }

    m.complete(p, BaKind::DISPATCH_TRIGGER_LOGICAL_EXPR);
}

/// Trigger conjunction: port names combined with `and`.
/// `port_name { and port_name }*`
fn dispatch_trigger_conjunction(p: &mut Parser) {
    let m = p.start();

    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected port name in dispatch trigger");
    }

    while p.at(BaKind::AND_KW) {
        p.bump(BaKind::AND_KW);
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected port name after 'and'");
        }
    }

    m.complete(p, BaKind::DISPATCH_TRIGGER_CONJUNCTION);
}

/// `frozen port_name {, port_name}*`
fn frozen_port_list(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::FROZEN_KW);

    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected port name after 'frozen'");
    }
    while p.eat(BaKind::COMMA) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected port name after ','");
        }
    }

    m.complete(p, BaKind::FROZEN_PORT_LIST);
}

// ── Action Block ─────────────────────────────────────────────────

/// `{ behavior_actions }`
fn action_block(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::L_CURLY);

    if !p.at(BaKind::R_CURLY) {
        behavior_actions(p);
    }

    p.expect(BaKind::R_CURLY);
    m.complete(p, BaKind::ACTION_BLOCK);
}

/// Actions separated by `;` (sequential) or `&` (parallel).
/// We parse a flat list with the separators as tokens.
fn behavior_actions(p: &mut Parser) {
    let m = p.start();

    behavior_action(p);

    while !p.at_end() && !p.at(BaKind::R_CURLY) {
        if p.eat(BaKind::SEMICOLON) {
            // Might be trailing semicolon before }
            if p.at(BaKind::R_CURLY) || p.at_end() {
                break;
            }
            behavior_action(p);
        } else if p.eat(BaKind::AMP) {
            behavior_action(p);
        } else {
            break;
        }
    }

    m.complete(p, BaKind::BEHAVIOR_ACTIONS);
}

/// A single behavior action: assignment, communication, computation,
/// timed action, if, for, forall, while, do-until, or subprogram call.
fn behavior_action(p: &mut Parser) {
    let m = p.start();

    match p.current() {
        BaKind::IF_KW => {
            if_statement(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        BaKind::FOR_KW => {
            for_statement(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        BaKind::FORALL_KW => {
            forall_statement(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        BaKind::WHILE_KW => {
            while_statement(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        BaKind::DO_KW => {
            do_until_statement(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        BaKind::COMPUTATION_KW => {
            computation_action(p);
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
        _ => {
            // Could be: assignment, communication action, or subprogram call.
            // All start with a name reference.
            // We need to parse the reference first, then decide based on what follows.
            //
            // assignment: target := value_expression
            // communication: port! | port!(expr) | port?(target) | port>> | port!< | port!>
            // subprogram call: sub!(params)
            //
            // The tricky bit: port!(expr) and sub!(params) look the same.
            // We treat both as COMMUNICATION_ACTION (which covers subprogram calls).
            if !p.at_end()
                && !p.at(BaKind::R_CURLY)
                && !p.at(BaKind::SEMICOLON)
                && !p.at(BaKind::AMP)
            {
                // Parse a name/reference, then decide
                parse_action_starting_with_name(p);
            } else {
                p.error("expected behavior action");
            }
            m.complete(p, BaKind::BEHAVIOR_ACTION);
        }
    }
}

/// Parse an action that starts with a name reference (assignment, comm, or call).
fn parse_action_starting_with_name(p: &mut Parser) {
    // Parse the name reference (could be dotted, indexed, etc.)
    let nm = p.start();
    name_reference(p);

    match p.current() {
        BaKind::COLON_EQ => {
            // Assignment: target := value_expression
            p.bump(BaKind::COLON_EQ);
            expression(p);
            nm.complete(p, BaKind::ASSIGNMENT_ACTION);
        }
        BaKind::BANG => {
            // Communication: port! or port!(expr) or subprogram!(params)
            p.bump(BaKind::BANG);
            if p.at(BaKind::L_PAREN) {
                // port!(expr, ...) or subprogram!(params)
                subprogram_call_params(p);
            }
            // else: just port! (send event)
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
        BaKind::BANG_L_ANGLE => {
            // port!< (lock)
            p.bump(BaKind::BANG_L_ANGLE);
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
        BaKind::BANG_R_ANGLE => {
            // port!> (unlock)
            p.bump(BaKind::BANG_R_ANGLE);
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
        BaKind::QUESTION => {
            // port?(target)
            p.bump(BaKind::QUESTION);
            if p.at(BaKind::L_PAREN) {
                p.bump(BaKind::L_PAREN);
                if !p.at(BaKind::R_PAREN) {
                    name_reference(p);
                }
                p.expect(BaKind::R_PAREN);
            }
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
        BaKind::R_ANGLE_R_ANGLE => {
            // port>> (alternative receive)
            p.bump(BaKind::R_ANGLE_R_ANGLE);
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
        _ => {
            // Might be a bare name reference as an expression (e.g., in conditions)
            // or something we don't understand. Wrap it up.
            nm.complete(p, BaKind::COMMUNICATION_ACTION);
        }
    }
}

/// `( expr {, expr}* )`
fn subprogram_call_params(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::L_PAREN);

    if !p.at(BaKind::R_PAREN) {
        expression(p);
        while p.eat(BaKind::COMMA) {
            expression(p);
        }
    }

    p.expect(BaKind::R_PAREN);
    m.complete(p, BaKind::SUBPROGRAM_CALL_PARAMS);
}

// ── Control Flow ─────────────────────────────────────────────────

/// `if (cond) actions [elsif (cond) actions]* [else actions] end if`
fn if_statement(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::IF_KW);

    // condition
    p.expect(BaKind::L_PAREN);
    expression(p);
    p.expect(BaKind::R_PAREN);

    // then actions (inside implicit block)
    if !p.at(BaKind::ELSIF_KW) && !p.at(BaKind::ELSE_KW) && !p.at(BaKind::END_KW) {
        behavior_actions(p);
    }

    // elsif clauses
    while p.at(BaKind::ELSIF_KW) {
        let em = p.start();
        p.bump(BaKind::ELSIF_KW);
        p.expect(BaKind::L_PAREN);
        expression(p);
        p.expect(BaKind::R_PAREN);
        if !p.at(BaKind::ELSIF_KW) && !p.at(BaKind::ELSE_KW) && !p.at(BaKind::END_KW) {
            behavior_actions(p);
        }
        em.complete(p, BaKind::ELSIF_CLAUSE);
    }

    // else clause
    if p.at(BaKind::ELSE_KW) {
        let em = p.start();
        p.bump(BaKind::ELSE_KW);
        if !p.at(BaKind::END_KW) {
            behavior_actions(p);
        }
        em.complete(p, BaKind::ELSE_CLAUSE);
    }

    p.expect(BaKind::END_KW);
    p.expect(BaKind::IF_KW);

    m.complete(p, BaKind::IF_STATEMENT);
}

/// `for ( var : type in expr [.. expr] ) { actions }`
fn for_statement(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::FOR_KW);

    p.expect(BaKind::L_PAREN);
    // variable name
    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected loop variable name");
    }
    p.expect(BaKind::COLON);
    // type reference
    type_reference(p);
    p.expect(BaKind::IN_KW);
    // range/collection expression: expr [.. expr]
    expression(p);
    if p.eat(BaKind::DOT_DOT) {
        expression(p);
    }
    p.expect(BaKind::R_PAREN);

    // action block
    if p.at(BaKind::L_CURLY) {
        action_block(p);
    }

    m.complete(p, BaKind::FOR_STATEMENT);
}

/// `forall ( var : type in expr [.. expr] ) { actions }`
fn forall_statement(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::FORALL_KW);

    p.expect(BaKind::L_PAREN);
    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else {
        p.error("expected loop variable name");
    }
    p.expect(BaKind::COLON);
    type_reference(p);
    p.expect(BaKind::IN_KW);
    expression(p);
    if p.eat(BaKind::DOT_DOT) {
        expression(p);
    }
    p.expect(BaKind::R_PAREN);

    if p.at(BaKind::L_CURLY) {
        action_block(p);
    }

    m.complete(p, BaKind::FORALL_STATEMENT);
}

/// `while ( cond ) { actions }`
fn while_statement(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::WHILE_KW);

    p.expect(BaKind::L_PAREN);
    expression(p);
    p.expect(BaKind::R_PAREN);

    if p.at(BaKind::L_CURLY) {
        action_block(p);
    }

    m.complete(p, BaKind::WHILE_STATEMENT);
}

/// `do actions until ( cond )`
fn do_until_statement(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::DO_KW);

    // actions
    behavior_actions(p);

    p.expect(BaKind::UNTIL_KW);
    p.expect(BaKind::L_PAREN);
    expression(p);
    p.expect(BaKind::R_PAREN);

    m.complete(p, BaKind::DO_UNTIL_STATEMENT);
}

// ── Computation Action ───────────────────────────────────────────

/// `computation ( range ) [in binding ( processor )]`
fn computation_action(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::COMPUTATION_KW);

    p.expect(BaKind::L_PAREN);
    // Parse the range or single value: min .. max or single
    expression(p);
    if p.eat(BaKind::DOT_DOT) {
        expression(p);
    }
    p.expect(BaKind::R_PAREN);

    // Optional: in binding ( processor )
    if p.at(BaKind::IN_KW) {
        p.bump(BaKind::IN_KW);
        p.expect(BaKind::BINDING_KW);
        p.expect(BaKind::L_PAREN);
        name_reference(p);
        p.expect(BaKind::R_PAREN);
    }

    m.complete(p, BaKind::COMPUTATION_ACTION);
}

/// Timeout clause: `timeout expr`
fn timeout_clause(p: &mut Parser) {
    let m = p.start();
    p.bump(BaKind::TIMEOUT_KW);
    // Timeout value (time expression)
    if !p.at(BaKind::SEMICOLON) && !p.at_end() {
        expression(p);
    }
    m.complete(p, BaKind::TIMEOUT_CLAUSE);
}

// ── Type Reference ───────────────────────────────────────────────

/// Parse a type reference: qualified name or `classifier(...)`.
fn type_reference(p: &mut Parser) {
    if p.at(BaKind::CLASSIFIER_KW) {
        let m = p.start();
        p.bump(BaKind::CLASSIFIER_KW);
        p.expect(BaKind::L_PAREN);
        qualified_name(p);
        p.expect(BaKind::R_PAREN);
        m.complete(p, BaKind::CLASSIFIER_REF);
    } else {
        qualified_name(p);
    }
}

/// Parse a qualified name: `ident { :: ident }* [. ident]`
fn qualified_name(p: &mut Parser) {
    let m = p.start();

    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else if p.current().is_keyword() {
        // Keywords can sometimes appear as identifiers in type refs
        p.bump_any();
    } else {
        p.error("expected name");
        m.abandon(p);
        return;
    }

    // :: segments
    while p.eat(BaKind::COLON_COLON) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else if p.current().is_keyword() {
            p.bump_any();
        } else {
            p.error("expected name after '::'");
        }
    }

    // . for implementation references (e.g., pkg::type.impl)
    if p.eat(BaKind::DOT) {
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else if p.current().is_keyword() {
            p.bump_any();
        } else {
            p.error("expected implementation name after '.'");
        }
    }

    m.complete(p, BaKind::QUALIFIED_NAME);
}

// ── Name Reference ───────────────────────────────────────────────

/// Parse a name reference used in actions/expressions.
/// Handles dotted names, array indexing, port properties.
/// `ident { . ident }* [ [expr] ] [ 'property ]`
fn name_reference(p: &mut Parser) {
    let m = p.start();

    if p.at(BaKind::IDENT) {
        p.bump(BaKind::IDENT);
    } else if p.current().is_keyword() {
        // Some keywords can be used as identifiers in references
        p.bump_any();
    } else {
        p.error("expected name reference");
        m.complete(p, BaKind::NAME_REF);
        return;
    }

    // :: for package-qualified names
    while p.at(BaKind::COLON_COLON) {
        p.bump(BaKind::COLON_COLON);
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else if p.current().is_keyword() {
            p.bump_any();
        } else {
            p.error("expected name after '::'");
        }
    }

    // Dotted segments
    while p.at(BaKind::DOT) {
        p.bump(BaKind::DOT);
        if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else if p.current().is_keyword() {
            p.bump_any();
        } else {
            p.error("expected field name after '.'");
            break;
        }

        // Array index after field name
        if p.at(BaKind::L_BRACK) {
            let am = p.start();
            p.bump(BaKind::L_BRACK);
            expression(p);
            p.expect(BaKind::R_BRACK);
            am.complete(p, BaKind::ARRAY_INDEX);
        }
    }

    // Array index on the main name
    if p.at(BaKind::L_BRACK) {
        let am = p.start();
        p.bump(BaKind::L_BRACK);
        expression(p);
        p.expect(BaKind::R_BRACK);
        am.complete(p, BaKind::ARRAY_INDEX);
    }

    // Port property: 'count or 'fresh
    if p.at(BaKind::TICK) {
        p.bump(BaKind::TICK);
        if p.at(BaKind::COUNT_KW) || p.at(BaKind::FRESH_KW) {
            p.bump_any();
        } else if p.at(BaKind::IDENT) {
            p.bump(BaKind::IDENT);
        } else {
            p.error("expected property name after tick");
        }
    }

    m.complete(p, BaKind::NAME_REF);
}

// ── Expressions ──────────────────────────────────────────────────
//
// Precedence (low to high):
// 1. or
// 2. xor
// 3. and
// 4. relational: =, !=, <, <=, >, >=
// 5. additive: +, -
// 6. multiplicative: *, /, mod, rem
// 7. power: **
// 8. unary: not, abs, +, -
// 9. primary: literals, references, parens, function calls

/// Top-level expression entry point.
pub(crate) fn expression(p: &mut Parser) {
    or_expr(p);
}

/// `xor_expr { or xor_expr }*`
fn or_expr(p: &mut Parser) {
    let mut lhs = xor_expr(p);

    while p.at(BaKind::OR_KW) {
        if let Some(cm) = lhs {
            let m = cm.precede(p);
            p.bump(BaKind::OR_KW);
            xor_expr(p);
            lhs = Some(m.complete(p, BaKind::BINARY_EXPR));
        } else {
            break;
        }
    }
}

/// `and_expr { xor and_expr }*`
fn xor_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let mut lhs = and_expr(p);

    while p.at(BaKind::XOR_KW) {
        if let Some(cm) = lhs {
            let m = cm.precede(p);
            p.bump(BaKind::XOR_KW);
            and_expr(p);
            lhs = Some(m.complete(p, BaKind::BINARY_EXPR));
        } else {
            break;
        }
    }

    lhs
}

/// `relational_expr { and relational_expr }*`
fn and_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let mut lhs = relational_expr(p);

    while p.at(BaKind::AND_KW) {
        if let Some(cm) = lhs {
            let m = cm.precede(p);
            p.bump(BaKind::AND_KW);
            relational_expr(p);
            lhs = Some(m.complete(p, BaKind::BINARY_EXPR));
        } else {
            break;
        }
    }

    lhs
}

/// `additive_expr [ relop additive_expr ]`
fn relational_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let lhs = additive_expr(p);

    if matches!(
        p.current(),
        BaKind::EQ
            | BaKind::BANG_EQ
            | BaKind::L_ANGLE
            | BaKind::L_ANGLE_EQ
            | BaKind::R_ANGLE
            | BaKind::R_ANGLE_EQ
    ) && let Some(cm) = lhs
    {
        let m = cm.precede(p);
        p.bump_any(); // the relational operator
        additive_expr(p);
        return Some(m.complete(p, BaKind::BINARY_EXPR));
    }

    lhs
}

/// `multiplicative_expr { (+|-) multiplicative_expr }*`
fn additive_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let mut lhs = multiplicative_expr(p);

    while matches!(p.current(), BaKind::PLUS | BaKind::MINUS) {
        if let Some(cm) = lhs {
            let m = cm.precede(p);
            p.bump_any();
            multiplicative_expr(p);
            lhs = Some(m.complete(p, BaKind::BINARY_EXPR));
        } else {
            break;
        }
    }

    lhs
}

/// `power_expr { (*|/|mod|rem) power_expr }*`
fn multiplicative_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let mut lhs = power_expr(p);

    while matches!(
        p.current(),
        BaKind::STAR | BaKind::SLASH | BaKind::MOD_KW | BaKind::REM_KW
    ) {
        if let Some(cm) = lhs {
            let m = cm.precede(p);
            p.bump_any();
            power_expr(p);
            lhs = Some(m.complete(p, BaKind::BINARY_EXPR));
        } else {
            break;
        }
    }

    lhs
}

/// `unary_expr [ ** unary_expr ]`
fn power_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let lhs = unary_expr(p);

    if p.at(BaKind::STAR_STAR)
        && let Some(cm) = lhs
    {
        let m = cm.precede(p);
        p.bump(BaKind::STAR_STAR);
        unary_expr(p);
        return Some(m.complete(p, BaKind::BINARY_EXPR));
    }

    lhs
}

/// `[not|abs|+|-] primary_expr`
fn unary_expr(p: &mut Parser) -> Option<CompletedMarker> {
    if matches!(
        p.current(),
        BaKind::NOT_KW | BaKind::ABS_KW | BaKind::PLUS | BaKind::MINUS
    ) {
        let m = p.start();
        p.bump_any();
        primary_expr(p);
        return Some(m.complete(p, BaKind::UNARY_EXPR));
    }

    primary_expr(p)
}

/// Primary expression: literal, reference, parenthesized, property ref.
fn primary_expr(p: &mut Parser) -> Option<CompletedMarker> {
    match p.current() {
        BaKind::INT_LIT => {
            let m = p.start();
            p.bump(BaKind::INT_LIT);
            // Check for time unit suffix (identifier like ms, sec, etc.)
            if p.at(BaKind::IDENT) {
                p.bump(BaKind::IDENT);
            }
            Some(m.complete(p, BaKind::INTEGER_LITERAL))
        }
        BaKind::REAL_LIT => {
            let m = p.start();
            p.bump(BaKind::REAL_LIT);
            // Check for unit suffix
            if p.at(BaKind::IDENT) {
                p.bump(BaKind::IDENT);
            }
            Some(m.complete(p, BaKind::REAL_LITERAL))
        }
        BaKind::STRING_LIT => {
            let m = p.start();
            p.bump(BaKind::STRING_LIT);
            Some(m.complete(p, BaKind::STRING_LITERAL))
        }
        BaKind::TRUE_KW | BaKind::FALSE_KW => {
            let m = p.start();
            p.bump_any();
            Some(m.complete(p, BaKind::BOOLEAN_LITERAL))
        }
        BaKind::L_PAREN => {
            let m = p.start();
            p.bump(BaKind::L_PAREN);
            expression(p);
            p.expect(BaKind::R_PAREN);
            Some(m.complete(p, BaKind::PAREN_EXPR))
        }
        BaKind::HASH => {
            // Property reference: #PropertySet::PropName
            let m = p.start();
            p.bump(BaKind::HASH);
            qualified_name(p);
            Some(m.complete(p, BaKind::PROPERTY_REF))
        }
        BaKind::IDENT => {
            // Name reference (variable, port, dotted, indexed, port property)
            let m = p.start();
            name_reference(p);
            Some(m.complete(p, BaKind::VALUE_EXPRESSION))
        }
        // Keywords that can appear as identifiers in some contexts
        _ if p.current().is_keyword() && !is_structural_keyword(p.current()) => {
            let m = p.start();
            name_reference(p);
            Some(m.complete(p, BaKind::VALUE_EXPRESSION))
        }
        _ => {
            // Don't error here; the caller will handle EOF/unexpected tokens
            None
        }
    }
}

/// Check if a keyword is a structural keyword that cannot be used as an identifier
/// in expression context.
fn is_structural_keyword(kind: BaKind) -> bool {
    matches!(
        kind,
        BaKind::IF_KW
            | BaKind::ELSIF_KW
            | BaKind::ELSE_KW
            | BaKind::END_KW
            | BaKind::FOR_KW
            | BaKind::FORALL_KW
            | BaKind::WHILE_KW
            | BaKind::DO_KW
            | BaKind::UNTIL_KW
            | BaKind::ON_KW
            | BaKind::DISPATCH_KW
            | BaKind::VARIABLES_KW
            | BaKind::STATES_KW
            | BaKind::TRANSITIONS_KW
            | BaKind::COMPUTATION_KW
            | BaKind::STATE_KW
            | BaKind::FROZEN_KW
            | BaKind::STOP_KW
            | BaKind::OTHERWISE_KW
            | BaKind::AND_KW
            | BaKind::OR_KW
            | BaKind::XOR_KW
            | BaKind::NOT_KW
            | BaKind::ABS_KW
            | BaKind::MOD_KW
            | BaKind::REM_KW
            | BaKind::TRUE_KW
            | BaKind::FALSE_KW
            | BaKind::INITIAL_KW
            | BaKind::COMPLETE_KW
            | BaKind::FINAL_KW
    )
}
