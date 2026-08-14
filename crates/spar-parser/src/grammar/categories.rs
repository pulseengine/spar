//! Which component categories may contain which.
//!
//! # Provenance: OSATE 2.18.0 — a TEST, not the authority
//!
//! Read this header before trusting the table. It records **what one
//! implementation does**, not what the standard says.
//!
//! The authority is AS5506; OSATE is the reference implementation and the most
//! useful oracle available, but it is still an implementation and it can be
//! wrong. Where the two would disagree, the standard wins and OSATE's verdict
//! becomes a finding *about OSATE*. This table has NOT been checked against the
//! AS5506 text — the document is paywalled and the freely available samples
//! cover §9 (connections), not §4.5. So every entry below is properly read as
//! "OSATE 2.18.0 accepts/rejects this", and the pairs listed under
//! `UNADJUDICATED` are the ones where that gap plausibly matters.
//!
//! Every one of the 196 (container, subcomponent) pairs was put to OSATE
//! 2.18.0 as a minimal probe package:
//!
//! ```text
//! package P
//! public
//!   <container> Container
//!   end Container;
//!   <container> implementation Container.i
//!     subcomponents
//!       x : <sub>;
//!   end Container.i;
//! end P;
//! ```
//!
//! 196/196 returned a verdict: **72 accepted, 124 rejected**. This table is
//! exactly that result.
//!
//! It was then cross-checked the other way: all 28 distinct pairs occurring in
//! the 548-file vendored OSATE corpus (1346 implementations) appear in the
//! legal set here. That direction matters more than it looks — a table
//! *stricter* than the standard would make spar reject valid AADL, which is
//! the one failure direction `osate_agreement.rs` treats as an invariant
//! rather than a ratchet. Deriving it from the oracle rather than from memory
//! is what keeps that cell empty.
//!
//! # Why the parser and not a later stage
//!
//! Because that is where OSATE enforces it. Its Xtext grammar has per-category
//! subcomponent rules (`ProcessSubcomponentType` and friends), so a thread
//! inside a system implementation fails as `no viable alternative at input
//! 'thread'` — a syntax error, not a validation one. Matching that placement
//! keeps `spar parse` comparable to OSATE's parse, which is the comparison
//! the agreement matrix is built on.
//!
//! # UNADJUDICATED: where OSATE may be wrong
//!
//! These came back REJECT, and the AADL literature commonly describes them as
//! permitted. They are encoded as OSATE reports them, because following the
//! oracle we actually ran beats following a half-remembered table — but they
//! are the first place to look if a user reports spar refusing a valid model,
//! and they should be checked against the AS5506 text when someone has it:
//!
//!   * `data` containing `subprogram group`
//!   * `subprogram` containing `subprogram group`
//!
//! Both are *rejections*, so encoding them makes spar stricter, which is the
//! direction that can produce false errors for users. Neither appears in the
//! 548-file corpus, so there is no evidence either way from that quarter. If
//! the standard permits them, delete them from the illegal set — do not wait
//! for OSATE to change.
//!
//! Conversely `subprogram` containing `subprogram`, and `data` containing
//! `data`, came back ACCEPT and are encoded as legal. If the standard forbids
//! those, spar is currently too lenient there and the fix is ours to make,
//! regardless of what OSATE does.
//!
//! Addresses #420 category 1. See `tools/osate-conformance/README.md` to
//! regenerate the probe.

use crate::syntax_kind::SyntaxKind;

/// The 14 AADL component categories.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Category {
    Abstract,
    Bus,
    Data,
    Device,
    Memory,
    Process,
    Processor,
    Subprogram,
    SubprogramGroup,
    System,
    Thread,
    ThreadGroup,
    VirtualBus,
    VirtualProcessor,
}

impl Category {
    /// As it appears in source, for diagnostics.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Category::Abstract => "abstract",
            Category::Bus => "bus",
            Category::Data => "data",
            Category::Device => "device",
            Category::Memory => "memory",
            Category::Process => "process",
            Category::Processor => "processor",
            Category::Subprogram => "subprogram",
            Category::SubprogramGroup => "subprogram group",
            Category::System => "system",
            Category::Thread => "thread",
            Category::ThreadGroup => "thread group",
            Category::VirtualBus => "virtual bus",
            Category::VirtualProcessor => "virtual processor",
        }
    }
}

/// Read the category at the cursor WITHOUT consuming it.
///
/// Two categories are two tokens (`virtual bus`, `virtual processor`) and two
/// more are optionally two (`thread group`, `subprogram group`), so a
/// single-token peek would classify `thread group` as `thread` and reject
/// `thread group` inside a process — a false rejection, the direction that
/// must stay empty.
pub(crate) fn peek(current: SyntaxKind, next: SyntaxKind) -> Option<Category> {
    Some(match current {
        SyntaxKind::ABSTRACT_KW => Category::Abstract,
        SyntaxKind::BUS_KW => Category::Bus,
        SyntaxKind::DATA_KW => Category::Data,
        SyntaxKind::DEVICE_KW => Category::Device,
        SyntaxKind::MEMORY_KW => Category::Memory,
        SyntaxKind::PROCESS_KW => Category::Process,
        SyntaxKind::PROCESSOR_KW => Category::Processor,
        SyntaxKind::SYSTEM_KW => Category::System,
        SyntaxKind::THREAD_KW if next == SyntaxKind::GROUP_KW => Category::ThreadGroup,
        SyntaxKind::THREAD_KW => Category::Thread,
        SyntaxKind::SUBPROGRAM_KW if next == SyntaxKind::GROUP_KW => Category::SubprogramGroup,
        SyntaxKind::SUBPROGRAM_KW => Category::Subprogram,
        SyntaxKind::VIRTUAL_KW if next == SyntaxKind::BUS_KW => Category::VirtualBus,
        SyntaxKind::VIRTUAL_KW if next == SyntaxKind::PROCESSOR_KW => Category::VirtualProcessor,
        _ => return None,
    })
}

/// May `container` hold a subcomponent of category `sub`?
///
/// The 72 accepted pairs, verbatim from the OSATE probe.
pub(crate) fn may_contain(container: Category, sub: Category) -> bool {
    use Category::*;
    let allowed: &[Category] = match container {
        // `abstract` is the wildcard: it may contain every category.
        Abstract => &[
            Abstract,
            Bus,
            Data,
            Device,
            Memory,
            Process,
            Processor,
            Subprogram,
            SubprogramGroup,
            System,
            Thread,
            ThreadGroup,
            VirtualBus,
            VirtualProcessor,
        ],
        Bus => &[Abstract, VirtualBus],
        Data => &[Abstract, Data, Subprogram],
        Device => &[Abstract, Bus, Data, VirtualBus],
        Memory => &[Abstract, Bus, Memory, VirtualBus],
        Process => &[
            Abstract,
            Data,
            Subprogram,
            SubprogramGroup,
            Thread,
            ThreadGroup,
        ],
        Processor => &[Abstract, Bus, Memory, VirtualBus, VirtualProcessor],
        Subprogram => &[Abstract, Data, Subprogram],
        SubprogramGroup => &[Abstract, Data, Subprogram, SubprogramGroup],
        // Note the absences: NOT thread, NOT thread group. Threads live in a
        // process. 0 occurrences across 666 system implementations in OSATE's
        // corpus, and the probe rejects both.
        System => &[
            Abstract,
            Bus,
            Data,
            Device,
            Memory,
            Process,
            Processor,
            Subprogram,
            SubprogramGroup,
            System,
            VirtualBus,
            VirtualProcessor,
        ],
        Thread => &[Abstract, Data, Subprogram, SubprogramGroup],
        ThreadGroup => &[
            Abstract,
            Data,
            Subprogram,
            SubprogramGroup,
            Thread,
            ThreadGroup,
        ],
        VirtualBus => &[Abstract, VirtualBus],
        VirtualProcessor => &[Abstract, VirtualBus, VirtualProcessor],
    };
    allowed.contains(&sub)
}

/// Feature kinds whose legality depends on the containing component category.
///
/// Only two are encoded, out of the 14 probed. That is deliberate: these are
/// the two where a SECOND, independent signal agrees with OSATE. Rejecting a
/// feature makes spar stricter, which is the direction that invents errors for
/// users, so single-sourced rejections are not worth the risk yet.
///
/// | rule | corpus (3877 type declarations) | OSATE probe |
/// |------|--------------------------------|-------------|
/// | bus access | system 53, device 30, processor 18, memory 3, bus 3, abstract 1 — never on process/thread | process, thread, thread group, data, subprogram(-group) REJECT |
/// | parameter | subprogram 59, nothing else | every category except subprogram REJECTS |
///
/// The corpus is data and the probe is code, so these are closer to
/// independent than two runs of the same validator would be.
///
/// The other 12 probed feature kinds are measured but NOT encoded — see
/// `tools/osate-conformance/README.md`. Encoding them needs either the AS5506
/// text or a corroborating signal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FeatureKind {
    /// `requires bus access` / `provides bus access`
    BusAccess,
    /// `in parameter` / `out parameter`
    Parameter,
}

impl FeatureKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            FeatureKind::BusAccess => "bus access",
            FeatureKind::Parameter => "parameter",
        }
    }
}

/// May a component of `container` declare a feature of this kind?
pub(crate) fn may_have_feature(container: Category, kind: FeatureKind) -> bool {
    use Category::*;
    match kind {
        // Bus access is a platform concern. Software categories bind to a
        // processor; they do not touch a bus directly.
        FeatureKind::BusAccess => matches!(
            container,
            Abstract | Bus | Device | Memory | Processor | System | VirtualBus | VirtualProcessor
        ),
        // Parameters are the subprogram call interface, and nothing else --
        // not even `abstract`, which is otherwise the permissive wildcard.
        FeatureKind::Parameter => matches!(container, Subprogram),
    }
}

#[cfg(test)]
mod tests {
    use super::Category::*;
    use super::*;

    #[test]
    fn the_table_has_exactly_the_72_pairs_osate_accepted() {
        const ALL: [Category; 14] = [
            Abstract,
            Bus,
            Data,
            Device,
            Memory,
            Process,
            Processor,
            Subprogram,
            SubprogramGroup,
            System,
            Thread,
            ThreadGroup,
            VirtualBus,
            VirtualProcessor,
        ];
        let n = ALL
            .iter()
            .flat_map(|c| ALL.iter().map(move |s| (*c, *s)))
            .filter(|(c, s)| may_contain(*c, *s))
            .count();
        // The probe returned 72 accepted of 196. A table drifting in either
        // direction changes this count, and the too-strict direction is the
        // one that breaks the agreement invariant.
        assert_eq!(n, 72, "table must hold exactly the 72 OSATE-accepted pairs");
    }

    #[test]
    fn a_thread_may_not_sit_directly_in_a_system() {
        // The concrete #420 finding: test-data/parser/complex_system.aadl:47
        // declares `voter : thread Voter;` inside `system implementation
        // FMC.dual`, which spar accepted and OSATE refuses.
        assert!(!may_contain(System, Thread));
        assert!(!may_contain(System, ThreadGroup));
        // The discriminating partner: the same thread inside a process is
        // fine, so the rule is about the PAIR, not about threads.
        assert!(may_contain(Process, Thread));
        // ...and a system may still hold a process.
        assert!(may_contain(System, Process));
    }

    #[test]
    fn bus_access_is_a_platform_feature_and_parameters_are_subprogram_only() {
        // #420: `requires bus access` on a process type. Both signals agree --
        // the probe rejects it, and the 548-file corpus puts bus access on
        // system/device/processor/memory/bus/abstract and never on a process.
        assert!(!may_have_feature(Process, FeatureKind::BusAccess));
        assert!(!may_have_feature(Thread, FeatureKind::BusAccess));
        assert!(!may_have_feature(ThreadGroup, FeatureKind::BusAccess));
        // The discriminating partners: the same feature on the platform side.
        assert!(may_have_feature(System, FeatureKind::BusAccess));
        assert!(may_have_feature(Device, FeatureKind::BusAccess));
        assert!(may_have_feature(Processor, FeatureKind::BusAccess));

        // Parameters: subprogram alone. Note `abstract` is NOT permitted here
        // even though it is the wildcard for subcomponents -- so a rule
        // written as "abstract allows everything" would be wrong.
        assert!(may_have_feature(Subprogram, FeatureKind::Parameter));
        assert!(!may_have_feature(Abstract, FeatureKind::Parameter));
        assert!(!may_have_feature(Thread, FeatureKind::Parameter));
        assert!(!may_have_feature(System, FeatureKind::Parameter));
    }

    #[test]
    fn two_token_categories_are_not_truncated_to_their_first_token() {
        // `thread group` inside a process is legal; `thread` inside a system
        // is not. A peek reading only the first token conflates them and
        // would reject valid AADL.
        assert_eq!(
            peek(SyntaxKind::THREAD_KW, SyntaxKind::GROUP_KW),
            Some(ThreadGroup)
        );
        assert_eq!(peek(SyntaxKind::THREAD_KW, SyntaxKind::IDENT), Some(Thread));
        assert_eq!(
            peek(SyntaxKind::VIRTUAL_KW, SyntaxKind::PROCESSOR_KW),
            Some(VirtualProcessor)
        );
        assert_eq!(
            peek(SyntaxKind::VIRTUAL_KW, SyntaxKind::BUS_KW),
            Some(VirtualBus)
        );
        // A processor may hold a VIRTUAL processor but not a plain one.
        assert!(may_contain(Processor, VirtualProcessor));
        assert!(!may_contain(Processor, Processor));
        assert_eq!(peek(SyntaxKind::IDENT, SyntaxKind::IDENT), None);
    }
}
