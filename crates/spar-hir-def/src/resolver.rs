//! Name resolution for AADL models.
//!
//! Resolves classifier references (`Pkg::Type.Impl`), property
//! references (`PropSet::PropName`), and feature/subcomponent
//! references within their containing scope.

use rustc_hash::FxHashMap;

use crate::item_tree::*;
use crate::name::{ClassifierRef, Name};
use crate::standard_properties;

/// Location of an item across multiple item trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemLoc {
    /// Index of the item tree in the global scope's `trees` vec.
    pub tree: usize,
    /// Raw index within the arena (as u32).
    pub raw_idx: u32,
}

/// Result of resolving a classifier reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedClassifier {
    ComponentType { package: Name, loc: ItemLoc },
    ComponentImpl { package: Name, loc: ItemLoc },
    FeatureGroupType { package: Name, loc: ItemLoc },
    Unresolved,
}

/// Result of resolving a property reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedProperty {
    PropertyDef {
        property_set: Name,
        property_name: Name,
    },
    PropertyConstant {
        property_set: Name,
        property_name: Name,
    },
    Unresolved,
}

/// Case-insensitive name wrapper for HashMap keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CiName(String);

impl CiName {
    pub fn new(name: &Name) -> Self {
        Self(name.as_str().to_ascii_lowercase())
    }

    pub fn from_str(s: &str) -> Self {
        Self(s.to_ascii_lowercase())
    }
}

/// Per-package scope for name resolution.
#[derive(Debug, Default)]
pub struct PackageScope {
    pub name: Name,
    pub imports: Vec<Name>,
    /// Component types: ci_name → (tree_idx, ComponentTypeIdx)
    pub component_types: FxHashMap<CiName, ItemLoc>,
    /// Component implementations: (ci_type_name, ci_impl_name) → loc
    pub component_impls: FxHashMap<(CiName, CiName), ItemLoc>,
    /// Feature group types: ci_name → loc
    pub feature_group_types: FxHashMap<CiName, ItemLoc>,
}

/// Stored property set info for resolution.
#[derive(Debug)]
struct PropertySetInfo {
    name: Name,
    property_names: Vec<Name>,
    constant_names: Vec<Name>,
}

/// Global scope containing all packages and property sets.
#[derive(Debug, Default)]
pub struct GlobalScope {
    pub packages: FxHashMap<CiName, PackageScope>,
    property_sets: FxHashMap<CiName, PropertySetInfo>,
    /// The underlying item trees, for looking up full item data.
    trees: Vec<std::sync::Arc<ItemTree>>,
}

impl GlobalScope {
    /// Build a global scope from a set of item trees.
    pub fn from_trees(trees: Vec<std::sync::Arc<ItemTree>>) -> Self {
        let mut scope = GlobalScope::default();

        for (tree_idx, tree) in trees.iter().enumerate() {
            for (_idx, pkg) in tree.packages.iter() {
                let mut pkg_scope = PackageScope {
                    name: pkg.name.clone(),
                    imports: pkg.with_clauses.clone(),
                    ..Default::default()
                };

                for item_ref in &pkg.public_items {
                    match item_ref {
                        ItemRef::ComponentType(idx) => {
                            let ct = &tree.component_types[*idx];
                            pkg_scope.component_types.insert(
                                CiName::new(&ct.name),
                                ItemLoc {
                                    tree: tree_idx,
                                    raw_idx: idx.into_raw().into_u32(),
                                },
                            );
                        }
                        ItemRef::ComponentImpl(idx) => {
                            let ci = &tree.component_impls[*idx];
                            pkg_scope.component_impls.insert(
                                (CiName::new(&ci.type_name), CiName::new(&ci.impl_name)),
                                ItemLoc {
                                    tree: tree_idx,
                                    raw_idx: idx.into_raw().into_u32(),
                                },
                            );
                        }
                        ItemRef::FeatureGroupType(idx) => {
                            let fgt = &tree.feature_group_types[*idx];
                            pkg_scope.feature_group_types.insert(
                                CiName::new(&fgt.name),
                                ItemLoc {
                                    tree: tree_idx,
                                    raw_idx: idx.into_raw().into_u32(),
                                },
                            );
                        }
                        ItemRef::PropertySet(idx) => {
                            let ps = &tree.property_sets[*idx];
                            scope.property_sets.insert(
                                CiName::new(&ps.name),
                                PropertySetInfo {
                                    name: ps.name.clone(),
                                    property_names: ps
                                        .property_defs
                                        .iter()
                                        .map(|d| d.name.clone())
                                        .collect(),
                                    constant_names: ps
                                        .property_constants
                                        .iter()
                                        .map(|c| c.name.clone())
                                        .collect(),
                                },
                            );
                        }
                        ItemRef::AnnexLibrary => {}
                    }
                }

                scope
                    .packages
                    .insert(CiName::new(&pkg.name), pkg_scope);
            }

            // Top-level property sets (outside packages)
            for (_idx, ps) in tree.property_sets.iter() {
                scope.property_sets.insert(
                    CiName::new(&ps.name),
                    PropertySetInfo {
                        name: ps.name.clone(),
                        property_names: ps
                            .property_defs
                            .iter()
                            .map(|d| d.name.clone())
                            .collect(),
                        constant_names: ps
                            .property_constants
                            .iter()
                            .map(|c| c.name.clone())
                            .collect(),
                    },
                );
            }
        }

        scope.trees = trees;

        // Register standard predefined property sets (AS5506 Appendix A)
        // so they can be resolved without explicit `with` imports.
        for &set_name in standard_properties::STANDARD_PROPERTY_SET_NAMES {
            let ci_key = CiName::from_str(set_name);
            // Don't overwrite a user-provided property set with the same name.
            if !scope.property_sets.contains_key(&ci_key) {
                let prop_names = standard_properties::standard_properties_in_set(set_name);
                scope.property_sets.insert(
                    ci_key,
                    PropertySetInfo {
                        name: Name::new(set_name),
                        property_names: prop_names.iter().map(|&n| Name::new(n)).collect(),
                        constant_names: Vec::new(),
                    },
                );
            }
        }

        scope
    }

    /// Get an item tree by index.
    pub fn tree(&self, idx: usize) -> Option<&ItemTree> {
        self.trees.get(idx).map(|arc| arc.as_ref())
    }

    /// Look up a component implementation's data by its location.
    pub fn get_component_impl(&self, loc: ItemLoc) -> Option<&ComponentImplItem> {
        let tree = self.tree(loc.tree)?;
        let idx: ComponentImplIdx = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
        Some(&tree.component_impls[idx])
    }

    /// Look up a component type's data by its location.
    pub fn get_component_type(&self, loc: ItemLoc) -> Option<&ComponentTypeItem> {
        let tree = self.tree(loc.tree)?;
        let idx: ComponentTypeIdx = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
        Some(&tree.component_types[idx])
    }

    /// Look up a feature group type's data by its location.
    pub fn get_feature_group_type(&self, loc: ItemLoc) -> Option<&FeatureGroupTypeItem> {
        let tree = self.tree(loc.tree)?;
        let idx: FeatureGroupTypeIdx =
            la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
        Some(&tree.feature_group_types[idx])
    }

    /// Look up a feature's data by tree index and feature index.
    pub fn get_feature(&self, tree_idx: usize, feat_idx: FeatureIdx) -> Option<&Feature> {
        let tree = self.tree(tree_idx)?;
        Some(&tree.features[feat_idx])
    }

    /// Resolve a classifier reference from within a package context.
    pub fn resolve_classifier(
        &self,
        from_package: &Name,
        reference: &ClassifierRef,
    ) -> ResolvedClassifier {
        let target_pkg = match &reference.package {
            Some(pkg_name) => CiName::new(pkg_name),
            None => CiName::new(from_package),
        };

        if let Some(result) = self.resolve_in_package(&target_pkg, reference) {
            return result;
        }

        // If no explicit package, search imports
        if reference.package.is_none() {
            let from_key = CiName::new(from_package);
            if let Some(from_scope) = self.packages.get(&from_key) {
                for import in &from_scope.imports {
                    let import_key = CiName::new(import);
                    if let Some(result) = self.resolve_in_package(&import_key, reference) {
                        return result;
                    }
                }
            }
        }

        ResolvedClassifier::Unresolved
    }

    fn resolve_in_package(
        &self,
        pkg_key: &CiName,
        reference: &ClassifierRef,
    ) -> Option<ResolvedClassifier> {
        let pkg_scope = self.packages.get(pkg_key)?;

        // Implementation reference
        if let Some(impl_name) = &reference.impl_name {
            let key = (
                CiName::new(&reference.type_name),
                CiName::new(impl_name),
            );
            if let Some(&loc) = pkg_scope.component_impls.get(&key) {
                return Some(ResolvedClassifier::ComponentImpl {
                    package: pkg_scope.name.clone(),
                    loc,
                });
            }
            return None;
        }

        // Component type
        let type_key = CiName::new(&reference.type_name);
        if let Some(&loc) = pkg_scope.component_types.get(&type_key) {
            return Some(ResolvedClassifier::ComponentType {
                package: pkg_scope.name.clone(),
                loc,
            });
        }

        // Feature group type
        if let Some(&loc) = pkg_scope.feature_group_types.get(&type_key) {
            return Some(ResolvedClassifier::FeatureGroupType {
                package: pkg_scope.name.clone(),
                loc,
            });
        }

        None
    }

    /// Resolve a property reference.
    pub fn resolve_property(
        &self,
        property_set_name: &Name,
        property_name: &Name,
    ) -> ResolvedProperty {
        let ps_key = CiName::new(property_set_name);
        let ps_info = match self.property_sets.get(&ps_key) {
            Some(info) => info,
            None => return ResolvedProperty::Unresolved,
        };

        let prop_key = CiName::from_str(property_name.as_str());

        // Check property definitions
        for def_name in &ps_info.property_names {
            if CiName::new(def_name) == prop_key {
                return ResolvedProperty::PropertyDef {
                    property_set: ps_info.name.clone(),
                    property_name: def_name.clone(),
                };
            }
        }

        // Check property constants
        for const_name in &ps_info.constant_names {
            if CiName::new(const_name) == prop_key {
                return ResolvedProperty::PropertyConstant {
                    property_set: ps_info.name.clone(),
                    property_name: const_name.clone(),
                };
            }
        }

        ResolvedProperty::Unresolved
    }

    /// Check if a package exists in scope.
    pub fn has_package(&self, name: &Name) -> bool {
        self.packages.contains_key(&CiName::new(name))
    }

    /// List all package names.
    pub fn package_names(&self) -> Vec<&Name> {
        self.packages.values().map(|s| &s.name).collect()
    }
}
