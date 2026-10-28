//! spar's instance model against OSATE's, component by component
//! (REQ-INTEROP-OSATE-INSTANCE-ORACLE-001).
//!
//! `test-data/osate2/instances/*.aaxl2` are instance models OSATE 2 wrote for
//! the `.aadl` models beside them. Until now they were read by hand: PR #517
//! (REQ-AADL-SUBCOMPONENT-REFINE-001) compared component COUNTS once and
//! recorded the result in prose. A count can agree while the trees differ,
//! and a one-off reading does not notice spar moving afterwards. This test
//! re-derives spar's side on every run and compares, per file:
//!
//! - the set of component instance paths (relative to the root);
//! - each component's category;
//! - each component's classifier, where OSATE resolved one;
//! - a few property values on components, where BOTH sides hold a value:
//!   `Timing_Properties::Period` (numerically, in picoseconds) and the
//!   `Deployment_Properties::Actual_{Processor,Memory,Connection,Function}_Binding`
//!   reference targets (as instance paths).
//!
//! # The ledger is a ratchet in both directions
//!
//! Every mismatch must appear in `KNOWN_DIVERGENCES.toml` beside the `.aaxl2`
//! files, one entry per (file, path, kind), each with a measured reason. An
//! unlisted mismatch fails. A listed entry that no longer occurs ALSO fails,
//! so when spar improves the ledger must shrink with it — an entry cannot
//! outlive the defect it describes and quietly excuse a different one later.
//!
//! # OSATE is a test, not the authority
//!
//! A ledger entry is not a verdict that spar is wrong. Its reason says which
//! side AS5506 supports where that is known. See
//! `tools/osate-conformance/README.md`.
//!
//! # What is NOT compared
//!
//! - features, connections, flows, modes, SOMs and annex (EMV2) instances;
//! - property associations held on feature or connection instances (most of
//!   the corpus's `Actual_Connection_Binding` and `Data_Size` values live
//!   there), and every property other than the five above;
//! - `Compute_Execution_Time`: no `.aaxl2` here carries a value for it, so a
//!   comparison would be code no data exercises;
//! - modal property values (`inModes`): skipped and counted;
//! - property values held by only one side: counted and printed, not failed —
//!   OSATE caches values a model never states, and the comparison is defined
//!   as "where both have a value";
//! - a classifier OSATE could not resolve (an Xtext `|::` proxy href): there
//!   is no OSATE value to compare, so it is skipped and counted.
//!
//! The XML is read by a minimal scanner below rather than an XML crate: none
//! is a workspace dependency, and adding one means cargo-vet work for a
//! test-only reader of 11 machine-written files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const INSTANCES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/osate2/instances"
);
const LEDGER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-data/osate2/instances/KNOWN_DIVERGENCES.toml"
);

/// Below this the corpus has lost files and the test is measuring less than
/// it did when written (11 files).
const MIN_FILES: usize = 10;

// ── Minimal XML scanner ────────────────────────────────────────────────

#[derive(Debug)]
struct Elem {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Elem>,
}

impl Elem {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Elem> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    fn kid(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.name == name)
    }
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse an XML document into an element tree. Text content, comments,
/// processing instructions and CDATA are skipped: the instance tree this test
/// reads is carried entirely in element names and attributes. Attribute
/// values are scanned quote-aware, because OSATE writes connection names such
/// as `a.output -> b.input` with a literal `>` inside the quotes.
fn parse_xml(src: &str) -> Elem {
    let b = src.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Elem> = vec![Elem {
        name: "#document".into(),
        attrs: vec![],
        children: vec![],
    }];
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if src[i..].starts_with("<?") {
            i += src[i..].find("?>").expect("unterminated <?") + 2;
            continue;
        }
        if src[i..].starts_with("<!--") {
            i += src[i..].find("-->").expect("unterminated comment") + 3;
            continue;
        }
        if src[i..].starts_with("<![CDATA[") {
            i += src[i..].find("]]>").expect("unterminated CDATA") + 3;
            continue;
        }
        if src[i..].starts_with("</") {
            let end = i + src[i..].find('>').expect("unterminated end tag");
            let name = src[i + 2..end].trim();
            let done = stack.pop().expect("end tag with empty stack");
            assert_eq!(done.name, name, "mismatched end tag at byte {i}");
            stack.last_mut().unwrap().children.push(done);
            i = end + 1;
            continue;
        }
        // Start or empty-element tag.
        i += 1;
        let name_start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' && b[i] != b'/' {
            i += 1;
        }
        let name = src[name_start..i].to_string();
        let mut attrs = Vec::new();
        let self_closing;
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            assert!(i < b.len(), "unterminated tag <{name}");
            if b[i] == b'/' {
                assert_eq!(b[i + 1], b'>', "stray '/' in <{name}");
                self_closing = true;
                i += 2;
                break;
            }
            if b[i] == b'>' {
                self_closing = false;
                i += 1;
                break;
            }
            let k_start = i;
            while b[i] != b'=' && !b[i].is_ascii_whitespace() {
                i += 1;
            }
            let key = src[k_start..i].to_string();
            while b[i] != b'"' && b[i] != b'\'' {
                i += 1;
            }
            let quote = b[i];
            i += 1;
            let v_start = i;
            while b[i] != quote {
                i += 1;
            }
            attrs.push((key, unescape(&src[v_start..i])));
            i += 1;
        }
        let elem = Elem {
            name,
            attrs,
            children: vec![],
        };
        if self_closing {
            stack.last_mut().unwrap().children.push(elem);
        } else {
            stack.push(elem);
        }
    }
    assert_eq!(stack.len(), 1, "unclosed elements: {}", stack.len());
    stack.pop().unwrap()
}

// ── The compared model ────────────────────────────────────────────────

/// A property value in a form both sides can be put into.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    /// A time, in picoseconds.
    TimePs(u128),
    /// Reference targets, as root-relative lowercase instance paths.
    Refs(Vec<String>),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::TimePs(ps) => write!(f, "{ps} ps"),
            Value::Refs(r) => write!(f, "refs({})", r.join(", ")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Time,
    Refs,
}

/// The properties compared: (property set, name, value kind).
const PROPERTIES: &[(&str, &str, Kind)] = &[
    ("Timing_Properties", "Period", Kind::Time),
    (
        "Deployment_Properties",
        "Actual_Processor_Binding",
        Kind::Refs,
    ),
    ("Deployment_Properties", "Actual_Memory_Binding", Kind::Refs),
    (
        "Deployment_Properties",
        "Actual_Connection_Binding",
        Kind::Refs,
    ),
    (
        "Deployment_Properties",
        "Actual_Function_Binding",
        Kind::Refs,
    ),
];

#[derive(Debug, Default)]
struct Comp {
    /// Normalised category: lowercase, no spaces (`threadgroup`).
    category: String,
    /// Lowercase `pkg.type[.impl]`; `None` when the side has no resolved one.
    classifier: Option<String>,
    /// Keyed by `Set::Name`.
    props: BTreeMap<String, Value>,
}

#[derive(Debug, Default)]
struct Tree {
    /// Keyed by root-relative lowercase path; the root is `""`.
    comps: BTreeMap<String, Comp>,
    /// Property associations not reducible to a compared [`Value`].
    skipped_props: usize,
    /// Classifiers OSATE left as unresolved proxies.
    unresolved_classifiers: usize,
}

fn norm_category(c: &str) -> String {
    c.chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn join(parent: &str, name: &str) -> String {
    let name = name.to_ascii_lowercase();
    if parent.is_empty() {
        name
    } else {
        format!("{parent}/{name}")
    }
}

// ── OSATE side ────────────────────────────────────────────────────────

/// AADL_Project::Time_Units, in declaration order, as picoseconds. OSATE
/// writes a unit as an index into `AADL_Project.aadl`
/// (`@ownedPropertyType.16/@ownedLiteral.N`), not as text. Index 16 is pinned
/// by DigitalControlSystem.aadl `sensor`'s `Period => 25 ms`, which OSATE
/// writes as `value="25"` with literal 3 — the fourth of ps, ns, us, ms. A
/// wrong table makes every Period comparison mismatch, so it cannot be wrong
/// silently.
const TIME_UNITS_TYPE: &str = "@ownedPropertyType.16";
const TIME_UNITS_PS: [u128; 7] = [
    1,
    1_000,
    1_000_000,
    1_000_000_000,
    1_000_000_000_000,
    60_000_000_000_000,
    3_600_000_000_000_000,
];

/// Turn an EMF reference such as `//@componentInstance.2/@componentInstance.0`
/// (bare `SystemInstance` root) or `/0/@componentInstance.4` (root wrapped in
/// `<xmi:XMI>`) into the index path of component children. `None` if it names
/// anything that is not a component (a feature, a connection).
fn emf_component_path(r: &str) -> Option<Vec<usize>> {
    let rest = r
        .strip_prefix("//")
        .or_else(|| r.strip_prefix("/0/"))
        .or_else(|| (r == "/0").then_some(""))?;
    let mut out = Vec::new();
    for seg in rest.split('/').filter(|s| !s.is_empty()) {
        out.push(seg.strip_prefix("@componentInstance.")?.parse().ok()?);
    }
    Some(out)
}

fn osate_tree(doc: &Elem) -> (Tree, String) {
    // Six files have the SystemInstance as the document element; five wrap it
    // in <xmi:XMI> beside detached top-level `instance:ComponentInstance`s
    // (classifier-value proxies such as `name="null"`). Only the
    // SystemInstance subtree is the instance tree.
    let top = &doc.children[0];
    let root = if top.name == "instance:SystemInstance" {
        top
    } else {
        top.kid("instance:SystemInstance")
            .expect("no instance:SystemInstance in the document")
    };
    let root_ref = root
        .kid("componentImplementation")
        .and_then(|e| e.attr("href"))
        .and_then(|h| h.split_once('#'))
        .map(|(_, frag)| frag.to_string())
        .expect("root has no componentImplementation href");

    // First pass: EMF index path -> relative path, for reference values.
    let mut index: BTreeMap<Vec<usize>, String> = BTreeMap::new();
    fn walk_index(
        e: &Elem,
        idx: &mut Vec<usize>,
        path: &str,
        out: &mut BTreeMap<Vec<usize>, String>,
    ) {
        out.insert(idx.clone(), path.to_string());
        for (n, c) in e.kids("componentInstance").enumerate() {
            idx.push(n);
            walk_index(c, idx, &join(path, c.attr("name").unwrap_or("")), out);
            idx.pop();
        }
    }
    walk_index(root, &mut Vec::new(), "", &mut index);

    let mut tree = Tree::default();
    fn walk(e: &Elem, path: &str, index: &BTreeMap<Vec<usize>, String>, tree: &mut Tree) {
        // EMF omits an enum attribute at its default; category's is abstract.
        let category = norm_category(e.attr("category").unwrap_or("abstract"));
        let classifier = match e
            .kid("classifier")
            .and_then(|c| c.attr("href"))
            .and_then(|h| h.split_once('#'))
        {
            Some((_, frag)) if !frag.contains("|::") && !frag.starts_with('/') => {
                Some(frag.to_ascii_lowercase())
            }
            _ => {
                tree.unresolved_classifiers += 1;
                None
            }
        };
        let mut props = BTreeMap::new();
        for pa in e.kids("ownedPropertyAssociation") {
            let Some((set, name)) = pa
                .kid("property")
                .and_then(|p| p.attr("href"))
                .and_then(|h| h.split_once('#'))
                .and_then(|(_, f)| f.split_once('.'))
            else {
                continue;
            };
            let Some(&(_, _, kind)) = PROPERTIES
                .iter()
                .find(|(s, n, _)| s.eq_ignore_ascii_case(set) && n.eq_ignore_ascii_case(name))
            else {
                continue;
            };
            let modal: Vec<&Elem> = pa.kids("ownedValue").collect();
            let value = match modal.as_slice() {
                [one] if one.attr("inMode").is_none() => one
                    .kid("ownedValue")
                    .and_then(|v| osate_value(v, kind, index)),
                _ => None,
            };
            match value {
                Some(v) => {
                    props.insert(format!("{set}::{name}"), v);
                }
                None => tree.skipped_props += 1,
            }
        }
        tree.comps.insert(
            path.to_string(),
            Comp {
                category,
                classifier,
                props,
            },
        );
        for c in e.kids("componentInstance") {
            walk(c, &join(path, c.attr("name").unwrap_or("")), index, tree);
        }
    }
    walk(root, "", &index, &mut tree);
    (tree, root_ref)
}

fn osate_value(v: &Elem, kind: Kind, index: &BTreeMap<Vec<usize>, String>) -> Option<Value> {
    let ty = v.attr("xsi:type")?;
    match kind {
        Kind::Time => {
            if ty != "aadl2:IntegerLiteral" {
                return None;
            }
            // EMF omits `value` when it is the default, 0.
            let n: u128 = v.attr("value").unwrap_or("0").parse().ok()?;
            let unit = v.kid("unit")?.attr("href")?;
            let (_, frag) = unit.split_once('#')?;
            let lit = frag.strip_prefix(&format!("/0/{TIME_UNITS_TYPE}/@ownedLiteral."))?;
            let factor = *TIME_UNITS_PS.get(lit.parse::<usize>().ok()?)?;
            Some(Value::TimePs(n * factor))
        }
        Kind::Refs => {
            let elems: Vec<&Elem> = match ty {
                "aadl2:ListValue" => v.kids("ownedListElement").collect(),
                "instance:InstanceReferenceValue" => vec![v],
                _ => return None,
            };
            let mut out = Vec::new();
            for e in elems {
                if e.attr("xsi:type") != Some("instance:InstanceReferenceValue") {
                    return None;
                }
                let p = emf_component_path(e.attr("referencedInstanceObject")?)?;
                out.push(index.get(&p)?.clone());
            }
            Some(Value::Refs(out))
        }
    }
}

// ── spar side ─────────────────────────────────────────────────────────

/// The `.aadl` files OSATE's instance names (`href="../X.aadl#..."`): the
/// files OSATE demonstrably resolved this instance against.
fn source_files(aaxl2: &str) -> Vec<PathBuf> {
    let mut set = BTreeSet::new();
    for chunk in aaxl2.split("href=\"../").skip(1) {
        if let Some((file, _)) = chunk.split_once('#') {
            set.insert(file.to_string());
        }
    }
    let dir = Path::new(INSTANCES).join("..");
    set.into_iter().map(|f| dir.join(f)).collect()
}

/// Split OSATE's `Pkg.Type.Impl` fragment; the package may itself hold `::`.
fn split_root(frag: &str) -> (String, String, String) {
    let mut parts: Vec<&str> = frag.rsplitn(3, '.').collect();
    parts.reverse();
    assert_eq!(parts.len(), 3, "root `{frag}` is not Pkg.Type.Impl");
    (parts[0].into(), parts[1].into(), parts[2].into())
}

/// Every `reference (x.y)` target in a raw property value text.
fn reference_targets(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(at) = rest.find("reference") {
        rest = &rest[at + "reference".len()..];
        let Some(open) = rest.find('(') else { break };
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        out.push(rest[open + 1..open + close].trim());
        rest = &rest[open + close..];
    }
    out
}

fn spar_tree(files: &[PathBuf], root: &str) -> Tree {
    use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};

    let (pkg, ty, im) = split_root(root);
    let db = spar_hir_def::HirDefDatabase::default();
    let trees: Vec<_> = files
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(f)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", f.display()));
            let sf = spar_base_db::SourceFile::new(&db, f.display().to_string(), src);
            spar_hir_def::file_item_tree(&db, sf)
        })
        .collect();
    let scope = spar_hir_def::GlobalScope::from_trees(trees);
    let inst = SystemInstance::instantiate(
        &scope,
        &spar_hir_def::Name::new(&pkg),
        &spar_hir_def::Name::new(&ty),
        &spar_hir_def::Name::new(&im),
    );

    fn rel_path(inst: &SystemInstance, idx: ComponentInstanceIdx) -> String {
        let mut parts = Vec::new();
        let mut cur = idx;
        while let Some(parent) = inst.component(cur).parent {
            let c = inst.component(cur);
            let mut seg = c.name.as_str().to_ascii_lowercase();
            if let Some(n) = c.array_index {
                seg.push_str(&format!("[{n}]"));
            }
            parts.push(seg);
            cur = parent;
        }
        parts.reverse();
        parts.join("/")
    }

    let mut tree = Tree::default();
    for (idx, c) in inst.all_components() {
        let classifier = Some(
            match &c.impl_name {
                Some(i) => format!("{}.{}.{}", c.package.as_str(), c.type_name.as_str(), i),
                None => format!("{}.{}", c.package.as_str(), c.type_name.as_str()),
            }
            .to_ascii_lowercase(),
        );
        let pm = inst.properties_for(idx);
        let mut props = BTreeMap::new();
        for &(set, name, kind) in PROPERTIES {
            let value = match kind {
                Kind::Time => spar_analysis::property_accessors::get_timing_property(pm, name)
                    .filter(|_| set == "Timing_Properties")
                    .map(|ps| Value::TimePs(ps as u128)),
                Kind::Refs => {
                    // Resolved exactly as binding_rules::check_binding_target
                    // does: from the declaring component, walking children.
                    let raw = pm.get(set, name).or_else(|| pm.get("", name));
                    raw.map(|raw| {
                        let ctx = inst.reference_context(idx, set, name);
                        Value::Refs(
                            reference_targets(raw)
                                .into_iter()
                                .map(|t| match inst.resolve_reference(ctx, t) {
                                    Some(target) => rel_path(&inst, target),
                                    None => format!("<unresolved {t}>"),
                                })
                                .collect(),
                        )
                    })
                }
            };
            if let Some(v) = value {
                props.insert(format!("{set}::{name}"), v);
            }
        }
        let prev = tree.comps.insert(
            rel_path(&inst, idx),
            Comp {
                category: norm_category(&c.category.to_string()),
                classifier,
                props,
            },
        );
        assert!(
            prev.is_none(),
            "spar produced two components at {}",
            rel_path(&inst, idx)
        );
    }
    tree
}

// ── Comparison and ledger ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    file: String,
    path: String,
    kind: String,
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} | /{} | {}", self.file, self.path, self.kind)
    }
}

#[derive(Debug, Default)]
struct FileStats {
    components: usize,
    classifiers_compared: usize,
    props_compared: usize,
    osate_only_props: usize,
    spar_only_props: usize,
}

fn compare(file: &str, osate: &Tree, spar: &Tree, stats: &mut FileStats) -> BTreeMap<Key, String> {
    let mut out = BTreeMap::new();
    let mut add = |path: &str, kind: &str, detail: String| {
        out.insert(
            Key {
                file: file.to_string(),
                path: path.to_string(),
                kind: kind.to_string(),
            },
            detail,
        );
    };
    for (path, o) in &osate.comps {
        let Some(s) = spar.comps.get(path) else {
            add(
                path,
                "missing-in-spar",
                format!("OSATE has a {}", o.category),
            );
            continue;
        };
        stats.components += 1;
        if o.category != s.category {
            add(
                path,
                "category",
                format!("OSATE {} vs spar {}", o.category, s.category),
            );
        }
        if let (Some(oc), Some(sc)) = (&o.classifier, &s.classifier) {
            stats.classifiers_compared += 1;
            if oc != sc {
                add(path, "classifier", format!("OSATE {oc} vs spar {sc}"));
            }
        }
        for (prop, ov) in &o.props {
            match s.props.get(prop) {
                Some(sv) => {
                    stats.props_compared += 1;
                    println!("    compared /{path} {prop}: OSATE {ov}, spar {sv}");
                    if ov != sv {
                        add(
                            path,
                            &format!("property:{prop}"),
                            format!("OSATE {ov} vs spar {sv}"),
                        );
                    }
                }
                None => stats.osate_only_props += 1,
            }
        }
        stats.spar_only_props += s.props.keys().filter(|k| !o.props.contains_key(*k)).count();
    }
    for (path, s) in &spar.comps {
        if !osate.comps.contains_key(path) {
            add(path, "extra-in-spar", format!("spar has a {}", s.category));
        }
    }
    out
}

/// Load the ledger. A missing or malformed ledger PANICS: falling back to an
/// empty ledger on error would read as "no known divergences", the ideal
/// reading, from a failure.
fn load_ledger() -> BTreeMap<Key, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct File {
        #[serde(default)]
        divergence: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry {
        file: String,
        path: String,
        kind: String,
        reason: String,
    }
    let text = std::fs::read_to_string(LEDGER)
        .unwrap_or_else(|e| panic!("cannot read {LEDGER}: {e} — the ledger is required"));
    let parsed: File =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{LEDGER} does not parse: {e}"));
    let mut out = BTreeMap::new();
    for e in parsed.divergence {
        assert!(
            !e.reason.trim().is_empty(),
            "ledger entry {}/{}/{} has no reason",
            e.file,
            e.path,
            e.kind
        );
        let key = Key {
            file: e.file,
            path: e.path,
            kind: e.kind,
        };
        assert!(
            !out.contains_key(&key),
            "duplicate ledger entry: {key} — one entry per (file, path, kind)"
        );
        out.insert(key, e.reason);
    }
    out
}

#[test]
fn the_scanner_reads_quoted_angle_brackets_and_both_reference_forms() {
    let doc = parse_xml(
        r#"<?xml version="1.0"?><a x="p -> q" y='1'><b/><!-- c --><c k="&amp;"></c></a>"#,
    );
    let a = &doc.children[0];
    assert_eq!(a.attr("x"), Some("p -> q"));
    assert_eq!(a.attr("y"), Some("1"));
    assert_eq!(a.children.len(), 2);
    assert_eq!(a.kid("c").unwrap().attr("k"), Some("&"));
    assert_eq!(
        emf_component_path("//@componentInstance.2/@componentInstance.0"),
        Some(vec![2, 0])
    );
    assert_eq!(emf_component_path("/0/@componentInstance.4"), Some(vec![4]));
    assert_eq!(emf_component_path("/0"), Some(vec![]));
    assert_eq!(
        emf_component_path("//@componentInstance.0/@featureInstance.1"),
        None
    );
    assert_eq!(
        split_root("A::B.T.i"),
        ("A::B".into(), "T".into(), "i".into())
    );
    assert_eq!(
        reference_targets("(reference (a.b), reference ( c ))"),
        vec!["a.b", "c"]
    );
}

// rivet: verifies REQ-INTEROP-OSATE-INSTANCE-ORACLE-001
#[test]
fn spar_instances_match_osate_except_the_ledgered_divergences() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(INSTANCES)
        .unwrap_or_else(|e| panic!("cannot list {INSTANCES}: {e}"))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "aaxl2"))
        .collect();
    files.sort();
    let found = files.len();
    assert!(
        found >= MIN_FILES,
        "only {found} .aaxl2 files in {INSTANCES}, expected >= {MIN_FILES} — a \
         shrunken corpus reports a clean result because it measured less"
    );

    let mut actual: BTreeMap<Key, String> = BTreeMap::new();
    let mut processed = 0;
    let mut total_components = 0;
    let mut total_props = 0;
    let mut total_classifiers = 0;
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).unwrap();
        let (osate, root) = osate_tree(&parse_xml(&text));
        assert!(
            osate.comps.len() > 1,
            "{name}: the scanner found {} component(s) — it did not descend \
             into the instance tree",
            osate.comps.len()
        );
        let sources = source_files(&text);
        assert!(!sources.is_empty(), "{name}: names no .aadl source");
        let spar = spar_tree(&sources, &root);
        let mut stats = FileStats::default();
        let diffs = compare(&name, &osate, &spar, &mut stats);
        println!(
            "{name}: root {root}; osate {} / spar {} components, {} compared, {} \
             classifiers compared; {} properties compared, {} OSATE-only, {} spar-only, {} OSATE \
             values skipped; {} OSATE classifiers unresolved; {} divergence(s)",
            osate.comps.len(),
            spar.comps.len(),
            stats.components,
            stats.classifiers_compared,
            stats.props_compared,
            stats.osate_only_props,
            stats.spar_only_props,
            osate.skipped_props,
            osate.unresolved_classifiers,
            diffs.len()
        );
        for (k, d) in &diffs {
            println!("    {k}: {d}");
        }
        total_components += stats.components;
        total_props += stats.props_compared;
        total_classifiers += stats.classifiers_compared;
        actual.extend(diffs);
        processed += 1;
    }
    println!(
        "TOTAL: {processed} files, {total_components} components compared, \
         {total_classifiers} classifiers compared, {total_props} property \
         values compared, {} divergence(s)",
        actual.len()
    );
    assert_eq!(processed, found, "every .aaxl2 found must be processed");
    assert!(total_components > 0, "no component was compared");
    assert!(total_props > 0, "no property value was compared");

    let ledger = load_ledger();
    let unlisted: Vec<String> = actual
        .iter()
        .filter(|(k, _)| !ledger.contains_key(*k))
        .map(|(k, d)| format!("{k}: {d}"))
        .collect();
    let stale: Vec<String> = ledger
        .iter()
        .filter(|(k, _)| !actual.contains_key(*k))
        .map(|(k, r)| format!("{k} ({r})"))
        .collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "spar's instance models diverge from OSATE's outside the ledger \
         {LEDGER}.\n\nNEW divergences (fix spar, or add a ledger entry with a \
         measured reason):\n  {}\n\nSTALE ledger entries that no longer occur \
         (remove them — the ledger only shrinks):\n  {}",
        unlisted.join("\n  "),
        stale.join("\n  ")
    );
}
