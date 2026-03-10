//! Prototype resolution (AS5506 §4.7).
//!
//! AADL prototypes are a generic/template mechanism. A component type
//! declares formal prototypes; an implementation or subcomponent
//! declaration provides actual bindings.
//!
//! Example:
//! ```aadl
//! system Controller
//!   prototypes
//!     proc : processor;       -- formal: "proc" must be a processor
//! end Controller;
//!
//! system implementation Controller.impl
//!   subcomponents
//!     sub1 : system Controller (proc => MyProcessor.impl);
//! end Controller.impl;
//! ```

use crate::item_tree::{ComponentCategory, ItemTree, PrototypeBindingIdx, PrototypeIdx};
use crate::name::{ClassifierRef, Name};

/// A resolved prototype: a formal paired with its actual binding (if any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPrototype {
    /// The formal prototype name.
    pub formal_name: Name,
    /// The category constraint on the prototype (e.g., must be a processor).
    pub formal_category: Option<ComponentCategory>,
    /// The constraining classifier from the formal declaration (if any).
    pub formal_constraining_classifier: Option<ClassifierRef>,
    /// The actual classifier bound to this prototype, if any.
    pub actual_classifier: Option<ClassifierRef>,
    /// The actual category (when binding to a category rather than a classifier).
    pub actual_category: Option<ComponentCategory>,
}

impl ResolvedPrototype {
    /// Returns `true` if this prototype has been bound to an actual.
    pub fn is_bound(&self) -> bool {
        self.actual_classifier.is_some() || self.actual_category.is_some()
    }
}

/// Resolve prototype bindings for a component.
///
/// Takes the formal prototypes from a component type (or implementation)
/// and the bindings provided at a use site, and returns the resolved
/// mappings. Matching is case-insensitive per the AADL spec (AS5506 §3.1).
pub fn resolve_prototypes(
    tree: &ItemTree,
    formals: &[PrototypeIdx],
    bindings: &[PrototypeBindingIdx],
) -> Vec<ResolvedPrototype> {
    let mut resolved = Vec::new();

    for &formal_idx in formals {
        let formal = &tree.prototypes[formal_idx];

        // Look for a binding that matches this formal's name (case-insensitive)
        let matching_binding = bindings.iter().find_map(|&bind_idx| {
            let binding = &tree.prototype_bindings[bind_idx];
            if binding.formal.eq_ci(&formal.name) {
                Some(binding)
            } else {
                None
            }
        });

        let (actual_classifier, actual_category) = match matching_binding {
            Some(binding) => (binding.actual.clone(), binding.actual_category),
            None => (None, None),
        };

        resolved.push(ResolvedPrototype {
            formal_name: formal.name.clone(),
            formal_category: formal.category,
            formal_constraining_classifier: formal.constraining_classifier.clone(),
            actual_classifier,
            actual_category,
        });
    }

    resolved
}

/// Validate that prototype bindings are category-compatible.
///
/// Returns a list of error messages for incompatible bindings.
/// When an actual category is provided, it must match the formal's
/// category constraint.
pub fn validate_prototype_bindings(resolved: &[ResolvedPrototype]) -> Vec<String> {
    let mut errors = Vec::new();

    for proto in resolved {
        // If the formal has a category constraint and the binding
        // provides an explicit category, they must match.
        if let (Some(formal_cat), Some(actual_cat)) = (proto.formal_category, proto.actual_category)
            && formal_cat != actual_cat
        {
            errors.push(format!(
                "prototype '{}': expected category '{}', got '{}'",
                proto.formal_name, formal_cat, actual_cat,
            ));
        }
    }

    errors
}

/// Collect unbound formals — prototypes that have no matching binding.
///
/// These may need to be bound at a higher level in the hierarchy.
pub fn unbound_prototypes(resolved: &[ResolvedPrototype]) -> Vec<&ResolvedPrototype> {
    resolved.iter().filter(|p| !p.is_bound()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::{ItemTree, PrototypeBindingItem, PrototypeItem};
    use crate::name::{ClassifierRef, Name};

    /// Helper: create an ItemTree with given prototypes and bindings,
    /// then resolve them.
    fn make_and_resolve(
        prototypes: Vec<PrototypeItem>,
        bindings: Vec<PrototypeBindingItem>,
    ) -> Vec<ResolvedPrototype> {
        let mut tree = ItemTree::default();
        let formal_idxs: Vec<_> = prototypes
            .into_iter()
            .map(|p| tree.prototypes.alloc(p))
            .collect();
        let binding_idxs: Vec<_> = bindings
            .into_iter()
            .map(|b| tree.prototype_bindings.alloc(b))
            .collect();
        resolve_prototypes(&tree, &formal_idxs, &binding_idxs)
    }

    #[test]
    fn resolve_single_binding() {
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("proc"),
                category: Some(ComponentCategory::Processor),
                constraining_classifier: None,
            }],
            vec![PrototypeBindingItem {
                formal: Name::new("proc"),
                actual: Some(ClassifierRef::qualified(
                    Name::new("HW"),
                    Name::new("MyProcessor"),
                )),
                actual_category: None,
            }],
        );

        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        assert_eq!(r.formal_name.as_str(), "proc");
        assert_eq!(r.formal_category, Some(ComponentCategory::Processor));
        assert!(r.is_bound());
        assert_eq!(
            r.actual_classifier,
            Some(ClassifierRef::qualified(
                Name::new("HW"),
                Name::new("MyProcessor"),
            ))
        );
    }

    #[test]
    fn unbound_prototype_no_matching_binding() {
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("data_type"),
                category: Some(ComponentCategory::Data),
                constraining_classifier: None,
            }],
            vec![], // no bindings
        );

        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        assert_eq!(r.formal_name.as_str(), "data_type");
        assert!(!r.is_bound());
        assert!(r.actual_classifier.is_none());
        assert!(r.actual_category.is_none());

        let unbound = unbound_prototypes(&resolved);
        assert_eq!(unbound.len(), 1);
        assert_eq!(unbound[0].formal_name.as_str(), "data_type");
    }

    #[test]
    fn case_insensitive_matching() {
        // AADL names are case-insensitive (AS5506 §3.1)
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("MyProto"),
                category: Some(ComponentCategory::System),
                constraining_classifier: None,
            }],
            vec![PrototypeBindingItem {
                formal: Name::new("myproto"), // different case
                actual: Some(ClassifierRef::type_only(Name::new("ConcreteSystem"))),
                actual_category: None,
            }],
        );

        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].is_bound());
        assert_eq!(
            resolved[0].actual_classifier,
            Some(ClassifierRef::type_only(Name::new("ConcreteSystem")))
        );
    }

    #[test]
    fn multiple_prototypes_partial_binding() {
        let resolved = make_and_resolve(
            vec![
                PrototypeItem {
                    name: Name::new("bus_proto"),
                    category: Some(ComponentCategory::Bus),
                    constraining_classifier: None,
                },
                PrototypeItem {
                    name: Name::new("data_proto"),
                    category: Some(ComponentCategory::Data),
                    constraining_classifier: None,
                },
                PrototypeItem {
                    name: Name::new("proc_proto"),
                    category: Some(ComponentCategory::Processor),
                    constraining_classifier: None,
                },
            ],
            vec![
                // Only bind bus_proto and proc_proto, leave data_proto unbound
                PrototypeBindingItem {
                    formal: Name::new("bus_proto"),
                    actual: Some(ClassifierRef::type_only(Name::new("Ethernet"))),
                    actual_category: None,
                },
                PrototypeBindingItem {
                    formal: Name::new("proc_proto"),
                    actual: Some(ClassifierRef::qualified(
                        Name::new("HW"),
                        Name::new("ARM_Cortex"),
                    )),
                    actual_category: None,
                },
            ],
        );

        assert_eq!(resolved.len(), 3);

        // bus_proto: bound
        assert!(resolved[0].is_bound());
        assert_eq!(resolved[0].formal_name.as_str(), "bus_proto");

        // data_proto: unbound
        assert!(!resolved[1].is_bound());
        assert_eq!(resolved[1].formal_name.as_str(), "data_proto");

        // proc_proto: bound
        assert!(resolved[2].is_bound());
        assert_eq!(resolved[2].formal_name.as_str(), "proc_proto");

        let unbound = unbound_prototypes(&resolved);
        assert_eq!(unbound.len(), 1);
        assert_eq!(unbound[0].formal_name.as_str(), "data_proto");
    }

    #[test]
    fn validate_category_mismatch() {
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("proc"),
                category: Some(ComponentCategory::Processor),
                constraining_classifier: None,
            }],
            vec![PrototypeBindingItem {
                formal: Name::new("proc"),
                actual: None,
                actual_category: Some(ComponentCategory::Memory), // wrong category
            }],
        );

        let errors = validate_prototype_bindings(&resolved);
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].contains("processor"),
            "error should mention expected category: {}",
            errors[0]
        );
        assert!(
            errors[0].contains("memory"),
            "error should mention actual category: {}",
            errors[0]
        );
    }

    #[test]
    fn validate_category_match_ok() {
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("proc"),
                category: Some(ComponentCategory::Processor),
                constraining_classifier: None,
            }],
            vec![PrototypeBindingItem {
                formal: Name::new("proc"),
                actual: None,
                actual_category: Some(ComponentCategory::Processor),
            }],
        );

        let errors = validate_prototype_bindings(&resolved);
        assert!(errors.is_empty(), "unexpected errors: {:?}", errors);
    }

    #[test]
    fn constraining_classifier_preserved() {
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("sensor"),
                category: Some(ComponentCategory::Device),
                constraining_classifier: Some(ClassifierRef::qualified(
                    Name::new("Devices"),
                    Name::new("BaseSensor"),
                )),
            }],
            vec![PrototypeBindingItem {
                formal: Name::new("sensor"),
                actual: Some(ClassifierRef::qualified(
                    Name::new("Devices"),
                    Name::new("TempSensor"),
                )),
                actual_category: None,
            }],
        );

        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        assert_eq!(
            r.formal_constraining_classifier,
            Some(ClassifierRef::qualified(
                Name::new("Devices"),
                Name::new("BaseSensor"),
            ))
        );
        assert!(r.is_bound());
    }

    #[test]
    fn empty_formals_and_bindings() {
        let resolved = make_and_resolve(vec![], vec![]);
        assert!(resolved.is_empty());
    }

    #[test]
    fn binding_with_no_matching_formal_is_ignored() {
        // Extra bindings that don't match any formal should not produce
        // spurious entries — only formals drive the result.
        let resolved = make_and_resolve(
            vec![PrototypeItem {
                name: Name::new("alpha"),
                category: None,
                constraining_classifier: None,
            }],
            vec![
                PrototypeBindingItem {
                    formal: Name::new("alpha"),
                    actual: Some(ClassifierRef::type_only(Name::new("A"))),
                    actual_category: None,
                },
                PrototypeBindingItem {
                    formal: Name::new("beta"), // no matching formal
                    actual: Some(ClassifierRef::type_only(Name::new("B"))),
                    actual_category: None,
                },
            ],
        );

        // Only one resolved entry (for "alpha"), "beta" binding is orphaned
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].formal_name.as_str(), "alpha");
        assert!(resolved[0].is_bound());
    }
}
