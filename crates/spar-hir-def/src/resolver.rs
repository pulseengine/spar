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

    #[allow(clippy::should_implement_trait)]
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
    /// Set of private component type names (ci_name) — these are only
    /// visible within the declaring package.
    pub private_types: rustc_hash::FxHashSet<CiName>,
    /// Set of private component impl keys — same visibility rule.
    pub private_impls: rustc_hash::FxHashSet<(CiName, CiName)>,
    /// Set of private feature group type names — same visibility rule.
    pub private_fgts: rustc_hash::FxHashSet<CiName>,
    /// Package renames: alias (ci_name) → original package name.
    pub package_renames: FxHashMap<CiName, Name>,
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

                // Register items from both public and private sections.
                // Items from the private section are registered but marked
                // so they can be filtered out when resolving from another package.
                Self::register_items(
                    &mut pkg_scope,
                    &mut scope.property_sets,
                    tree,
                    tree_idx,
                    &pkg.public_items,
                    true,
                );
                Self::register_items(
                    &mut pkg_scope,
                    &mut scope.property_sets,
                    tree,
                    tree_idx,
                    &pkg.private_items,
                    false,
                );

                // Process renames declarations
                for &renames_idx in &pkg.renames {
                    let ri = &tree.renames[renames_idx];
                    if ri.kind == crate::item_tree::RenamesKind::Package {
                        pkg_scope
                            .package_renames
                            .insert(CiName::new(&ri.alias), ri.original.clone());
                    }
                }

                scope.packages.insert(CiName::new(&pkg.name), pkg_scope);
            }

            // Top-level property sets (outside packages)
            for (_idx, ps) in tree.property_sets.iter() {
                scope.property_sets.insert(
                    CiName::new(&ps.name),
                    PropertySetInfo {
                        name: ps.name.clone(),
                        property_names: ps.property_defs.iter().map(|d| d.name.clone()).collect(),
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
            scope.property_sets.entry(ci_key).or_insert_with(|| {
                let prop_names = standard_properties::standard_properties_in_set(set_name);
                PropertySetInfo {
                    name: Name::new(set_name),
                    property_names: prop_names.iter().map(|&n| Name::new(n)).collect(),
                    constant_names: Vec::new(),
                }
            });
        }

        scope
    }

    /// Register items from a package section (public or private) into a PackageScope.
    fn register_items(
        pkg_scope: &mut PackageScope,
        property_sets: &mut FxHashMap<CiName, PropertySetInfo>,
        tree: &ItemTree,
        tree_idx: usize,
        items: &[ItemRef],
        is_public: bool,
    ) {
        for item_ref in items {
            match item_ref {
                ItemRef::ComponentType(idx) => {
                    let ct = &tree.component_types[*idx];
                    let ci_name = CiName::new(&ct.name);
                    pkg_scope.component_types.insert(
                        ci_name.clone(),
                        ItemLoc {
                            tree: tree_idx,
                            raw_idx: idx.into_raw().into_u32(),
                        },
                    );
                    if !is_public {
                        pkg_scope.private_types.insert(ci_name);
                    }
                }
                ItemRef::ComponentImpl(idx) => {
                    let ci = &tree.component_impls[*idx];
                    let key = (CiName::new(&ci.type_name), CiName::new(&ci.impl_name));
                    pkg_scope.component_impls.insert(
                        key.clone(),
                        ItemLoc {
                            tree: tree_idx,
                            raw_idx: idx.into_raw().into_u32(),
                        },
                    );
                    if !is_public {
                        pkg_scope.private_impls.insert(key);
                    }
                }
                ItemRef::FeatureGroupType(idx) => {
                    let fgt = &tree.feature_group_types[*idx];
                    let ci_name = CiName::new(&fgt.name);
                    pkg_scope.feature_group_types.insert(
                        ci_name.clone(),
                        ItemLoc {
                            tree: tree_idx,
                            raw_idx: idx.into_raw().into_u32(),
                        },
                    );
                    if !is_public {
                        pkg_scope.private_fgts.insert(ci_name);
                    }
                }
                ItemRef::PropertySet(idx) => {
                    let ps = &tree.property_sets[*idx];
                    property_sets.insert(
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
    }

    /// Get an item tree by index.
    pub fn tree(&self, idx: usize) -> Option<&ItemTree> {
        self.trees.get(idx).map(|arc| arc.as_ref())
    }

    /// Look up a component implementation's data by its location.
    pub fn get_component_impl(&self, loc: ItemLoc) -> Option<&ComponentImplItem> {
        let tree = self.tree(loc.tree)?;
        let idx: ComponentImplIdx =
            la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
        Some(&tree.component_impls[idx])
    }

    /// Look up a component type's data by its location.
    pub fn get_component_type(&self, loc: ItemLoc) -> Option<&ComponentTypeItem> {
        let tree = self.tree(loc.tree)?;
        let idx: ComponentTypeIdx =
            la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(loc.raw_idx));
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
        let from_key = CiName::new(from_package);
        let is_same_package;

        let target_pkg = match &reference.package {
            Some(pkg_name) => {
                let mut key = CiName::new(pkg_name);
                // Check if the package name is a renames alias
                if let Some(from_scope) = self.packages.get(&from_key)
                    && let Some(original) = from_scope.package_renames.get(&key)
                {
                    key = CiName::new(original);
                }
                is_same_package = key == from_key;
                key
            }
            None => {
                is_same_package = true;
                from_key.clone()
            }
        };

        if let Some(result) = self.resolve_in_package(&target_pkg, reference, is_same_package) {
            return result;
        }

        // If no explicit package, search imports
        if reference.package.is_none()
            && let Some(from_scope) = self.packages.get(&from_key)
        {
            for import in &from_scope.imports {
                let import_key = CiName::new(import);
                // Resolving from an imported package — never same package
                if let Some(result) = self.resolve_in_package(&import_key, reference, false) {
                    return result;
                }
            }
        }

        ResolvedClassifier::Unresolved
    }

    fn resolve_in_package(
        &self,
        pkg_key: &CiName,
        reference: &ClassifierRef,
        is_same_package: bool,
    ) -> Option<ResolvedClassifier> {
        let pkg_scope = self.packages.get(pkg_key)?;

        // Implementation reference
        if let Some(impl_name) = &reference.impl_name {
            let key = (CiName::new(&reference.type_name), CiName::new(impl_name));
            if let Some(&loc) = pkg_scope.component_impls.get(&key) {
                // Check visibility: private impls are only visible within the same package
                if !is_same_package && pkg_scope.private_impls.contains(&key) {
                    return None;
                }
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
            // Check visibility: private types are only visible within the same package
            if !is_same_package && pkg_scope.private_types.contains(&type_key) {
                return None;
            }
            return Some(ResolvedClassifier::ComponentType {
                package: pkg_scope.name.clone(),
                loc,
            });
        }

        // Feature group type
        if let Some(&loc) = pkg_scope.feature_group_types.get(&type_key) {
            // Check visibility: private FGTs are only visible within the same package
            if !is_same_package && pkg_scope.private_fgts.contains(&type_key) {
                return None;
            }
            return Some(ResolvedClassifier::FeatureGroupType {
                package: pkg_scope.name.clone(),
                loc,
            });
        }

        None
    }

    /// Check if a classifier in a target package is private (not visible from outside).
    pub fn is_private_classifier(&self, target_package: &Name, reference: &ClassifierRef) -> bool {
        let pkg_key = CiName::new(target_package);
        let pkg_scope = match self.packages.get(&pkg_key) {
            Some(s) => s,
            None => return false,
        };

        if let Some(impl_name) = &reference.impl_name {
            let key = (CiName::new(&reference.type_name), CiName::new(impl_name));
            return pkg_scope.private_impls.contains(&key);
        }

        let type_key = CiName::new(&reference.type_name);
        if pkg_scope.private_types.contains(&type_key) {
            return true;
        }
        if pkg_scope.private_fgts.contains(&type_key) {
            return true;
        }
        false
    }

    /// Resolve a package name, following renames aliases if present.
    pub fn resolve_package_name(&self, from_package: &Name, pkg_name: &Name) -> Option<Name> {
        let from_key = CiName::new(from_package);
        let from_scope = self.packages.get(&from_key)?;

        let ci = CiName::new(pkg_name);
        // Check if it's a renames alias
        if let Some(original) = from_scope.package_renames.get(&ci) {
            return Some(original.clone());
        }

        // Check if it's a real package
        if self.packages.contains_key(&ci) {
            return Some(pkg_name.clone());
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

    /// List all property set names (including standard predeclared sets).
    pub fn property_set_names(&self) -> Vec<&Name> {
        self.property_sets.values().map(|ps| &ps.name).collect()
    }

    /// List all property names within a given property set.
    ///
    /// Returns an empty vec if the property set is not found.
    /// Comparison is case-insensitive.
    pub fn property_names_in_set(&self, set_name: &str) -> Vec<&Name> {
        let key = CiName::from_str(set_name);
        match self.property_sets.get(&key) {
            Some(info) => info.property_names.iter().collect(),
            None => Vec::new(),
        }
    }

    /// List all component types across all packages, returning (package_name, type_name, category).
    pub fn all_component_types(&self) -> Vec<(&Name, &Name, crate::item_tree::ComponentCategory)> {
        let mut result = Vec::new();
        for pkg_scope in self.packages.values() {
            for loc in pkg_scope.component_types.values() {
                if let Some(ct) = self.get_component_type(*loc) {
                    result.push((&pkg_scope.name, &ct.name, ct.category));
                }
            }
        }
        result
    }

    /// List all component implementations across all packages,
    /// returning (package_name, type_name, impl_name, category).
    pub fn all_component_impls(
        &self,
    ) -> Vec<(&Name, &Name, &Name, crate::item_tree::ComponentCategory)> {
        let mut result = Vec::new();
        for pkg_scope in self.packages.values() {
            for loc in pkg_scope.component_impls.values() {
                if let Some(ci) = self.get_component_impl(*loc) {
                    result.push((&pkg_scope.name, &ci.type_name, &ci.impl_name, ci.category));
                }
            }
        }
        result
    }

    /// List all feature group types across all packages,
    /// returning (package_name, fgt_name).
    pub fn all_feature_group_types(&self) -> Vec<(&Name, &Name)> {
        let mut result = Vec::new();
        for pkg_scope in self.packages.values() {
            for loc in pkg_scope.feature_group_types.values() {
                if let Some(fgt) = self.get_feature_group_type(*loc) {
                    result.push((&pkg_scope.name, &fgt.name));
                }
            }
        }
        result
    }
}
