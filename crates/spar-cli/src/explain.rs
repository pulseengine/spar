//! `spar instance --explain <path>` — property provenance
//! (REQ-AADL-PROPERTY-PROVENANCE-001).
//!
//! For one component instance, every property value spar uses, WHERE it came
//! from (the [`PropertySource`] recorded when instantiation inserted it — never
//! reconstructed here), and for each `reference (...)` term the component it
//! resolves to and the context component it is resolved from.
//!
//! Read-only: this renders the instance model the analyses see; it changes
//! nothing about it.

use serde_json::{Value, json};
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::{PropertySource, PropertyValue};

/// Resolve the `--explain` argument to a component instance, EXACTLY.
///
/// Accepted spellings: the root's own name (`Top.impl`), a path from the root
/// (`sub.app.t` or `sub/app/t`), or a full instance path as `spar` prints it
/// (`Top.impl/sub/app/t`). Each segment must name a child of the previous one;
/// there is no search, so the same name at two depths can never be confused.
pub fn resolve_instance_path(inst: &SystemInstance, path: &str) -> Option<ComponentInstanceIdx> {
    let path = path.trim();
    if path.eq_ignore_ascii_case(inst.component(inst.root).name.as_str()) {
        return Some(inst.root);
    }
    inst.resolve_reference(inst.root, path)
}

/// One `reference (...)` term of a value, resolved from `context`.
struct Reference {
    path: String,
    target: Option<String>,
}

/// Every `reference (path)` in raw value text, in order.
fn scan_text(text: &str, out: &mut Vec<String>) {
    let mut rest = text;
    while let Some(at) = rest.find("reference") {
        let after = &rest[at + "reference".len()..];
        let Some(open) = after.find('(') else { break };
        let inner = &after[open + 1..];
        let Some(close) = inner.find(')') else { break };
        out.push(inner[..close].trim().to_string());
        // From the `)`: it cannot begin another `reference`.
        rest = &inner[close..];
    }
}

/// The `reference (...)` paths in a value, in order: from the typed
/// expression when there is one (scanning the text of any part the parser
/// left opaque), otherwise from the value text.
fn reference_paths(pv: &PropertyValue) -> Vec<String> {
    fn walk(e: &PropertyExpr, out: &mut Vec<String>) {
        match e {
            PropertyExpr::ReferenceValue(s) => out.push(s.trim().to_string()),
            PropertyExpr::List(items) => items.iter().for_each(|i| walk(i, out)),
            PropertyExpr::Record(fields) => fields.iter().for_each(|(_, v)| walk(v, out)),
            PropertyExpr::UnitValue(inner, _) => walk(inner, out),
            PropertyExpr::Opaque(text) => scan_text(text, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    match &pv.typed_expr {
        Some(e) => walk(e, &mut out),
        None => scan_text(&pv.value, &mut out),
    }
    out.retain(|p| !p.is_empty());
    out
}

/// The `source` label, as the requirement names it.
fn source_label(inst: &SystemInstance, src: &PropertySource) -> String {
    match src {
        PropertySource::Classifier { .. } => "classifier".to_string(),
        PropertySource::Implementation { .. } => "implementation".to_string(),
        PropertySource::DeclaredLocal => "declared-local".to_string(),
        PropertySource::Refined {
            subcomponent,
            declared_in,
        } => format!("refined from {declared_in}/{subcomponent}"),
        PropertySource::Contained { declarer } => format!(
            "contained (applies to) from {}",
            inst.component_instance_path(*declarer)
        ),
        PropertySource::ContainedUnresolved { path } => {
            format!("unresolved applies to '{path}', kept on its declarer")
        }
        PropertySource::Inherited { ancestor, .. } => {
            format!("inherited from {}", inst.component_instance_path(*ancestor))
        }
        PropertySource::Unrecorded => "unrecorded".to_string(),
    }
}

/// The classifier that wrote a classifier/implementation value, if any.
fn declared_in(src: &PropertySource) -> Option<&str> {
    match src {
        PropertySource::Classifier { classifier }
        | PropertySource::Implementation { classifier } => Some(classifier),
        _ => None,
    }
}

/// The source for text output: the label, the declaring classifier, and for
/// an inherited value how it reached the ancestor.
fn source_text(inst: &SystemInstance, src: &PropertySource) -> String {
    let mut s = source_label(inst, src);
    if let Some(c) = declared_in(src) {
        s.push_str(&format!(" {c}"));
    }
    if let PropertySource::Inherited { origin, .. } = src {
        s.push_str(&format!(" (there: {})", source_text(inst, origin)));
    }
    s
}

fn source_json(inst: &SystemInstance, src: &PropertySource) -> Value {
    let mut v = json!({ "source": source_label(inst, src) });
    if let Some(c) = declared_in(src) {
        v["declared_in"] = json!(c);
    }
    if let PropertySource::Inherited { origin, .. } = src {
        v["origin"] = source_json(inst, origin);
    }
    v
}

struct ExplainedValue<'a> {
    value: &'a PropertyValue,
    source: &'a PropertySource,
    references: Vec<Reference>,
}

struct ExplainedProperty<'a> {
    name: String,
    /// Where `reference (...)` terms of this key resolve from.
    context: String,
    values: Vec<ExplainedValue<'a>>,
}

/// Whether the `--property` filter selects a value named `name`. `Set::Name`
/// matches that set or an unqualified value of that name; a bare `Name`
/// matches it in any set.
fn matches_filter(filter: &str, name: &spar_hir_def::name::PropertyRef) -> bool {
    let (set, prop) = match filter.rsplit_once("::") {
        Some((s, p)) => (Some(s.trim()), p.trim()),
        None => (None, filter.trim()),
    };
    if !name.property_name.as_str().eq_ignore_ascii_case(prop) {
        return false;
    }
    match (set, &name.property_set) {
        (Some(want), Some(have)) => have.as_str().eq_ignore_ascii_case(want),
        _ => true,
    }
}

fn explain<'a>(
    inst: &'a SystemInstance,
    comp: ComponentInstanceIdx,
    filter: Option<&str>,
) -> Vec<ExplainedProperty<'a>> {
    let props = inst.properties_for(comp);
    let mut out: Vec<ExplainedProperty<'a>> = Vec::new();
    for (key, values) in props.iter() {
        let Some(first) = values.first() else {
            continue;
        };
        if filter.is_some_and(|f| !matches_filter(f, &first.name)) {
            continue;
        }
        let ctx = props.reference_context_for_key(key).unwrap_or(comp);
        let sources = props.sources_for_key(key);
        let values = values
            .iter()
            .enumerate()
            .map(|(i, value)| ExplainedValue {
                value,
                source: sources.get(i).unwrap_or(&PropertySource::Unrecorded),
                references: reference_paths(value)
                    .into_iter()
                    .map(|path| Reference {
                        target: inst
                            .resolve_reference(ctx, &path)
                            .map(|t| inst.component_instance_path(t)),
                        path,
                    })
                    .collect(),
            })
            .collect();
        out.push(ExplainedProperty {
            name: first.name.to_string(),
            context: inst.component_instance_path(ctx),
            values,
        });
    }
    out.sort_by_key(|p| p.name.to_ascii_lowercase());
    out
}

/// Render the explanation and return the exit code: 0 when something was
/// explained, 1 when `--property` selected nothing (an empty answer to a
/// specific question must not look like success).
pub fn run(
    inst: &SystemInstance,
    comp: ComponentInstanceIdx,
    filter: Option<&str>,
    json_out: bool,
) -> i32 {
    let c = inst.component(comp);
    let path = inst.component_instance_path(comp);
    let impl_suffix = c
        .impl_name
        .as_ref()
        .map(|i| format!(".{i}"))
        .unwrap_or_default();
    let classifier = format!("{}::{}{}", c.package, c.type_name, impl_suffix);
    let explained = explain(inst, comp, filter);

    if json_out {
        let properties: Vec<Value> = explained
            .iter()
            .map(|p| {
                let values: Vec<Value> = p
                    .values
                    .iter()
                    .map(|v| {
                        let mut o = source_json(inst, v.source);
                        o["value"] = json!(v.value.value);
                        o["append"] = json!(v.value.is_append);
                        o["references"] = Value::Array(
                            v.references
                                .iter()
                                .map(|r| {
                                    json!({
                                        "path": r.path,
                                        "context": p.context,
                                        "target": r.target,
                                    })
                                })
                                .collect(),
                        );
                        o
                    })
                    .collect();
                json!({ "property": p.name, "values": values })
            })
            .collect();
        let doc = json!({
            "instance": path,
            "category": c.category.to_string(),
            "classifier": classifier,
            "properties": properties,
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        println!("{path} : {} {classifier}", c.category);
        if explained.is_empty() {
            println!("  (no property values)");
        }
        for p in &explained {
            for v in &p.values {
                let op = if v.value.is_append { "+=>" } else { "=>" };
                println!("  {} {op} {}", p.name, v.value.value);
                println!("    source: {}", source_text(inst, v.source));
                for r in &v.references {
                    let target = r.target.as_deref().unwrap_or("<unresolved>");
                    println!(
                        "    reference ({}) -> {target}, resolved from {}",
                        r.path, p.context
                    );
                }
            }
        }
    }

    if filter.is_some() && explained.is_empty() {
        eprintln!(
            "error: {path} has no value for property '{}'",
            filter.unwrap_or_default()
        );
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use spar_hir_def::{Name, PropertyRef};

    fn pv(value: &str, typed: Option<PropertyExpr>) -> PropertyValue {
        PropertyValue {
            name: PropertyRef {
                property_set: None,
                property_name: Name::new("P"),
            },
            value: value.to_string(),
            typed_expr: typed,
            is_append: false,
        }
    }

    fn r(s: &str) -> PropertyExpr {
        PropertyExpr::ReferenceValue(s.to_string())
    }

    // rivet: verifies REQ-AADL-PROPERTY-PROVENANCE-001
    #[test]
    fn every_reference_in_a_value_is_found() {
        // A list, a record field, a unit-wrapped value, and an opaque part.
        let typed = PropertyExpr::List(vec![
            r(" a.cpu "),
            PropertyExpr::Record(vec![(Name::new("f"), r("b"))]),
            PropertyExpr::UnitValue(Box::new(r("c")), Name::new("u")),
            PropertyExpr::Opaque("reference (d) reference(e)".to_string()),
            PropertyExpr::Integer(1, None),
            r("  "),
        ]);
        assert_eq!(
            reference_paths(&pv("(ignored)", Some(typed))),
            ["a.cpu", "b", "c", "d", "e"]
        );
        // No typed expression: the text is scanned, every occurrence.
        assert_eq!(
            reference_paths(&pv("(reference (x), reference ( y.z ))", None)),
            ["x", "y.z"]
        );
        // Malformed text stops the scan without a panic.
        assert!(reference_paths(&pv("reference x", None)).is_empty());
        assert!(reference_paths(&pv("reference (x", None)).is_empty());
        assert!(reference_paths(&pv("10 ms", None)).is_empty());
    }
}
