//! `spar emit --format zwinger-profile`: the `name` and `network` section of
//! a zwinger profile, generated from the architecture model.
//!
//! The seam is pulseengine/zwinger#17. zwinger owns the `Zwinger_Profile`
//! property set (its `architecture/zwinger_profile.aadl`, copied verbatim
//! into this crate's test fixtures) and the profile schema (`Profile` and
//! `Network` in its `src/profile.rs`). spar reads the session process named
//! by `--session` (an instance path below the root) and writes:
//!
//! - `name`: `Zwinger_Profile::Profile_Name` (required);
//! - `network.mode`: `Network_Mode`, where `confined` (the AADL literal,
//!   because `none` is an AADL keyword) is written `none` and `unfiltered`
//!   stays; absent means `none`, zwinger's fail-closed default;
//! - `network.allow`: the sorted, deduplicated `Zwinger_Profile::Host` of
//!   every end-to-end connection leaving the session's `egress` feature,
//!   each taken from the destination or its nearest ancestor that has one;
//! - `network.allow_local_binding`: `Allow_Local_Binding`, default false;
//! - `network.unix_sockets`: `Unix_Sockets`, default `confined`.
//!
//! Paths and `mach_lookup` stay hand-written in the profile.
//!
//! The YAML is written by `rivet_yaml::to_string`, the emitter of the
//! parser zwinger reads profiles with. Everything zwinger's
//! `Profile::validate` refuses about the network section is refused here
//! too, naming the model element: `unfiltered` with hosts, `unfiltered`
//! with local binding, and a Host that is empty or holds `*`, `/`, `:` or
//! whitespace.

use serde::Serialize;
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::PropertyExpr;
use std::collections::BTreeSet;

const SET: &str = "Zwinger_Profile";
/// The session feature whose connections are the profile's egress.
const EGRESS: &str = "egress";

#[derive(Serialize)]
struct Fragment {
    name: String,
    network: Network,
}

#[derive(Serialize)]
struct Network {
    mode: &'static str,
    allow: Vec<String>,
    allow_local_binding: bool,
    unix_sockets: &'static str,
}

/// The component at `path` (dot-separated subcomponent names) below the root.
fn find(inst: &SystemInstance, path: &str) -> Option<ComponentInstanceIdx> {
    let mut at = inst.root;
    for seg in path.split('.') {
        at = inst
            .component(at)
            .children
            .iter()
            .copied()
            .find(|&c| inst.component(c).name.as_str().eq_ignore_ascii_case(seg))?;
    }
    Some(at)
}

/// The instance path of `idx` below the root, for messages.
fn path_of(inst: &SystemInstance, idx: ComponentInstanceIdx) -> String {
    let mut segs = Vec::new();
    let mut at = Some(idx);
    while let Some(i) = at.filter(|&i| i != inst.root) {
        segs.push(inst.component(i).name.as_str().to_string());
        at = inst.component(i).parent;
    }
    segs.reverse();
    segs.join(".")
}

/// An enumeration property mapped through `map` (AADL literal to YAML value).
fn enumeration(
    inst: &SystemInstance,
    idx: ComponentInstanceIdx,
    prop: &str,
    map: &[(&str, &'static str)],
    at: &str,
) -> Result<Option<&'static str>, String> {
    match inst.properties_for(idx).get_typed(SET, prop) {
        None => Ok(None),
        Some(PropertyExpr::Enum(v)) => map
            .iter()
            .find(|(k, _)| v.as_str().eq_ignore_ascii_case(k))
            .map(|&(_, out)| Some(out))
            .ok_or_else(|| format!("{at}: {SET}::{prop} '{}' is not one of {map:?}", v.as_str())),
        Some(_) => Err(format!("{at}: {SET}::{prop} is not an enumeration literal")),
    }
}

/// zwinger's exact-host rule (`Profile::validate`): not empty, and no `*`,
/// `/`, `:` or whitespace.
fn exact_host(h: &str) -> bool {
    !(h.is_empty() || h.contains(['*', '/', ':']) || h.contains(char::is_whitespace))
}

pub fn emit(inst: &SystemInstance, session: &str) -> Result<String, String> {
    let idx = find(inst, session)
        .ok_or_else(|| format!("--session {session}: no such subcomponent below the root"))?;
    let at = format!("session {session}");
    let name = match inst.properties_for(idx).get_typed(SET, "Profile_Name") {
        Some(PropertyExpr::StringLit(s)) if !s.is_empty() => s.clone(),
        Some(_) => {
            return Err(format!(
                "{at}: {SET}::Profile_Name is not a non-empty string"
            ));
        }
        None => return Err(format!("{at}: {SET}::Profile_Name is missing")),
    };
    let mode = enumeration(
        inst,
        idx,
        "Network_Mode",
        &[("confined", "none"), ("unfiltered", "unfiltered")],
        &at,
    )?
    .unwrap_or("none");
    let unix_sockets = enumeration(
        inst,
        idx,
        "Unix_Sockets",
        &[("confined", "confined"), ("unfiltered", "unfiltered")],
        &at,
    )?
    .unwrap_or("confined");
    let allow_local_binding = match inst
        .properties_for(idx)
        .get_typed(SET, "Allow_Local_Binding")
    {
        None => false,
        Some(PropertyExpr::Boolean(b)) => *b,
        Some(_) => return Err(format!("{at}: {SET}::Allow_Local_Binding is not a boolean")),
    };

    let mut allow = BTreeSet::new();
    for sc in &inst.semantic_connections {
        if sc.ultimate_source.0 != idx
            || !sc.ultimate_source.1.as_str().eq_ignore_ascii_case(EGRESS)
        {
            continue;
        }
        let dst = sc.ultimate_destination.0;
        let mut owner = Some(dst);
        let host = loop {
            let Some(c) = owner else { break None };
            match inst.properties_for(c).get_typed(SET, "Host") {
                Some(PropertyExpr::StringLit(h)) => break Some(h.clone()),
                Some(_) => {
                    return Err(format!("{}: {SET}::Host is not a string", path_of(inst, c)));
                }
                None => owner = inst.component(c).parent,
            }
        };
        let conn = sc.name.as_str();
        let Some(host) = host else {
            return Err(format!(
                "connection {conn}: egress reaches {}, and neither it nor an ancestor has {SET}::Host",
                path_of(inst, dst)
            ));
        };
        if !exact_host(&host) {
            return Err(format!(
                "connection {conn}: {SET}::Host {host:?} is not an exact host name"
            ));
        }
        allow.insert(host);
    }

    if mode == "unfiltered" && !allow.is_empty() {
        return Err(format!(
            "{at}: Network_Mode unfiltered allows every host, but egress reaches {:?}; remove one of them",
            allow
        ));
    }
    if mode == "unfiltered" && allow_local_binding {
        return Err(format!(
            "{at}: Allow_Local_Binding is implied by Network_Mode unfiltered; remove one of them"
        ));
    }

    let doc = Fragment {
        name,
        network: Network {
            mode,
            allow: allow.into_iter().collect(),
            allow_local_binding,
            unix_sockets,
        },
    };
    rivet_yaml::to_string(&doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_host_mirrors_zwinger() {
        assert!(exact_host("api.anthropic.com"));
        assert!(exact_host("localhost"));
        for bad in [
            "",
            "*.example.com",
            "https://x",
            "x/y",
            "x:443",
            "a b",
            "a\tb",
        ] {
            assert!(!exact_host(bad), "{bad:?}");
        }
    }
}
