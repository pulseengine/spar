//! EMV2 grammar rules.
//!
//! Recursive descent parser for the EMV2 annex sublanguage.
//! Specification: SAE AS5506/1 Annex E (Error Model V2)
//! Reference: OSATE2 ErrorModel.xtext (703 lines)

use super::parser::Parser;
use super::syntax_kind::Emv2Kind;

// ── Root ─────────────────────────────────────────────────────────

/// Parse EMV2 content. Auto-detects library vs subclause form.
pub(crate) fn root(p: &mut Parser) {
    let m = p.start();
    if at_library_start(p) {
        library(p);
    } else {
        subclause(p);
    }
    m.complete(p, Emv2Kind::EMV2_ROOT);
}

fn at_library_start(p: &Parser) -> bool {
    match p.current() {
        Emv2Kind::ERROR_KW => matches!(p.nth(1), Emv2Kind::TYPES_KW | Emv2Kind::BEHAVIOR_KW),
        Emv2Kind::TYPE_KW => matches!(p.nth(1), Emv2Kind::MAPPINGS_KW | Emv2Kind::TRANSFORMATIONS_KW),
        _ => false,
    }
}

// ── Library ──────────────────────────────────────────────────────

/// Parse an EMV2 library: error types, behavior state machines, mappings, transformations.
fn library(p: &mut Parser) {
    let m = p.start();

    // error types section
    if p.at(Emv2Kind::ERROR_KW) && p.nth(1) == Emv2Kind::TYPES_KW {
        error_types_section(p);
    }

    // error behavior state machines
    while p.at(Emv2Kind::ERROR_KW) && p.nth(1) == Emv2Kind::BEHAVIOR_KW {
        error_behavior_sm(p);
    }

    // type mappings
    while p.at(Emv2Kind::TYPE_KW) && p.nth(1) == Emv2Kind::MAPPINGS_KW {
        type_mapping_set(p);
    }

    // type transformations
    while p.at(Emv2Kind::TYPE_KW) && p.nth(1) == Emv2Kind::TRANSFORMATIONS_KW {
        type_transformation_set(p);
    }

    m.complete(p, Emv2Kind::EMV2_LIBRARY);
}

// ── Error Types Section ──────────────────────────────────────────

/// `error types ... end types;`
fn error_types_section(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::ERROR_KW);
    p.bump(Emv2Kind::TYPES_KW);

    // Optional: `use types Ref, Ref;`
    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TYPES_KW {
        use_types(p);
    }

    // Optional: `extends Ref, Ref with`
    if p.at(Emv2Kind::EXTENDS_KW) {
        p.bump(Emv2Kind::EXTENDS_KW);
        qemref(p);
        while p.eat(Emv2Kind::COMMA) {
            qemref(p);
        }
        p.expect(Emv2Kind::WITH_KW);
    }

    // Type definitions and type set definitions
    while p.at(Emv2Kind::IDENT) {
        type_def_or_set(p);
    }

    // Optional properties
    if p.at(Emv2Kind::PROPERTIES_KW) {
        emv2_properties_section(p);
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::TYPES_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_TYPES_SECTION);
}

/// Parse a type definition or type set definition.
/// Both start with `Name :` or `Name renames`.
fn type_def_or_set(p: &mut Parser) {
    // Peek ahead to determine form
    if p.nth(1) == Emv2Kind::COLON {
        // Name : type ...  OR  Name : type set { ... }
        if p.nth(2) == Emv2Kind::TYPE_KW && p.nth(3) == Emv2Kind::SET_KW {
            type_set_definition(p);
        } else {
            type_definition(p);
        }
    } else if p.nth(1) == Emv2Kind::RENAMES_KW {
        // Name renames type ...  OR  Name renames type set ...
        if p.nth(2) == Emv2Kind::TYPE_KW && p.nth(3) == Emv2Kind::SET_KW {
            type_set_definition(p);
        } else {
            type_definition(p);
        }
    } else {
        p.err_and_bump("expected type or type set definition");
    }
}

/// `Name : type;` or `Name : type extends SuperType;` or `Name renames type Alias;`
fn type_definition(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IDENT); // name

    if p.eat(Emv2Kind::COLON) {
        p.expect(Emv2Kind::TYPE_KW);
        if p.at(Emv2Kind::EXTENDS_KW) {
            p.bump(Emv2Kind::EXTENDS_KW);
            qemref(p);
        }
    } else if p.eat(Emv2Kind::RENAMES_KW) {
        p.expect(Emv2Kind::TYPE_KW);
        qemref(p);
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_DEFINITION);
}

/// `Name : type set { TypeRef, TypeRef };` or `Name renames type set Alias;`
fn type_set_definition(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IDENT); // name

    if p.eat(Emv2Kind::COLON) {
        p.expect(Emv2Kind::TYPE_KW);
        p.expect(Emv2Kind::SET_KW);
        type_set_constructor(p);
    } else if p.eat(Emv2Kind::RENAMES_KW) {
        p.expect(Emv2Kind::TYPE_KW);
        p.expect(Emv2Kind::SET_KW);
        qemref(p);
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_SET_DEFINITION);
}

// ── Type Set Constructor ─────────────────────────────────────────

/// `{ TypeRef * TypeRef, TypeRef }`
fn type_set_constructor(p: &mut Parser) {
    let m = p.start();
    p.expect(Emv2Kind::L_CURLY);
    if !p.at(Emv2Kind::R_CURLY) {
        type_set_element(p);
        while p.eat(Emv2Kind::COMMA) {
            type_set_element(p);
        }
    }
    p.expect(Emv2Kind::R_CURLY);
    m.complete(p, Emv2Kind::TYPE_SET_CONSTRUCTOR);
}

/// `TypeRef * TypeRef` (product type) or just `TypeRef`
fn type_set_element(p: &mut Parser) {
    let m = p.start();
    if p.at(Emv2Kind::NOERROR_KW) {
        p.bump(Emv2Kind::NOERROR_KW);
        m.complete(p, Emv2Kind::TYPE_SET_ELEMENT);
        return;
    }
    qemref(p);
    while p.eat(Emv2Kind::STAR) {
        qemref(p);
    }
    m.complete(p, Emv2Kind::TYPE_SET_ELEMENT);
}

// ── Error Behavior State Machine ─────────────────────────────────

/// `error behavior Name ... end behavior;`
fn error_behavior_sm(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::ERROR_KW);
    p.bump(Emv2Kind::BEHAVIOR_KW);
    p.expect(Emv2Kind::IDENT); // name

    // use types
    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TYPES_KW {
        use_types(p);
    }

    // use transformations
    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TRANSFORMATIONS_KW {
        let u = p.start();
        p.bump(Emv2Kind::USE_KW);
        p.bump(Emv2Kind::TRANSFORMATIONS_KW);
        qemref(p);
        p.expect(Emv2Kind::SEMICOLON);
        u.complete(p, Emv2Kind::USE_TRANSFORMATIONS);
    }

    // events
    if p.at(Emv2Kind::EVENTS_KW) {
        p.bump(Emv2Kind::EVENTS_KW);
        while at_event_start(p) {
            error_behavior_event(p);
        }
    }

    // states
    if p.at(Emv2Kind::STATES_KW) {
        p.bump(Emv2Kind::STATES_KW);
        while p.at(Emv2Kind::IDENT) {
            error_behavior_state(p);
        }
    }

    // transitions
    if p.at(Emv2Kind::TRANSITIONS_KW) {
        p.bump(Emv2Kind::TRANSITIONS_KW);
        while !p.at(Emv2Kind::END_KW) && !p.at(Emv2Kind::PROPERTIES_KW) && !p.at_end() {
            error_behavior_transition(p);
        }
    }

    // properties
    if p.at(Emv2Kind::PROPERTIES_KW) {
        emv2_properties_section(p);
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::BEHAVIOR_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_BEHAVIOR_SM);
}

fn at_event_start(p: &Parser) -> bool {
    p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON
}

/// `Name : error event {TypeSet}?;` | `Name : repair event ...;` | `Name : recover event ...;`
fn error_behavior_event(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IDENT); // name
    p.expect(Emv2Kind::COLON);

    let kind = match p.current() {
        Emv2Kind::ERROR_KW => {
            p.bump(Emv2Kind::ERROR_KW);
            p.expect(Emv2Kind::EVENT_KW);
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            if p.at(Emv2Kind::IF_KW) {
                if_condition(p);
            }
            Emv2Kind::ERROR_EVENT
        }
        Emv2Kind::REPAIR_KW => {
            p.bump(Emv2Kind::REPAIR_KW);
            p.expect(Emv2Kind::EVENT_KW);
            if p.at(Emv2Kind::WHEN_KW) {
                p.bump(Emv2Kind::WHEN_KW);
                p.expect(Emv2Kind::IDENT);
                while p.eat(Emv2Kind::COMMA) {
                    p.expect(Emv2Kind::IDENT);
                }
            }
            Emv2Kind::REPAIR_EVENT
        }
        Emv2Kind::RECOVER_KW => {
            p.bump(Emv2Kind::RECOVER_KW);
            p.expect(Emv2Kind::EVENT_KW);
            if p.at(Emv2Kind::WHEN_KW) {
                p.bump(Emv2Kind::WHEN_KW);
                p.expect(Emv2Kind::IDENT);
                while p.eat(Emv2Kind::COMMA) {
                    p.expect(Emv2Kind::IDENT);
                }
            }
            if p.at(Emv2Kind::IF_KW) {
                if_condition(p);
            }
            Emv2Kind::RECOVER_EVENT
        }
        _ => {
            p.error("expected `error`, `repair`, or `recover` event");
            Emv2Kind::ERROR
        }
    };

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, kind);
}

/// `Name : initial? state {TypeSet}?;`
fn error_behavior_state(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IDENT); // name
    p.expect(Emv2Kind::COLON);
    p.eat(Emv2Kind::INITIAL_KW);
    p.expect(Emv2Kind::STATE_KW);
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_BEHAVIOR_STATE);
}

/// `(Name :)? (Source TypeSet? | all) -[ Condition ]-> Target;`
fn error_behavior_transition(p: &mut Parser) {
    let m = p.start();

    // Optional name
    if p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON {
        p.bump(Emv2Kind::IDENT);
        p.bump(Emv2Kind::COLON);
    }

    // Source state or `all`
    if p.eat(Emv2Kind::ALL_KW) {
        // all states
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT); // state name
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }

    // -[ condition ]->
    p.expect(Emv2Kind::TRANS_OPEN);
    condition_expression(p);
    p.expect(Emv2Kind::TRANS_CLOSE);

    // Target: state, same state, or branching
    if p.at(Emv2Kind::L_PAREN) {
        // Branching: ( target with value, target with value )
        p.bump(Emv2Kind::L_PAREN);
        transition_branch(p);
        while p.eat(Emv2Kind::COMMA) {
            transition_branch(p);
        }
        p.expect(Emv2Kind::R_PAREN);
    } else if p.at(Emv2Kind::SAME_KW) {
        p.bump(Emv2Kind::SAME_KW);
        p.expect(Emv2Kind::STATE_KW);
    } else {
        // Simple target state
        if p.at(Emv2Kind::IDENT) {
            p.bump(Emv2Kind::IDENT);
        }
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_BEHAVIOR_TRANSITION);
}

/// `(State TypeSet? | same state) with BranchValue`
fn transition_branch(p: &mut Parser) {
    let m = p.start();
    if p.at(Emv2Kind::SAME_KW) {
        p.bump(Emv2Kind::SAME_KW);
        p.expect(Emv2Kind::STATE_KW);
    } else {
        if p.at(Emv2Kind::IDENT) {
            p.bump(Emv2Kind::IDENT);
        }
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }
    p.expect(Emv2Kind::WITH_KW);
    branch_value(p);
    m.complete(p, Emv2Kind::TRANSITION_BRANCH);
}

/// Real literal, property constant reference, or `others`
fn branch_value(p: &mut Parser) {
    let m = p.start();
    match p.current() {
        Emv2Kind::REAL_LIT | Emv2Kind::INT_LIT => p.bump_any(),
        Emv2Kind::OTHERS_KW => p.bump(Emv2Kind::OTHERS_KW),
        Emv2Kind::IDENT => qemref(p), // property constant
        _ => p.error("expected branch value"),
    }
    m.complete(p, Emv2Kind::BRANCH_VALUE);
}

// ── Subclause ────────────────────────────────────────────────────

fn subclause(p: &mut Parser) {
    let m = p.start();

    // use clauses
    while p.at(Emv2Kind::USE_KW) {
        use_clause(p);
    }

    // error propagations
    if p.at(Emv2Kind::ERROR_KW) && p.nth(1) == Emv2Kind::PROPAGATIONS_KW {
        error_propagations_section(p);
    }

    // component error behavior
    if p.at(Emv2Kind::COMPONENT_KW) {
        component_error_behavior(p);
    }

    // composite error behavior
    if p.at(Emv2Kind::COMPOSITE_KW) {
        composite_error_behavior(p);
    }

    // connection error
    if p.at(Emv2Kind::CONNECTION_KW) {
        connection_error(p);
    }

    // propagation paths
    if p.at(Emv2Kind::PROPAGATION_KW) && p.nth(1) == Emv2Kind::PATHS_KW {
        propagation_paths_section(p);
    }

    // properties
    if p.at(Emv2Kind::PROPERTIES_KW) {
        emv2_properties_section(p);
    }

    m.complete(p, Emv2Kind::EMV2_SUBCLAUSE);
}

// ── Use Clauses ──────────────────────────────────────────────────

fn use_clause(p: &mut Parser) {
    match p.nth(1) {
        Emv2Kind::TYPES_KW => use_types(p),
        Emv2Kind::TYPE_KW => {
            // use type equivalence Ref;
            let m = p.start();
            p.bump(Emv2Kind::USE_KW);
            p.bump(Emv2Kind::TYPE_KW);
            p.expect(Emv2Kind::EQUIVALENCE_KW);
            qemref(p);
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::USE_TYPE_EQUIVALENCE);
        }
        Emv2Kind::MAPPINGS_KW => {
            let m = p.start();
            p.bump(Emv2Kind::USE_KW);
            p.bump(Emv2Kind::MAPPINGS_KW);
            qemref(p);
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::USE_MAPPINGS);
        }
        Emv2Kind::BEHAVIOR_KW => {
            let m = p.start();
            p.bump(Emv2Kind::USE_KW);
            p.bump(Emv2Kind::BEHAVIOR_KW);
            qemref(p);
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::USE_BEHAVIOR);
        }
        Emv2Kind::TRANSFORMATIONS_KW => {
            let m = p.start();
            p.bump(Emv2Kind::USE_KW);
            p.bump(Emv2Kind::TRANSFORMATIONS_KW);
            qemref(p);
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::USE_TRANSFORMATIONS);
        }
        _ => {
            p.err_and_bump("expected `types`, `behavior`, `mappings`, or `transformations` after `use`");
        }
    }
}

/// `use types Ref, Ref;`
fn use_types(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::USE_KW);
    p.bump(Emv2Kind::TYPES_KW);
    qemref(p);
    while p.eat(Emv2Kind::COMMA) {
        qemref(p);
    }
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::USE_TYPES);
}

// ── Error Propagations ───────────────────────────────────────────

/// `error propagations ... flows ... end propagations;`
fn error_propagations_section(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::ERROR_KW);
    p.bump(Emv2Kind::PROPAGATIONS_KW);

    // error propagation declarations
    while at_propagation_start(p) {
        error_propagation(p);
    }

    // flows subsection
    if p.at(Emv2Kind::FLOWS_KW) {
        p.bump(Emv2Kind::FLOWS_KW);
        while p.at(Emv2Kind::IDENT) {
            error_flow(p);
        }
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::PROPAGATIONS_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_PROPAGATIONS_SECTION);
}

fn at_propagation_start(p: &Parser) -> bool {
    // feature name or propagation kind
    if p.at(Emv2Kind::IDENT) {
        // Check it's not a flow (name : error source/sink/path)
        if p.nth(1) == Emv2Kind::COLON
            && (p.nth(2) == Emv2Kind::IN_KW
                || p.nth(2) == Emv2Kind::OUT_KW
                || p.nth(2) == Emv2Kind::NOT_KW)
        {
            return true;
        }
        // Dotted feature reference: `feat.sub : ...`
        if p.nth(1) == Emv2Kind::DOT {
            return true;
        }
        return p.nth(1) == Emv2Kind::COLON
            && p.nth(2) != Emv2Kind::ERROR_KW;
    }
    p.current().is_propagation_kind()
}

/// `(feature | PropagationKind) : not? (in|out) propagation {TypeSet};`
fn error_propagation(p: &mut Parser) {
    let m = p.start();

    // Feature reference or propagation kind
    if p.current().is_propagation_kind() {
        p.bump_any();
    } else {
        // Feature or PP reference (possibly dotted)
        let f = p.start();
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
        f.complete(p, Emv2Kind::FEATURE_OR_PP_REF);
    }

    p.expect(Emv2Kind::COLON);
    p.eat(Emv2Kind::NOT_KW);

    // Direction: in or out
    if p.at(Emv2Kind::IN_KW) || p.at(Emv2Kind::OUT_KW) {
        p.bump_any();
    } else {
        p.error("expected `in` or `out`");
    }

    p.expect(Emv2Kind::PROPAGATION_KW);
    type_set_constructor(p);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_PROPAGATION);
}

/// Parse an error flow: source, sink, or path.
fn error_flow(p: &mut Parser) {
    // All start with: name : error (source|sink|path)
    let m = p.start();
    p.bump(Emv2Kind::IDENT); // name
    p.expect(Emv2Kind::COLON);
    p.expect(Emv2Kind::ERROR_KW);

    match p.current() {
        Emv2Kind::SOURCE_KW => {
            p.bump(Emv2Kind::SOURCE_KW);
            // source element: feature name, propagation kind, or `all`
            error_propagation_point(p);
            // optional type constraint
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            // optional `when` clause
            if p.at(Emv2Kind::WHEN_KW) {
                p.bump(Emv2Kind::WHEN_KW);
                when_clause_body(p);
            }
            // optional `if` condition
            if p.at(Emv2Kind::IF_KW) {
                if_condition(p);
            }
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::ERROR_SOURCE);
        }
        Emv2Kind::SINK_KW => {
            p.bump(Emv2Kind::SINK_KW);
            error_propagation_point(p);
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            if p.at(Emv2Kind::IF_KW) {
                if_condition(p);
            }
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::ERROR_SINK);
        }
        Emv2Kind::PATH_KW => {
            p.bump(Emv2Kind::PATH_KW);
            // incoming
            error_propagation_point(p);
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            p.expect(Emv2Kind::ARROW);
            // outgoing
            error_propagation_point(p);
            // optional target type or mappings
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            } else if p.at(Emv2Kind::USE_KW) {
                p.bump(Emv2Kind::USE_KW);
                p.expect(Emv2Kind::MAPPINGS_KW);
                qemref(p);
            }
            if p.at(Emv2Kind::IF_KW) {
                if_condition(p);
            }
            p.expect(Emv2Kind::SEMICOLON);
            m.complete(p, Emv2Kind::ERROR_PATH);
        }
        _ => {
            p.error("expected `source`, `sink`, or `path`");
            eat_to_semi(p);
            m.complete(p, Emv2Kind::ERROR);
        }
    }
}

/// Parse an error propagation point: feature path, propagation kind, or `all`.
fn error_propagation_point(p: &mut Parser) {
    if p.eat(Emv2Kind::ALL_KW) {
        return;
    }
    if p.current().is_propagation_kind() {
        p.bump_any();
        return;
    }
    // Feature path: ID (. ID)*
    if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
}

fn when_clause_body(p: &mut Parser) {
    // State reference with optional type set, or type set alone, or string
    if p.at(Emv2Kind::STRING_LIT) {
        p.bump(Emv2Kind::STRING_LIT);
    } else if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT); // state reference
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }
}

// ── Component Error Behavior ─────────────────────────────────────

/// `component error behavior ... end component;`
fn component_error_behavior(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::COMPONENT_KW);
    p.expect(Emv2Kind::ERROR_KW);
    p.expect(Emv2Kind::BEHAVIOR_KW);

    // use transformations
    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TRANSFORMATIONS_KW {
        let u = p.start();
        p.bump(Emv2Kind::USE_KW);
        p.bump(Emv2Kind::TRANSFORMATIONS_KW);
        qemref(p);
        p.expect(Emv2Kind::SEMICOLON);
        u.complete(p, Emv2Kind::USE_TRANSFORMATIONS);
    }

    // events
    if p.at(Emv2Kind::EVENTS_KW) {
        p.bump(Emv2Kind::EVENTS_KW);
        while at_event_start(p) {
            error_behavior_event(p);
        }
    }

    // transitions
    if p.at(Emv2Kind::TRANSITIONS_KW) {
        p.bump(Emv2Kind::TRANSITIONS_KW);
        while !p.at(Emv2Kind::PROPAGATIONS_KW)
            && !p.at(Emv2Kind::DETECTIONS_KW)
            && !p.at(Emv2Kind::MODE_KW)
            && !p.at(Emv2Kind::END_KW)
            && !p.at_end()
        {
            error_behavior_transition(p);
        }
    }

    // outgoing propagation conditions
    if p.at(Emv2Kind::PROPAGATIONS_KW) {
        p.bump(Emv2Kind::PROPAGATIONS_KW);
        while !p.at(Emv2Kind::DETECTIONS_KW)
            && !p.at(Emv2Kind::MODE_KW)
            && !p.at(Emv2Kind::END_KW)
            && !p.at_end()
        {
            outgoing_propagation_condition(p);
        }
    }

    // detections
    if p.at(Emv2Kind::DETECTIONS_KW) {
        p.bump(Emv2Kind::DETECTIONS_KW);
        while !p.at(Emv2Kind::MODE_KW) && !p.at(Emv2Kind::END_KW) && !p.at_end() {
            error_detection(p);
        }
    }

    // mode mappings
    if p.at(Emv2Kind::MODE_KW) && p.nth(1) == Emv2Kind::MAPPINGS_KW {
        p.bump(Emv2Kind::MODE_KW);
        p.bump(Emv2Kind::MAPPINGS_KW);
        while !p.at(Emv2Kind::END_KW) && !p.at_end() {
            error_state_to_mode_mapping(p);
        }
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::COMPONENT_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::COMPONENT_ERROR_BEHAVIOR);
}

/// `(Name :)? (State TypeSet? | all) -[ Condition? ]-> Outgoing TypeToken?;`
fn outgoing_propagation_condition(p: &mut Parser) {
    let m = p.start();

    // Optional name
    if p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON {
        p.bump(Emv2Kind::IDENT);
        p.bump(Emv2Kind::COLON);
    }

    // Source state or all
    if p.eat(Emv2Kind::ALL_KW) {
        // all
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }

    // -[ condition? ]->
    p.expect(Emv2Kind::TRANS_OPEN);
    if !p.at(Emv2Kind::TRANS_CLOSE) {
        condition_expression(p);
    }
    p.expect(Emv2Kind::TRANS_CLOSE);

    // Outgoing: feature or propagation kind or all
    error_propagation_point(p);
    // Optional type token
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::OUTGOING_PROPAGATION_CONDITION);
}

/// Error detection: `(Name :)? (State TypeSet? | all) -[ Cond? ]-> Port ! (Code)?;`
fn error_detection(p: &mut Parser) {
    let m = p.start();

    if p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON {
        p.bump(Emv2Kind::IDENT);
        p.bump(Emv2Kind::COLON);
    }

    if p.eat(Emv2Kind::ALL_KW) {
        // all
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        if p.at(Emv2Kind::L_CURLY) {
            type_set_constructor(p);
        }
    }

    p.expect(Emv2Kind::TRANS_OPEN);
    if !p.at(Emv2Kind::TRANS_CLOSE) {
        condition_expression(p);
    }
    p.expect(Emv2Kind::TRANS_CLOSE);

    // Reporting port reference
    if p.at(Emv2Kind::IDENT) {
        let r = p.start();
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            p.expect(Emv2Kind::IDENT);
        }
        r.complete(p, Emv2Kind::REPORTING_PORT_REF);
    }
    p.expect(Emv2Kind::BANG);

    // Optional error code
    if p.at(Emv2Kind::L_PAREN) {
        let c = p.start();
        p.bump(Emv2Kind::L_PAREN);
        match p.current() {
            Emv2Kind::INT_LIT => p.bump(Emv2Kind::INT_LIT),
            Emv2Kind::STRING_LIT => p.bump(Emv2Kind::STRING_LIT),
            _ => { qemref(p); }
        }
        p.expect(Emv2Kind::R_PAREN);
        c.complete(p, Emv2Kind::ERROR_CODE_VALUE);
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_DETECTION);
}

/// `State TypeToken? in modes (M1, M2);`
fn error_state_to_mode_mapping(p: &mut Parser) {
    let m = p.start();
    p.expect(Emv2Kind::IDENT); // state
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }
    p.expect(Emv2Kind::IN_KW);
    p.expect(Emv2Kind::MODE_KW); // "modes" — but it's not a keyword, handle as IDENT
    // Actually in EMV2 it's `in modes (...)`, and "modes" isn't a keyword.
    // Let me check: the grammar says `'in' 'modes' '(' ...`
    // But our lexer only has MODE_KW (singular). Let me handle this.
    // If the current token text is "modes", accept it as IDENT.
    if !p.eat(Emv2Kind::MODE_KW) {
        // Try IDENT (might be "modes" which isn't a keyword)
        if p.at(Emv2Kind::IDENT) {
            p.bump(Emv2Kind::IDENT);
        }
    }
    p.expect(Emv2Kind::L_PAREN);
    p.expect(Emv2Kind::IDENT);
    while p.eat(Emv2Kind::COMMA) {
        p.expect(Emv2Kind::IDENT);
    }
    p.expect(Emv2Kind::R_PAREN);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::ERROR_STATE_TO_MODE_MAPPING);
}

// ── Composite Error Behavior ─────────────────────────────────────

/// `composite error behavior states ... end composite;`
fn composite_error_behavior(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::COMPOSITE_KW);
    p.expect(Emv2Kind::ERROR_KW);
    p.expect(Emv2Kind::BEHAVIOR_KW);

    if p.at(Emv2Kind::STATES_KW) {
        p.bump(Emv2Kind::STATES_KW);
        while !p.at(Emv2Kind::END_KW) && !p.at_end() {
            composite_state(p);
        }
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::COMPOSITE_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::COMPOSITE_ERROR_BEHAVIOR);
}

/// `(Name :)? [ Condition | others ]-> State TypeToken?;`
fn composite_state(p: &mut Parser) {
    let m = p.start();

    // Optional name
    if p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON {
        p.bump(Emv2Kind::IDENT);
        p.bump(Emv2Kind::COLON);
    }

    p.expect(Emv2Kind::L_BRACK);
    if p.eat(Emv2Kind::OTHERS_KW) {
        // others
    } else {
        s_condition_expression(p);
    }
    p.expect(Emv2Kind::TRANS_CLOSE);

    // Target state
    if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
    }
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::COMPOSITE_STATE);
}

// ── Connection Error ─────────────────────────────────────────────

/// `connection error ... end connection;`
fn connection_error(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::CONNECTION_KW);
    p.expect(Emv2Kind::ERROR_KW);

    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TRANSFORMATIONS_KW {
        let u = p.start();
        p.bump(Emv2Kind::USE_KW);
        p.bump(Emv2Kind::TRANSFORMATIONS_KW);
        qemref(p);
        p.expect(Emv2Kind::SEMICOLON);
        u.complete(p, Emv2Kind::USE_TRANSFORMATIONS);
    }

    // Connection error sources
    while p.at(Emv2Kind::IDENT) {
        error_flow(p); // reuse error flow parsing (sources)
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::CONNECTION_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::CONNECTION_ERROR);
}

// ── Propagation Paths ────────────────────────────────────────────

/// `propagation paths ... end paths;`
fn propagation_paths_section(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::PROPAGATION_KW);
    p.bump(Emv2Kind::PATHS_KW);

    while !p.at(Emv2Kind::END_KW) && !p.at_end() {
        if p.at(Emv2Kind::IDENT) && is_propagation_point(p) {
            propagation_point(p);
        } else if p.at(Emv2Kind::IDENT) {
            propagation_path(p);
        } else {
            p.err_and_bump("expected propagation point or path");
        }
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::PATHS_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::PROPAGATION_PATHS_SECTION);
}

/// Check if we're at `Name : propagation point;`
fn is_propagation_point(p: &Parser) -> bool {
    p.at(Emv2Kind::IDENT)
        && p.nth(1) == Emv2Kind::COLON
        && p.nth(2) == Emv2Kind::PROPAGATION_KW
        && p.nth(3) == Emv2Kind::POINT_KW
}

/// `Name : propagation point;`
fn propagation_point(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IDENT);
    p.expect(Emv2Kind::COLON);
    p.expect(Emv2Kind::PROPAGATION_KW);
    p.expect(Emv2Kind::POINT_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::PROPAGATION_POINT);
}

/// `(Name :)? Source -> Target;`
fn propagation_path(p: &mut Parser) {
    let m = p.start();

    // Optional name prefix (Name :)
    // Tricky: need to distinguish `Name : Source -> Target` from `Source -> Target`
    // where Source is `sub.point`. If nth(1) is COLON and nth(2) is not PROPAGATION,
    // it might be a named path.
    if p.at(Emv2Kind::IDENT) && p.nth(1) == Emv2Kind::COLON
        && p.nth(2) != Emv2Kind::PROPAGATION_KW
    {
        // Could be `Name : Source -> Target` or just `Source -> Target` where Source has `.`
        // Check if there's an `->` ahead suggesting it's `Source -> Target`
        // For simplicity, if nth(2) is IDENT and there's a DOT or ARROW ahead, treat as path
        p.bump(Emv2Kind::IDENT);
        p.bump(Emv2Kind::COLON);
    }

    // Qualified propagation point: sub.sub.point
    qualified_propagation_point(p);
    p.expect(Emv2Kind::ARROW);
    qualified_propagation_point(p);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::PROPAGATION_PATH_DECL);
}

/// `sub.sub.point` or just `point`
fn qualified_propagation_point(p: &mut Parser) {
    let m = p.start();
    if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    m.complete(p, Emv2Kind::QUALIFIED_PROPAGATION_POINT);
}

// ── Type Mappings ────────────────────────────────────────────────

/// `type mappings Name ... end mappings;`
fn type_mapping_set(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::TYPE_KW);
    p.bump(Emv2Kind::MAPPINGS_KW);
    p.expect(Emv2Kind::IDENT); // name

    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TYPES_KW {
        use_types(p);
    }

    while p.at(Emv2Kind::L_CURLY) {
        type_mapping(p);
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::MAPPINGS_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_MAPPING_SET);
}

/// `{source} -> {target};`
fn type_mapping(p: &mut Parser) {
    let m = p.start();
    type_set_constructor(p);
    p.expect(Emv2Kind::ARROW);
    type_set_constructor(p);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_MAPPING);
}

// ── Type Transformations ─────────────────────────────────────────

/// `type transformations Name ... end transformations;`
fn type_transformation_set(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::TYPE_KW);
    p.bump(Emv2Kind::TRANSFORMATIONS_KW);
    p.expect(Emv2Kind::IDENT); // name

    if p.at(Emv2Kind::USE_KW) && p.nth(1) == Emv2Kind::TYPES_KW {
        use_types(p);
    }

    while !p.at(Emv2Kind::END_KW) && !p.at_end() {
        type_transformation(p);
    }

    p.expect(Emv2Kind::END_KW);
    p.expect(Emv2Kind::TRANSFORMATIONS_KW);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_TRANSFORMATION_SET);
}

/// `({source} | all) -[ {contributor}? ]-> {target};`
fn type_transformation(p: &mut Parser) {
    let m = p.start();
    if p.eat(Emv2Kind::ALL_KW) {
        // all sources
    } else {
        type_set_constructor(p);
    }
    p.expect(Emv2Kind::TRANS_OPEN);
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }
    p.expect(Emv2Kind::TRANS_CLOSE);
    type_set_constructor(p);
    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::TYPE_TRANSFORMATION);
}

// ── Condition Expressions ────────────────────────────────────────
// For component error behavior transitions/conditions.

/// `AndExpr (or AndExpr)*`
fn condition_expression(p: &mut Parser) {
    and_expression(p);
    while p.at(Emv2Kind::OR_KW) {
        let m = p.start();
        p.bump(Emv2Kind::OR_KW);
        and_expression(p);
        m.complete(p, Emv2Kind::OR_EXPRESSION);
    }
}

/// `ConditionTerm (and ConditionTerm)*`
fn and_expression(p: &mut Parser) {
    condition_term(p);
    while p.at(Emv2Kind::AND_KW) {
        let m = p.start();
        p.bump(Emv2Kind::AND_KW);
        condition_term(p);
        m.complete(p, Emv2Kind::AND_EXPRESSION);
    }
}

/// ConditionElement | OrmoreExpr | OrlessExpr | AllExpr | (ConditionExpr)
fn condition_term(p: &mut Parser) {
    match p.current() {
        Emv2Kind::INT_LIT => {
            // ormore or orless
            let m = p.start();
            p.bump(Emv2Kind::INT_LIT);
            if p.at(Emv2Kind::ORMORE_KW) {
                p.bump(Emv2Kind::ORMORE_KW);
                p.expect(Emv2Kind::L_PAREN);
                condition_expression(p);
                while p.eat(Emv2Kind::COMMA) {
                    condition_expression(p);
                }
                p.expect(Emv2Kind::R_PAREN);
                m.complete(p, Emv2Kind::ORMORE_EXPRESSION);
            } else if p.at(Emv2Kind::ORLESS_KW) {
                p.bump(Emv2Kind::ORLESS_KW);
                p.expect(Emv2Kind::L_PAREN);
                condition_expression(p);
                while p.eat(Emv2Kind::COMMA) {
                    condition_expression(p);
                }
                p.expect(Emv2Kind::R_PAREN);
                m.complete(p, Emv2Kind::ORLESS_EXPRESSION);
            } else {
                p.error("expected `ormore` or `orless`");
                m.complete(p, Emv2Kind::ERROR);
            }
        }
        Emv2Kind::ALL_KW => {
            let m = p.start();
            p.bump(Emv2Kind::ALL_KW);
            // Optional `- count`
            if p.eat(Emv2Kind::MINUS) {
                p.expect(Emv2Kind::INT_LIT);
            }
            p.expect(Emv2Kind::L_PAREN);
            condition_expression(p);
            while p.eat(Emv2Kind::COMMA) {
                condition_expression(p);
            }
            p.expect(Emv2Kind::R_PAREN);
            m.complete(p, Emv2Kind::ALL_EXPRESSION);
        }
        Emv2Kind::L_PAREN => {
            p.bump(Emv2Kind::L_PAREN);
            condition_expression(p);
            p.expect(Emv2Kind::R_PAREN);
        }
        _ => {
            // ConditionElement: qualified reference with optional type constraint
            condition_element(p);
        }
    }
}

/// QualifiedErrorEventOrPropagation TypeTokenConstraint?
fn condition_element(p: &mut Parser) {
    let m = p.start();
    // Reference: propagation kind or ID.ID.ID path
    if p.current().is_propagation_kind() {
        p.bump_any();
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    // Optional type constraint
    if p.at(Emv2Kind::L_CURLY) {
        type_set_constructor(p);
    }
    m.complete(p, Emv2Kind::CONDITION_ELEMENT);
}

// ── Composite Condition Expressions ──────────────────────────────
// For composite error behavior states (reference subcomponent states).

/// `SAndExpr (or SAndExpr)*`
fn s_condition_expression(p: &mut Parser) {
    s_and_expression(p);
    while p.at(Emv2Kind::OR_KW) {
        let m = p.start();
        p.bump(Emv2Kind::OR_KW);
        s_and_expression(p);
        m.complete(p, Emv2Kind::OR_EXPRESSION);
    }
}

/// `SConditionTerm (and SConditionTerm)*`
fn s_and_expression(p: &mut Parser) {
    s_condition_term(p);
    while p.at(Emv2Kind::AND_KW) {
        let m = p.start();
        p.bump(Emv2Kind::AND_KW);
        s_condition_term(p);
        m.complete(p, Emv2Kind::AND_EXPRESSION);
    }
}

fn s_condition_term(p: &mut Parser) {
    match p.current() {
        Emv2Kind::INT_LIT => {
            let m = p.start();
            p.bump(Emv2Kind::INT_LIT);
            if p.at(Emv2Kind::ORMORE_KW) {
                p.bump(Emv2Kind::ORMORE_KW);
                p.expect(Emv2Kind::L_PAREN);
                s_condition_expression(p);
                while p.eat(Emv2Kind::COMMA) {
                    s_condition_expression(p);
                }
                p.expect(Emv2Kind::R_PAREN);
                m.complete(p, Emv2Kind::ORMORE_EXPRESSION);
            } else if p.at(Emv2Kind::ORLESS_KW) {
                p.bump(Emv2Kind::ORLESS_KW);
                p.expect(Emv2Kind::L_PAREN);
                s_condition_expression(p);
                while p.eat(Emv2Kind::COMMA) {
                    s_condition_expression(p);
                }
                p.expect(Emv2Kind::R_PAREN);
                m.complete(p, Emv2Kind::ORLESS_EXPRESSION);
            } else {
                p.error("expected `ormore` or `orless`");
                m.complete(p, Emv2Kind::ERROR);
            }
        }
        Emv2Kind::ALL_KW => {
            let m = p.start();
            p.bump(Emv2Kind::ALL_KW);
            if p.eat(Emv2Kind::MINUS) {
                p.expect(Emv2Kind::INT_LIT);
            }
            p.expect(Emv2Kind::L_PAREN);
            s_condition_expression(p);
            while p.eat(Emv2Kind::COMMA) {
                s_condition_expression(p);
            }
            p.expect(Emv2Kind::R_PAREN);
            m.complete(p, Emv2Kind::ALL_EXPRESSION);
        }
        Emv2Kind::L_PAREN => {
            p.bump(Emv2Kind::L_PAREN);
            s_condition_expression(p);
            p.expect(Emv2Kind::R_PAREN);
        }
        Emv2Kind::IN_KW => {
            // `in` qualified error propagation
            let m = p.start();
            p.bump(Emv2Kind::IN_KW);
            // propagation kind or path
            if p.current().is_propagation_kind() {
                p.bump_any();
            } else if p.at(Emv2Kind::IDENT) {
                p.bump(Emv2Kind::IDENT);
                while p.eat(Emv2Kind::DOT) {
                    if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                        p.bump_any();
                    }
                }
            }
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            m.complete(p, Emv2Kind::S_CONDITION_ELEMENT);
        }
        _ => {
            // Qualified error behavior state: sub.sub.state
            let m = p.start();
            if p.at(Emv2Kind::IDENT) {
                p.bump(Emv2Kind::IDENT);
                while p.eat(Emv2Kind::DOT) {
                    if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                        p.bump_any();
                    }
                }
            }
            if p.at(Emv2Kind::L_CURLY) {
                type_set_constructor(p);
            }
            m.complete(p, Emv2Kind::S_CONDITION_ELEMENT);
        }
    }
}

// ── Properties ───────────────────────────────────────────────────

fn emv2_properties_section(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::PROPERTIES_KW);
    while p.at(Emv2Kind::IDENT) && !p.at(Emv2Kind::END_KW) {
        emv2_property_association(p);
    }
    m.complete(p, Emv2Kind::EMV2_PROPERTIES_SECTION);
}

/// `property => value (applies to path)?;`
fn emv2_property_association(p: &mut Parser) {
    let m = p.start();
    // Property reference: QPREF (Name or PkgName::PropName)
    qemref(p);
    p.expect(Emv2Kind::FAT_ARROW);

    // Property value — simplified: consume until `;` or `applies`
    eat_property_value(p);

    // Optional `applies to` clause
    if p.at(Emv2Kind::APPLIES_KW) {
        p.bump(Emv2Kind::APPLIES_KW);
        p.expect(Emv2Kind::TO_KW);
        emv2_path(p);
        while p.eat(Emv2Kind::COMMA) {
            emv2_path(p);
        }
    }

    p.expect(Emv2Kind::SEMICOLON);
    m.complete(p, Emv2Kind::EMV2_PROPERTY_ASSOCIATION);
}

/// Consume a property value expression (simplified — stops at `;` or `applies`).
fn eat_property_value(p: &mut Parser) {
    let mut depth = 0i32;
    while !p.at_end() {
        match p.current() {
            Emv2Kind::SEMICOLON if depth == 0 => break,
            Emv2Kind::APPLIES_KW if depth == 0 => break,
            Emv2Kind::L_PAREN | Emv2Kind::L_CURLY | Emv2Kind::L_BRACK => {
                depth += 1;
                p.bump_any();
            }
            Emv2Kind::R_PAREN | Emv2Kind::R_CURLY | Emv2Kind::R_BRACK => {
                depth -= 1;
                p.bump_any();
            }
            _ => p.bump_any(),
        }
    }
}

/// EMV2 path: `(^ ContainmentPath @)? PathElement`
fn emv2_path(p: &mut Parser) {
    let m = p.start();
    if p.eat(Emv2Kind::CARET) {
        // containment path
        if p.at(Emv2Kind::IDENT) {
            p.bump(Emv2Kind::IDENT);
            while p.eat(Emv2Kind::DOT) {
                p.expect(Emv2Kind::IDENT);
            }
        }
        p.expect(Emv2Kind::AT);
    }
    // EMV2 path element or kind
    if p.current().is_propagation_kind() {
        p.bump_any();
        if p.eat(Emv2Kind::DOT) {
            p.expect(Emv2Kind::IDENT);
        }
    } else if p.at(Emv2Kind::IDENT) {
        p.bump(Emv2Kind::IDENT);
        while p.eat(Emv2Kind::DOT) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            }
        }
    }
    m.complete(p, Emv2Kind::EMV2_PATH);
}

// ── Shared helpers ───────────────────────────────────────────────

/// Parse a qualified EMV2 reference: `ID (:: ID)*`
fn qemref(p: &mut Parser) {
    let m = p.start();
    if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
        p.bump_any();
        while p.eat(Emv2Kind::COLON_COLON) {
            if p.at(Emv2Kind::IDENT) || p.current().is_keyword() {
                p.bump_any();
            } else {
                p.error("expected identifier after `::`");
            }
        }
    } else {
        p.error("expected qualified reference");
    }
    m.complete(p, Emv2Kind::QEMREF);
}

/// `if` condition: `if "description"` or `if FunctionRef`
fn if_condition(p: &mut Parser) {
    let m = p.start();
    p.bump(Emv2Kind::IF_KW);
    if p.at(Emv2Kind::STRING_LIT) {
        p.bump(Emv2Kind::STRING_LIT);
    } else if p.at(Emv2Kind::IDENT) {
        qemref(p);
    }
    m.complete(p, Emv2Kind::IF_CONDITION);
}

fn eat_to_semi(p: &mut Parser) {
    while !p.at(Emv2Kind::SEMICOLON) && !p.at_end() {
        p.bump_any();
    }
    if p.at(Emv2Kind::SEMICOLON) {
        p.bump(Emv2Kind::SEMICOLON);
    }
}
