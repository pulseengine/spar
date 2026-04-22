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
    /// Classifier renames: alias (ci_name) → (package, type_name).
    pub classifier_renames: FxHashMap<CiName, (Name, Name)>,
    /// Feature group renames: alias (ci_name) → (package, fgt_name).
    pub feature_group_renames: FxHashMap<CiName, (Name, Name)>,
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
    /// Diagnostics collected during scope construction (e.g. duplicate packages).
    pub diagnostics: Vec<String>,
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
                    match ri.kind {
                        crate::item_tree::RenamesKind::Package => {
                            pkg_scope
                                .package_renames
                                .insert(CiName::new(&ri.alias), ri.original.clone());
                        }
                        crate::item_tree::RenamesKind::Classifier
                        | crate::item_tree::RenamesKind::FeatureGroup => {
                            // Original is stored as "Pkg::TypeName"; split on "::"
                            let orig_str = ri.original.as_str();
                            if let Some((pkg_part, type_part)) = orig_str.split_once("::") {
                                let pkg_name = Name::new(pkg_part.trim());
                                let type_name = Name::new(type_part.trim());
                                let alias_key = CiName::new(&ri.alias);
                                if ri.kind == crate::item_tree::RenamesKind::Classifier {
                                    pkg_scope
                                        .classifier_renames
                                        .insert(alias_key, (pkg_name, type_name));
                                } else {
                                    pkg_scope
                                        .feature_group_renames
                                        .insert(alias_key, (pkg_name, type_name));
                                }
                            }
                        }
                    }
                }

                let key = CiName::new(&pkg.name);
                if scope.packages.contains_key(&key) {
                    scope.diagnostics.push(format!(
                        "package '{}' is defined in multiple files; only the last definition is used",
                        pkg.name,
                    ));
                }
                scope.packages.insert(key, pkg_scope);
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

        // Check classifier and feature group renames in the originating package.
        //
        // AS-5506D §4.2: a classifier alias names a type and can be used
        // anywhere that type can, including in `alias.impl_name` form. The
        // prior guard `impl_name.is_none()` skipped this branch whenever an
        // implementation suffix was present, so `MyAlias.i` resolved to
        // Unresolved despite `MyAlias renames system A::Target;`. Drop the
        // `impl_name.is_none()` gate and preserve `impl_name` through the
        // rewrite.
        if reference.package.is_none()
            && let Some(from_scope) = self.packages.get(&from_key)
        {
            let type_key = CiName::new(&reference.type_name);

            // Classifier renames: alias → (package, type_name). If the
            // reference carries `.impl_name`, preserve it through the
            // rewrite so `MyAlias.i` resolves to `orig_pkg::orig_type.i`.
            if let Some((orig_pkg, orig_type)) = from_scope.classifier_renames.get(&type_key) {
                let aliased_ref = match &reference.impl_name {
                    Some(impl_name) => ClassifierRef::implementation(
                        Some(orig_pkg.clone()),
                        orig_type.clone(),
                        impl_name.clone(),
                    ),
                    None => ClassifierRef::qualified(orig_pkg.clone(), orig_type.clone()),
                };
                return self.resolve_classifier(from_package, &aliased_ref);
            }

            // Feature group renames: alias → (package, fgt_name). Feature
            // group types do not have implementations, so the `.impl` form
            // should not be rewritten against a feature-group alias — fall
            // through if impl_name is present.
            if reference.impl_name.is_none()
                && let Some((orig_pkg, orig_fgt)) = from_scope.feature_group_renames.get(&type_key)
            {
                let aliased_ref = ClassifierRef::qualified(orig_pkg.clone(), orig_fgt.clone());
                return self.resolve_classifier(from_package, &aliased_ref);
            }
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

    /// Resolve a classifier reference, collecting ambiguity warnings (STPA-REQ-007).
    ///
    /// When an unqualified reference matches classifiers in multiple imported
    /// packages, returns the first match (same as `resolve_classifier`) but also
    /// emits a warning listing all candidate packages.
    pub fn resolve_classifier_with_diagnostics(
        &self,
        from_package: &Name,
        reference: &ClassifierRef,
    ) -> (ResolvedClassifier, Vec<String>) {
        let result = self.resolve_classifier(from_package, reference);
        let mut warnings = Vec::new();

        // Only check for ambiguity on unqualified references that resolved
        if reference.package.is_some() || matches!(result, ResolvedClassifier::Unresolved) {
            return (result, warnings);
        }

        let from_key = CiName::new(from_package);

        // Check if it resolved from same package (no ambiguity possible)
        if self
            .resolve_in_package(&from_key, reference, true)
            .is_some()
        {
            return (result, warnings);
        }

        // It resolved from an import — count all imports that match
        if let Some(from_scope) = self.packages.get(&from_key) {
            let mut candidates = Vec::new();
            for import in &from_scope.imports {
                let import_key = CiName::new(import);
                if self
                    .resolve_in_package(&import_key, reference, false)
                    .is_some()
                {
                    candidates.push(import.as_str().to_string());
                }
            }
            if candidates.len() > 1 {
                warnings.push(format!(
                    "ambiguous classifier reference '{}': matches found in packages {}; \
                     using first match from '{}'",
                    reference.type_name,
                    candidates.join(", "),
                    candidates[0],
                ));
            }
        }

        (result, warnings)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn ambiguous_unqualified_reference_warns() {
        // Packages A and B both have "Sensor" type.
        // Package C imports both and references unqualified "Sensor".
        let mut tree = ItemTree::default();

        let ct_a = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::Device,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_a)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let ct_b = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_b)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("C"),
            with_clauses: vec![Name::new("A"), Name::new("B")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let reference = ClassifierRef::type_only(Name::new("Sensor"));
        let (result, warnings) =
            scope.resolve_classifier_with_diagnostics(&Name::new("C"), &reference);

        assert!(
            !matches!(result, ResolvedClassifier::Unresolved),
            "should resolve to something"
        );
        assert!(
            warnings.iter().any(|w| w.contains("ambiguous")),
            "should warn about ambiguity: {:?}",
            warnings
        );
    }

    #[test]
    fn qualified_reference_no_ambiguity_warning() {
        let mut tree = ItemTree::default();

        let ct_a = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::Device,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_a)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let ct_b = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_b)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("C"),
            with_clauses: vec![Name::new("A"), Name::new("B")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        // Qualified reference: A::Sensor — no ambiguity
        let reference = ClassifierRef::qualified(Name::new("A"), Name::new("Sensor"));
        let (_result, warnings) =
            scope.resolve_classifier_with_diagnostics(&Name::new("C"), &reference);

        assert!(
            warnings.is_empty(),
            "qualified reference should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn same_package_match_no_ambiguity_warning() {
        let mut tree = ItemTree::default();

        // Package C has its own "Sensor" and imports A which also has "Sensor"
        let ct_c = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::Device,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("C"),
            with_clauses: vec![Name::new("A")],
            public_items: vec![ItemRef::ComponentType(ct_c)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let ct_a = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_a)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        let reference = ClassifierRef::type_only(Name::new("Sensor"));
        let (result, warnings) =
            scope.resolve_classifier_with_diagnostics(&Name::new("C"), &reference);

        // Same-package match takes priority — no ambiguity
        assert!(
            !matches!(result, ResolvedClassifier::Unresolved),
            "should resolve"
        );
        assert!(
            warnings.is_empty(),
            "same-package match should not warn: {:?}",
            warnings
        );
    }

    #[test]
    fn duplicate_package_emits_diagnostic() {
        // Two separate item trees each declare a package named "Foo".
        let mut tree1 = ItemTree::default();
        let ct1 = tree1.component_types.alloc(ComponentTypeItem {
            name: Name::new("Type1"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree1.packages.alloc(Package {
            name: Name::new("Foo"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct1)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let mut tree2 = ItemTree::default();
        let ct2 = tree2.component_types.alloc(ComponentTypeItem {
            name: Name::new("Type2"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree2.packages.alloc(Package {
            name: Name::new("Foo"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct2)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree1), Arc::new(tree2)]);

        assert!(
            !scope.diagnostics.is_empty(),
            "should emit a diagnostic for duplicate package name"
        );
        assert!(
            scope.diagnostics[0].contains("Foo"),
            "diagnostic should mention the package name: {:?}",
            scope.diagnostics[0]
        );
        assert!(
            scope.diagnostics[0].contains("multiple files"),
            "diagnostic should mention multiple files: {:?}",
            scope.diagnostics[0]
        );
    }

    #[test]
    fn no_diagnostic_for_unique_packages() {
        let mut tree = ItemTree::default();
        tree.packages.alloc(Package {
            name: Name::new("Alpha"),
            with_clauses: Vec::new(),
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });
        tree.packages.alloc(Package {
            name: Name::new("Beta"),
            with_clauses: Vec::new(),
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);
        assert!(
            scope.diagnostics.is_empty(),
            "no diagnostics expected for unique packages: {:?}",
            scope.diagnostics
        );
    }

    #[test]
    fn private_classifier_not_visible_from_other_package() {
        // Package A has a private type "Secret".
        // Package B imports A and tries to reference "Secret" — should fail.
        let mut tree = ItemTree::default();

        let ct_secret = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Secret"),
            category: ComponentCategory::System,
            is_public: false,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        // Public type in the same package to verify selective visibility
        let ct_public = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Visible"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct_public)],
            private_items: vec![ItemRef::ComponentType(ct_secret)],
            renames: Vec::new(),
        });

        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: vec![Name::new("A")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // From B, resolving A::Secret should fail (private)
        let ref_secret = ClassifierRef::qualified(Name::new("A"), Name::new("Secret"));
        let result = scope.resolve_classifier(&Name::new("B"), &ref_secret);
        assert!(
            matches!(result, ResolvedClassifier::Unresolved),
            "private type should not be visible from another package: {:?}",
            result
        );

        // From B, resolving A::Visible should succeed (public)
        let ref_visible = ClassifierRef::qualified(Name::new("A"), Name::new("Visible"));
        let result = scope.resolve_classifier(&Name::new("B"), &ref_visible);
        assert!(
            !matches!(result, ResolvedClassifier::Unresolved),
            "public type should be visible from another package: {:?}",
            result
        );

        // From A itself, resolving Secret should succeed (same package)
        let ref_secret_unqual = ClassifierRef::type_only(Name::new("Secret"));
        let result = scope.resolve_classifier(&Name::new("A"), &ref_secret_unqual);
        assert!(
            !matches!(result, ResolvedClassifier::Unresolved),
            "private type should be visible within the same package: {:?}",
            result
        );
    }

    #[test]
    fn package_renames_resolves_qualified_reference() {
        // Package Pkg1 has type "Sensor".
        // Package Pkg2 has `Pkg1Alias renames package Pkg1;`.
        // Reference Pkg1Alias::Sensor from Pkg2 should resolve to Pkg1::Sensor.
        let mut tree = ItemTree::default();

        let ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("Sensor"),
            category: ComponentCategory::Device,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("Pkg1"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let renames_idx = tree.renames.alloc(RenamesItem {
            alias: Name::new("Pkg1Alias"),
            original: Name::new("Pkg1"),
            kind: RenamesKind::Package,
        });
        tree.packages.alloc(Package {
            name: Name::new("Pkg2"),
            with_clauses: vec![Name::new("Pkg1")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: vec![renames_idx],
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // Pkg1Alias::Sensor from Pkg2 should resolve
        let reference = ClassifierRef::qualified(Name::new("Pkg1Alias"), Name::new("Sensor"));
        let result = scope.resolve_classifier(&Name::new("Pkg2"), &reference);
        assert!(
            matches!(
                result,
                ResolvedClassifier::ComponentType {
                    ref package,
                    ..
                } if package.as_str() == "Pkg1"
            ),
            "package rename should resolve Pkg1Alias::Sensor to Pkg1::Sensor: {:?}",
            result
        );
    }

    #[test]
    fn classifier_renames_resolves_unqualified_reference() {
        // Package A has type "OriginalSensor".
        // Package B has `MySensor renames system A::OriginalSensor;`.
        // Unqualified reference to "MySensor" from B should resolve to A::OriginalSensor.
        let mut tree = ItemTree::default();

        let ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("OriginalSensor"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let renames_idx = tree.renames.alloc(RenamesItem {
            alias: Name::new("MySensor"),
            original: Name::new("A::OriginalSensor"),
            kind: RenamesKind::Classifier,
        });
        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: vec![Name::new("A")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: vec![renames_idx],
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // Unqualified "MySensor" from B should resolve to A::OriginalSensor
        let reference = ClassifierRef::type_only(Name::new("MySensor"));
        let result = scope.resolve_classifier(&Name::new("B"), &reference);
        assert!(
            matches!(
                result,
                ResolvedClassifier::ComponentType {
                    ref package,
                    ..
                } if package.as_str() == "A"
            ),
            "classifier rename should resolve MySensor to A::OriginalSensor: {:?}",
            result
        );
    }

    #[test]
    fn classifier_renames_resolves_impl_reference() {
        // AS-5506D §4.2: classifier alias usable wherever the type is —
        // including `alias.impl_name` form. Prior to the fix,
        // resolve_classifier gated rename handling on `impl_name.is_none()`,
        // so `MyAlias.i` returned Unresolved despite a valid rename.
        let mut tree = ItemTree::default();

        let ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("OriginalSensor"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        let ci = tree.component_impls.alloc(ComponentImplItem {
            type_name: Name::new("OriginalSensor"),
            impl_name: Name::new("i"),
            category: ComponentCategory::System,
            is_public: true,
            extends: None,
            subcomponents: Vec::new(),
            connections: Vec::new(),
            end_to_end_flows: Vec::new(),
            flow_impls: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            call_sequences: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct), ItemRef::ComponentImpl(ci)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let renames_idx = tree.renames.alloc(RenamesItem {
            alias: Name::new("MySensor"),
            original: Name::new("A::OriginalSensor"),
            kind: RenamesKind::Classifier,
        });
        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: vec![Name::new("A")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: vec![renames_idx],
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // `MySensor.i` from B should resolve to A::OriginalSensor.i
        let reference = ClassifierRef::implementation(None, Name::new("MySensor"), Name::new("i"));
        let result = scope.resolve_classifier(&Name::new("B"), &reference);
        assert!(
            matches!(
                result,
                ResolvedClassifier::ComponentImpl { ref package, .. }
                    if package.as_str() == "A"
            ),
            "classifier rename should resolve MySensor.i to A::OriginalSensor.i: {:?}",
            result
        );
    }

    #[test]
    fn feature_group_renames_resolves_unqualified_reference() {
        // Package A has feature group type "OriginalBusIface".
        // Package B has `MyBus renames feature group A::OriginalBusIface;`.
        // Unqualified reference to "MyBus" from B should resolve to A::OriginalBusIface.
        let mut tree = ItemTree::default();

        let fgt = tree.feature_group_types.alloc(FeatureGroupTypeItem {
            name: Name::new("OriginalBusIface"),
            is_public: true,
            extends: None,
            inverse_of: None,
            features: Vec::new(),
            prototypes: Vec::new(),
        });
        tree.packages.alloc(Package {
            name: Name::new("A"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::FeatureGroupType(fgt)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let renames_idx = tree.renames.alloc(RenamesItem {
            alias: Name::new("MyBus"),
            original: Name::new("A::OriginalBusIface"),
            kind: RenamesKind::FeatureGroup,
        });
        tree.packages.alloc(Package {
            name: Name::new("B"),
            with_clauses: vec![Name::new("A")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: vec![renames_idx],
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // Unqualified "MyBus" from B should resolve to A::OriginalBusIface
        let reference = ClassifierRef::type_only(Name::new("MyBus"));
        let result = scope.resolve_classifier(&Name::new("B"), &reference);
        assert!(
            matches!(
                result,
                ResolvedClassifier::FeatureGroupType {
                    ref package,
                    ..
                } if package.as_str() == "A"
            ),
            "feature group rename should resolve MyBus to A::OriginalBusIface: {:?}",
            result
        );
    }

    #[test]
    fn classifier_renames_case_insensitive() {
        // Verify that classifier renames work case-insensitively.
        let mut tree = ItemTree::default();

        let ct = tree.component_types.alloc(ComponentTypeItem {
            name: Name::new("OrigType"),
            category: ComponentCategory::Data,
            is_public: true,
            extends: None,
            features: Vec::new(),
            flow_specs: Vec::new(),
            modes: Vec::new(),
            mode_transitions: Vec::new(),
            prototypes: Vec::new(),
            property_associations: Vec::new(),
            requires_modes: false,
        });
        tree.packages.alloc(Package {
            name: Name::new("Lib"),
            with_clauses: Vec::new(),
            public_items: vec![ItemRef::ComponentType(ct)],
            private_items: Vec::new(),
            renames: Vec::new(),
        });

        let renames_idx = tree.renames.alloc(RenamesItem {
            alias: Name::new("MyAlias"),
            original: Name::new("Lib::OrigType"),
            kind: RenamesKind::Classifier,
        });
        tree.packages.alloc(Package {
            name: Name::new("Consumer"),
            with_clauses: vec![Name::new("Lib")],
            public_items: Vec::new(),
            private_items: Vec::new(),
            renames: vec![renames_idx],
        });

        let scope = GlobalScope::from_trees(vec![Arc::new(tree)]);

        // Reference with different casing should still resolve
        let reference = ClassifierRef::type_only(Name::new("myalias"));
        let result = scope.resolve_classifier(&Name::new("Consumer"), &reference);
        assert!(
            matches!(
                result,
                ResolvedClassifier::ComponentType {
                    ref package,
                    ..
                } if package.as_str() == "Lib"
            ),
            "case-insensitive classifier rename should resolve: {:?}",
            result
        );
    }
}
