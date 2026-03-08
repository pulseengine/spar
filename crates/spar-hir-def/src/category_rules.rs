//! Category restriction tables from AS5506 §4.5, §5–6.
//!
//! These tables define, for each of the 14 AADL component categories,
//! which feature kinds are permitted and which subcomponent categories
//! may appear in implementations.

use crate::item_tree::{ComponentCategory, FeatureKind};

use ComponentCategory::*;
use FeatureKind::*;

// ── Feature-kind constants ───────────────────────────────────────────

/// All ten feature kinds.
const ALL_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    Parameter,
    DataAccess,
    BusAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

/// Common feature set: data/event/event-data port + feature group + abstract feature.
const PORTS_FG_ABS: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    FeatureGroup,
    AbstractFeature,
];

// ── Allowed features per category (AS5506 §5–6) ─────────────────────

const SYSTEM_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    BusAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const PROCESS_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const THREAD_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
    Parameter,
];

const THREAD_GROUP_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const PROCESSOR_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    BusAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const VIRTUAL_PROCESSOR_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const MEMORY_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    BusAccess,
    FeatureGroup,
    AbstractFeature,
];

const BUS_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    BusAccess,
    FeatureGroup,
    AbstractFeature,
];

const VIRTUAL_BUS_FEATURES: &[FeatureKind] = PORTS_FG_ABS;

const DEVICE_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    BusAccess,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const SUBPROGRAM_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    SubprogramAccess,
    FeatureGroup,
    AbstractFeature,
    Parameter,
];

const SUBPROGRAM_GROUP_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    SubprogramAccess,
    SubprogramGroupAccess,
    FeatureGroup,
    AbstractFeature,
];

const DATA_FEATURES: &[FeatureKind] = &[
    DataPort,
    EventPort,
    EventDataPort,
    DataAccess,
    SubprogramAccess,
    FeatureGroup,
    AbstractFeature,
];

// ── Allowed subcomponents per category (AS5506 §4.5) ─────────────────

const ALL_CATEGORIES: &[ComponentCategory] = &[
    System,
    Process,
    Thread,
    ThreadGroup,
    Processor,
    VirtualProcessor,
    Memory,
    Bus,
    VirtualBus,
    Device,
    Subprogram,
    SubprogramGroup,
    Data,
    Abstract,
];

const SYSTEM_SUBCOMPONENTS: &[ComponentCategory] = &[
    System,
    Process,
    Device,
    Memory,
    Bus,
    Processor,
    VirtualProcessor,
    VirtualBus,
    Data,
    Abstract,
];

const PROCESS_SUBCOMPONENTS: &[ComponentCategory] = &[
    Thread,
    ThreadGroup,
    Data,
    Subprogram,
    SubprogramGroup,
    Abstract,
];

const THREAD_SUBCOMPONENTS: &[ComponentCategory] = &[Data, Subprogram, Abstract];

const THREAD_GROUP_SUBCOMPONENTS: &[ComponentCategory] = &[
    Thread,
    ThreadGroup,
    Data,
    Subprogram,
    Abstract,
];

const PROCESSOR_SUBCOMPONENTS: &[ComponentCategory] = &[
    Memory,
    Bus,
    VirtualProcessor,
    VirtualBus,
    Abstract,
];

const VIRTUAL_PROCESSOR_SUBCOMPONENTS: &[ComponentCategory] =
    &[VirtualProcessor, VirtualBus, Abstract];

const MEMORY_SUBCOMPONENTS: &[ComponentCategory] = &[Memory, Bus, Abstract];

const BUS_SUBCOMPONENTS: &[ComponentCategory] = &[VirtualBus, Abstract];

const VIRTUAL_BUS_SUBCOMPONENTS: &[ComponentCategory] = &[VirtualBus, Abstract];

const DEVICE_SUBCOMPONENTS: &[ComponentCategory] = &[Bus, VirtualBus, Data, Abstract];

const SUBPROGRAM_SUBCOMPONENTS: &[ComponentCategory] = &[Data, Abstract];

const SUBPROGRAM_GROUP_SUBCOMPONENTS: &[ComponentCategory] =
    &[Subprogram, SubprogramGroup, Data, Abstract];

const DATA_SUBCOMPONENTS: &[ComponentCategory] = &[Data, Subprogram, Abstract];

// ── Public API ───────────────────────────────────────────────────────

/// Returns the list of feature kinds allowed for a given component category.
pub fn allowed_features(category: ComponentCategory) -> &'static [FeatureKind] {
    match category {
        System => SYSTEM_FEATURES,
        Process => PROCESS_FEATURES,
        Thread => THREAD_FEATURES,
        ThreadGroup => THREAD_GROUP_FEATURES,
        Processor => PROCESSOR_FEATURES,
        VirtualProcessor => VIRTUAL_PROCESSOR_FEATURES,
        Memory => MEMORY_FEATURES,
        Bus => BUS_FEATURES,
        VirtualBus => VIRTUAL_BUS_FEATURES,
        Device => DEVICE_FEATURES,
        Subprogram => SUBPROGRAM_FEATURES,
        SubprogramGroup => SUBPROGRAM_GROUP_FEATURES,
        Data => DATA_FEATURES,
        Abstract => ALL_FEATURES,
    }
}

/// Returns the list of subcomponent categories allowed for a given component category.
pub fn allowed_subcomponents(category: ComponentCategory) -> &'static [ComponentCategory] {
    match category {
        System => SYSTEM_SUBCOMPONENTS,
        Process => PROCESS_SUBCOMPONENTS,
        Thread => THREAD_SUBCOMPONENTS,
        ThreadGroup => THREAD_GROUP_SUBCOMPONENTS,
        Processor => PROCESSOR_SUBCOMPONENTS,
        VirtualProcessor => VIRTUAL_PROCESSOR_SUBCOMPONENTS,
        Memory => MEMORY_SUBCOMPONENTS,
        Bus => BUS_SUBCOMPONENTS,
        VirtualBus => VIRTUAL_BUS_SUBCOMPONENTS,
        Device => DEVICE_SUBCOMPONENTS,
        Subprogram => SUBPROGRAM_SUBCOMPONENTS,
        SubprogramGroup => SUBPROGRAM_GROUP_SUBCOMPONENTS,
        Data => DATA_SUBCOMPONENTS,
        Abstract => ALL_CATEGORIES,
    }
}

/// Check if a feature kind is allowed in a given component category.
pub fn is_feature_allowed(category: ComponentCategory, feature: FeatureKind) -> bool {
    allowed_features(category).contains(&feature)
}

/// Check if a subcomponent category is allowed in a given parent category.
pub fn is_subcomponent_allowed(parent: ComponentCategory, child: ComponentCategory) -> bool {
    allowed_subcomponents(parent).contains(&child)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Feature count tests ──────────────────────────────────────────

    #[test]
    fn system_has_9_features() {
        assert_eq!(allowed_features(System).len(), 9);
    }

    #[test]
    fn process_has_8_features() {
        assert_eq!(allowed_features(Process).len(), 8);
    }

    #[test]
    fn thread_has_9_features() {
        assert_eq!(allowed_features(Thread).len(), 9);
    }

    #[test]
    fn thread_group_has_8_features() {
        assert_eq!(allowed_features(ThreadGroup).len(), 8);
    }

    #[test]
    fn processor_has_8_features() {
        assert_eq!(allowed_features(Processor).len(), 8);
    }

    #[test]
    fn virtual_processor_has_7_features() {
        assert_eq!(allowed_features(VirtualProcessor).len(), 7);
    }

    #[test]
    fn memory_has_6_features() {
        assert_eq!(allowed_features(Memory).len(), 6);
    }

    #[test]
    fn bus_has_6_features() {
        assert_eq!(allowed_features(Bus).len(), 6);
    }

    #[test]
    fn virtual_bus_has_5_features() {
        assert_eq!(allowed_features(VirtualBus).len(), 5);
    }

    #[test]
    fn device_has_9_features() {
        assert_eq!(allowed_features(Device).len(), 9);
    }

    #[test]
    fn subprogram_has_8_features() {
        assert_eq!(allowed_features(Subprogram).len(), 8);
    }

    #[test]
    fn subprogram_group_has_7_features() {
        assert_eq!(allowed_features(SubprogramGroup).len(), 7);
    }

    #[test]
    fn data_has_7_features() {
        assert_eq!(allowed_features(Data).len(), 7);
    }

    #[test]
    fn abstract_has_all_10_features() {
        assert_eq!(allowed_features(Abstract).len(), 10);
    }

    // ── Subcomponent count tests ─────────────────────────────────────

    #[test]
    fn system_has_10_subcomponents() {
        assert_eq!(allowed_subcomponents(System).len(), 10);
    }

    #[test]
    fn process_has_6_subcomponents() {
        assert_eq!(allowed_subcomponents(Process).len(), 6);
    }

    #[test]
    fn thread_has_3_subcomponents() {
        assert_eq!(allowed_subcomponents(Thread).len(), 3);
    }

    #[test]
    fn thread_group_has_5_subcomponents() {
        assert_eq!(allowed_subcomponents(ThreadGroup).len(), 5);
    }

    #[test]
    fn processor_has_5_subcomponents() {
        assert_eq!(allowed_subcomponents(Processor).len(), 5);
    }

    #[test]
    fn virtual_processor_has_3_subcomponents() {
        assert_eq!(allowed_subcomponents(VirtualProcessor).len(), 3);
    }

    #[test]
    fn memory_has_3_subcomponents() {
        assert_eq!(allowed_subcomponents(Memory).len(), 3);
    }

    #[test]
    fn bus_has_2_subcomponents() {
        assert_eq!(allowed_subcomponents(Bus).len(), 2);
    }

    #[test]
    fn virtual_bus_has_2_subcomponents() {
        assert_eq!(allowed_subcomponents(VirtualBus).len(), 2);
    }

    #[test]
    fn device_has_4_subcomponents() {
        assert_eq!(allowed_subcomponents(Device).len(), 4);
    }

    #[test]
    fn subprogram_has_2_subcomponents() {
        assert_eq!(allowed_subcomponents(Subprogram).len(), 2);
    }

    #[test]
    fn subprogram_group_has_4_subcomponents() {
        assert_eq!(allowed_subcomponents(SubprogramGroup).len(), 4);
    }

    #[test]
    fn data_has_3_subcomponents() {
        assert_eq!(allowed_subcomponents(Data).len(), 3);
    }

    #[test]
    fn abstract_has_all_14_subcomponents() {
        assert_eq!(allowed_subcomponents(Abstract).len(), 14);
    }

    // ── Specific positive cases ──────────────────────────────────────

    #[test]
    fn system_allows_bus_access() {
        assert!(is_feature_allowed(System, BusAccess));
    }

    #[test]
    fn thread_allows_parameter() {
        assert!(is_feature_allowed(Thread, Parameter));
    }

    #[test]
    fn subprogram_allows_parameter() {
        assert!(is_feature_allowed(Subprogram, Parameter));
    }

    #[test]
    fn device_allows_bus_access() {
        assert!(is_feature_allowed(Device, BusAccess));
    }

    #[test]
    fn processor_allows_bus_access() {
        assert!(is_feature_allowed(Processor, BusAccess));
    }

    #[test]
    fn system_allows_process_subcomponent() {
        assert!(is_subcomponent_allowed(System, Process));
    }

    #[test]
    fn system_allows_device_subcomponent() {
        assert!(is_subcomponent_allowed(System, Device));
    }

    #[test]
    fn process_allows_thread_subcomponent() {
        assert!(is_subcomponent_allowed(Process, Thread));
    }

    #[test]
    fn process_allows_thread_group_subcomponent() {
        assert!(is_subcomponent_allowed(Process, ThreadGroup));
    }

    #[test]
    fn processor_allows_virtual_processor_subcomponent() {
        assert!(is_subcomponent_allowed(Processor, VirtualProcessor));
    }

    // ── Specific negative cases ──────────────────────────────────────

    #[test]
    fn process_disallows_bus_access() {
        assert!(!is_feature_allowed(Process, BusAccess));
    }

    #[test]
    fn virtual_processor_disallows_bus_access() {
        assert!(!is_feature_allowed(VirtualProcessor, BusAccess));
    }

    #[test]
    fn memory_disallows_data_access() {
        assert!(!is_feature_allowed(Memory, DataAccess));
    }

    #[test]
    fn bus_disallows_data_access() {
        assert!(!is_feature_allowed(Bus, DataAccess));
    }

    #[test]
    fn virtual_bus_disallows_bus_access() {
        assert!(!is_feature_allowed(VirtualBus, BusAccess));
    }

    #[test]
    fn data_disallows_parameter() {
        assert!(!is_feature_allowed(Data, Parameter));
    }

    #[test]
    fn process_disallows_parameter() {
        assert!(!is_feature_allowed(Process, Parameter));
    }

    #[test]
    fn subprogram_group_disallows_parameter() {
        assert!(!is_feature_allowed(SubprogramGroup, Parameter));
    }

    #[test]
    fn process_disallows_system_subcomponent() {
        assert!(!is_subcomponent_allowed(Process, System));
    }

    #[test]
    fn thread_disallows_process_subcomponent() {
        assert!(!is_subcomponent_allowed(Thread, Process));
    }

    #[test]
    fn bus_disallows_system_subcomponent() {
        assert!(!is_subcomponent_allowed(Bus, System));
    }

    #[test]
    fn memory_disallows_thread_subcomponent() {
        assert!(!is_subcomponent_allowed(Memory, Thread));
    }

    #[test]
    fn device_disallows_processor_subcomponent() {
        assert!(!is_subcomponent_allowed(Device, Processor));
    }

    #[test]
    fn subprogram_disallows_thread_subcomponent() {
        assert!(!is_subcomponent_allowed(Subprogram, Thread));
    }

    // ── Abstract allows everything ───────────────────────────────────

    #[test]
    fn abstract_allows_all_feature_kinds() {
        for &feature in ALL_FEATURES {
            assert!(
                is_feature_allowed(Abstract, feature),
                "abstract should allow {:?}",
                feature
            );
        }
    }

    #[test]
    fn abstract_allows_all_subcomponent_categories() {
        for &cat in ALL_CATEGORIES {
            assert!(
                is_subcomponent_allowed(Abstract, cat),
                "abstract should allow {:?} subcomponent",
                cat
            );
        }
    }

    // ── Every category allows data port, event port, event data port ─

    #[test]
    fn all_categories_allow_ports() {
        for &cat in ALL_CATEGORIES {
            assert!(
                is_feature_allowed(cat, DataPort),
                "{:?} should allow data port",
                cat
            );
            assert!(
                is_feature_allowed(cat, EventPort),
                "{:?} should allow event port",
                cat
            );
            assert!(
                is_feature_allowed(cat, EventDataPort),
                "{:?} should allow event data port",
                cat
            );
        }
    }

    // ── Every category allows abstract subcomponent ──────────────────

    #[test]
    fn all_categories_allow_abstract_subcomponent() {
        for &cat in ALL_CATEGORIES {
            assert!(
                is_subcomponent_allowed(cat, Abstract),
                "{:?} should allow abstract subcomponent",
                cat
            );
        }
    }

    // ── Only thread and subprogram allow parameter ───────────────────

    #[test]
    fn only_thread_subprogram_abstract_allow_parameter() {
        for &cat in ALL_CATEGORIES {
            let expected = matches!(cat, Thread | Subprogram | Abstract);
            assert_eq!(
                is_feature_allowed(cat, Parameter),
                expected,
                "{:?} parameter allowed = {}, expected {}",
                cat,
                is_feature_allowed(cat, Parameter),
                expected
            );
        }
    }
}
