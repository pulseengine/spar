#[test]
fn aadl_config_preserves_module() {
    #[crate::aadl_config]
    pub mod test_config {
        pub const COMPONENT: &str = "Test::Component.Impl";
        pub const PERIOD_PS: u64 = 10_000_000_000;
    }

    assert_eq!(test_config::COMPONENT, "Test::Component.Impl");
    assert_eq!(test_config::PERIOD_PS, 10_000_000_000);
}

#[test]
fn aadl_config_preserves_all_constant_types() {
    #[crate::aadl_config]
    pub mod full_config {
        pub const COMPONENT: &str = "SensorFusion::Ctrl.Impl";
        pub const CATEGORY: &str = "thread";
        pub const PERIOD_PS: u64 = 10_000_000_000;
        pub const DEADLINE_PS: u64 = 8_000_000_000;
        pub const WCET_PS: u64 = 2_000_000_000;
        pub const PROCESSOR_BINDING: &str = "cpu1";
    }

    assert_eq!(full_config::COMPONENT, "SensorFusion::Ctrl.Impl");
    assert_eq!(full_config::CATEGORY, "thread");
    assert_eq!(full_config::PERIOD_PS, 10_000_000_000);
    assert_eq!(full_config::DEADLINE_PS, 8_000_000_000);
    assert_eq!(full_config::WCET_PS, 2_000_000_000);
    assert_eq!(full_config::PROCESSOR_BINDING, "cpu1");
}

#[test]
fn aadl_config_works_without_component_const() {
    #[crate::aadl_config]
    pub mod no_component {
        pub const CATEGORY: &str = "process";
    }

    assert_eq!(no_component::CATEGORY, "process");
}
