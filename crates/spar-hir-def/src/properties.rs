//! Property evaluation for AADL models.
//!
//! Implements the AADL property inheritance rules from AS5506 Section 11:
//!
//! 1. Properties on a component type apply to all instances of that type.
//! 2. Properties on a component implementation override/extend type properties.
//! 3. Properties on subcomponent declarations override implementation-level.
//! 4. `+=>` appends to inherited values (for list properties).
//! 5. Modal values (`value in modes (m1, m2)`) are not yet supported.

use rustc_hash::FxHashMap;

use crate::item_tree::{
    ComponentImplIdx, ComponentTypeIdx, ItemTree, PropertyAssociationIdx, SubcomponentIdx,
};
use crate::name::PropertyRef;
use crate::resolver::CiName;

/// A single property value, either assigned (`=>`) or appended (`+=>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyValue {
    /// The property reference (possibly qualified with a property set).
    pub name: PropertyRef,
    /// Raw text of the property value expression.
    pub value: String,
    /// Whether this is an append association (`+=>`).
    pub is_append: bool,
}

/// A collection of resolved property values for a model element.
///
/// Properties are keyed by `(property_set_ci, property_name_ci)` where
/// the property set key is empty string for unqualified properties.
/// Multiple values for the same key are possible via `+=>` append.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropertyMap {
    props: FxHashMap<(CiName, CiName), Vec<PropertyValue>>,
}

impl PropertyMap {
    /// Create a new empty property map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a property value.
    ///
    /// If `is_append` is true, the value is appended to any existing values.
    /// Otherwise, all existing values for that key are replaced.
    pub fn add(&mut self, prop: PropertyValue) {
        let set_key = match &prop.name.property_set {
            Some(ps) => CiName::new(ps),
            None => CiName::from_str(""),
        };
        let name_key = CiName::new(&prop.name.property_name);
        let key = (set_key, name_key);

        if prop.is_append {
            self.props.entry(key).or_default().push(prop);
        } else {
            // Override: replace all existing values
            self.props.insert(key, vec![prop]);
        }
    }

    /// Look up a property value by property set and name.
    ///
    /// Returns the value text of the most recent assignment, or `None`.
    /// For append properties, returns the first value; use `get_all` for
    /// the complete list.
    pub fn get(&self, set: &str, name: &str) -> Option<&str> {
        let set_key = CiName::from_str(set);
        let name_key = CiName::from_str(name);
        self.props
            .get(&(set_key, name_key))
            .and_then(|vals| vals.first())
            .map(|pv| pv.value.as_str())
    }

    /// Look up all property values for a given property set and name.
    ///
    /// Returns all values including appended ones, in order.
    pub fn get_all(&self, set: &str, name: &str) -> Vec<&str> {
        let set_key = CiName::from_str(set);
        let name_key = CiName::from_str(name);
        self.props
            .get(&(set_key, name_key))
            .map(|vals| vals.iter().map(|pv| pv.value.as_str()).collect())
            .unwrap_or_default()
    }

    /// Return the number of distinct property keys.
    pub fn len(&self) -> usize {
        self.props.len()
    }

    /// Check if the property map is empty.
    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// Iterate over all property entries.
    pub fn iter(&self) -> impl Iterator<Item = (&(CiName, CiName), &Vec<PropertyValue>)> {
        self.props.iter()
    }

    /// Collect properties for a component from its type and implementation.
    ///
    /// Applies the AADL inheritance rules:
    /// 1. Start with type-level properties.
    /// 2. Implementation properties override or append.
    /// 3. Subcomponent-level properties override or append.
    pub fn collect_for_component(
        tree: &ItemTree,
        component_type_idx: Option<ComponentTypeIdx>,
        component_impl_idx: Option<ComponentImplIdx>,
    ) -> PropertyMap {
        let mut map = PropertyMap::new();

        // 1. Collect from component type
        if let Some(ct_idx) = component_type_idx {
            let ct = &tree.component_types[ct_idx];
            for &pa_idx in &ct.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        // 2. Collect from component implementation (overrides type)
        if let Some(ci_idx) = component_impl_idx {
            let ci = &tree.component_impls[ci_idx];
            for &pa_idx in &ci.property_associations {
                let pa = &tree.property_associations[pa_idx];
                map.add(PropertyValue {
                    name: pa.name.clone(),
                    value: pa.value.clone(),
                    is_append: pa.is_append,
                });
            }
        }

        map
    }

    /// Collect properties for a subcomponent, layering on top of an
    /// inherited property map from the subcomponent's type/impl.
    pub fn collect_for_subcomponent(
        tree: &ItemTree,
        base: PropertyMap,
        subcomponent_idx: SubcomponentIdx,
    ) -> PropertyMap {
        let mut map = base;

        let sub = &tree.subcomponents[subcomponent_idx];
        for &pa_idx in &sub.property_associations {
            let pa = &tree.property_associations[pa_idx];
            map.add(PropertyValue {
                name: pa.name.clone(),
                value: pa.value.clone(),
                is_append: pa.is_append,
            });
        }

        map
    }

    /// Collect properties from a list of property association indices.
    pub fn from_associations(tree: &ItemTree, indices: &[PropertyAssociationIdx]) -> PropertyMap {
        let mut map = PropertyMap::new();
        for &pa_idx in indices {
            let pa = &tree.property_associations[pa_idx];
            map.add(PropertyValue {
                name: pa.name.clone(),
                value: pa.value.clone(),
                is_append: pa.is_append,
            });
        }
        map
    }
}
