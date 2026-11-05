//! Property associations whose property set is declared nowhere
//! (REQ-AADL-UNDECLARED-PROPERTY-SET-001, #445).
//!
//! `Nonsense_Properties::Not_A_Real_Thing => 16#DEADBEEF#;` names a property
//! set that no loaded file declares and that is not one of the predeclared
//! sets (AS5506 Appendix A plus spar's own). Up to v0.48.1 spar parsed,
//! instantiated and analysed such a model without a word, so a misspelt or
//! invented property looked exactly like a satisfied one.
//!
//! The check reports each such set once per file, naming the set and the
//! associations that use it. An unknown set is a Warning. A set that OSATE
//! itself contributes through a plugin ([`OSATE_PLUGIN_SETS`]) is an Info:
//! OSATE models use them without a file, and on the vendored corpus they are
//! 136 of the 160 reports (Data_Model, SEI, ARINC653 lead), so as warnings
//! they would bury the misspelt and unloaded sets this check exists for.

use rustc_hash::FxHashMap;
use spar_hir_def::GlobalScope;
use spar_hir_def::item_tree::ItemTree;

use crate::{AnalysisDiagnostic, Severity};

const ANALYSIS_NAME: &str = "property_sets";

/// Property sets an OSATE plugin contributes, so a model can use them with no
/// declaring file. Compared case-insensitively.
const OSATE_PLUGIN_SETS: &[&str] = &[
    "SEI",
    "Data_Model",
    "ARINC653",
    "ARINC429",
    "Physical",
    "Code_Generation_Properties",
    "EMV2",
    "Behavior_Properties",
];

/// How many associations to name before eliding the rest.
const MAX_NAMED: usize = 3;

/// Report every property set that `tree`'s associations name and `scope`
/// does not know.
pub fn check_property_set_refs(tree: &ItemTree, scope: &GlobalScope) -> Vec<AnalysisDiagnostic> {
    let known: Vec<String> = scope
        .property_set_names()
        .iter()
        .map(|n| n.as_str().to_ascii_lowercase())
        .collect();

    // Unknown set (lowercase) -> (spelling as first written, association names).
    let mut unknown: FxHashMap<String, (String, Vec<String>)> = FxHashMap::default();
    let mut order: Vec<String> = Vec::new();
    for (_idx, assoc) in tree.property_associations.iter() {
        let Some(set) = &assoc.name.property_set else {
            continue;
        };
        let key = set.as_str().to_ascii_lowercase();
        if known.contains(&key) {
            continue;
        }
        let entry = unknown.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            (set.as_str().to_string(), Vec::new())
        });
        let shown = assoc.name.to_string();
        if !entry.1.contains(&shown) {
            entry.1.push(shown);
        }
    }

    let path: Vec<String> = tree
        .packages
        .iter()
        .map(|(_, p)| p.name.as_str().to_string())
        .collect();

    order
        .iter()
        .map(|key| {
            let (set, names) = &unknown[key];
            let mut listed = names
                .iter()
                .take(MAX_NAMED)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            if names.len() > MAX_NAMED {
                listed.push_str(&format!(", and {} more", names.len() - MAX_NAMED));
            }
            let plugin = OSATE_PLUGIN_SETS
                .iter()
                .any(|p| p.eq_ignore_ascii_case(set));
            let (severity, message) = if plugin {
                (
                    Severity::Info,
                    format!(
                        "property set '{set}' is contributed by an OSATE plugin and no loaded \
                         file declares it, so {listed} cannot be checked; load its declaring \
                         file to check them"
                    ),
                )
            } else {
                (
                    Severity::Warning,
                    format!(
                        "property set '{set}' is not declared in any loaded file and is not a \
                         predeclared set, so {listed} cannot be checked; load the file that \
                         declares '{set}', or correct the name"
                    ),
                )
            };
            AnalysisDiagnostic {
                severity,
                message,
                path: path.clone(),
                analysis: ANALYSIS_NAME.to_string(),
            }
        })
        .collect()
}
