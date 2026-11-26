//! `spar emit --format architecture-v1`: warte's architecture.v1 document
//! (its Architecture view) built from an AADL instance (#664).
//!
//! The contract is warte's `schemas/architecture.v1.json`, pinned at
//! 1f15e8094f6a35a81a773608ee320320c59ce9f0. The input is the root
//! implementation's direct subcomponents and its own connections, read
//! through the `PE_Arch` property set:
//!
//! - a subcomponent with `PE_Arch::Kind => "zone"` is a zone (`Label`,
//!   `Trust`, `Description`);
//! - every other subcomponent is a component (`Kind`, `Zone`, `Label`,
//!   `Host`, `Controller`, `Credentials` as `"name|kind|purpose"`);
//! - every connection of the root is a connection (`Protocol`,
//!   `Port_Number`, `Authenticated`, `Encrypted`, `Credential`,
//!   `Controller`), from and to the first element of its end paths.
//!
//! Ids are the AADL names lowercased with `_` turned into `-`. The exporter
//! copies names and kinds only. A missing required property, a value outside
//! the schema's enum, or two equal ids is an error naming the element, so the
//! document is never written schema-invalid. The cross-reference rules the
//! schema cannot express (zone and host targets, credential references,
//! secret-looking strings) are `warte arch check`'s, and the conformance test
//! runs it.

use serde::Serialize;
use spar_hir_def::instance::{ComponentInstanceIdx, SystemInstance};
use spar_hir_def::item_tree::PropertyExpr;
use spar_hir_def::properties::PropertyMap;

const SET: &str = "PE_Arch";

const TRUST: &[&str] = &["trusted", "internal", "dmz", "external"];
const COMPONENT_KIND: &[&str] = &[
    "host",
    "service",
    "session",
    "credential-store",
    "device",
    "external",
];
const CREDENTIAL_KIND: &[&str] = &[
    "private-key",
    "token",
    "password",
    "api-key",
    "certificate",
    "seed",
    "other",
];
const PROTOCOL: &[&str] = &["quic", "https", "ssh", "udp", "wireguard", "unix", "other"];

/// Fields of the document that do not come from the model.
pub struct SourceInfo<'a> {
    pub version: &'a str,
    pub model: &'a str,
    pub commit: Option<&'a str>,
    pub generated_at: &'a str,
}

#[derive(Serialize)]
struct Document {
    schema: &'static str,
    generated_at: String,
    source: Source,
    zones: Vec<Zone>,
    components: Vec<Component>,
    connections: Vec<Connection>,
}

#[derive(Serialize)]
struct Source {
    tool: &'static str,
    version: String,
    model: String,
    // Always written: `null` when the caller passed no commit.
    commit: Option<String>,
}

#[derive(Serialize)]
struct Zone {
    id: String,
    name: String,
    trust: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct Credential {
    name: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    purpose: Option<String>,
}

#[derive(Serialize)]
struct Component {
    id: String,
    name: String,
    kind: String,
    zone: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    controller: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    credentials: Vec<Credential>,
    aadl: String,
}

#[derive(Serialize)]
struct Connection {
    id: String,
    from: String,
    to: String,
    protocol: String,
    authenticated: bool,
    encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    controller: Vec<String>,
}

/// An architecture.v1 id from an AADL name: lowercase, `_` to `-`.
/// Errors if the result does not match `^[a-z0-9-]{1,64}$`.
fn to_id(name: &str, what: &str) -> Result<String, String> {
    let id: String = name
        .chars()
        .map(|c| {
            if c == '_' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "{what}: '{name}' gives id '{id}', which does not match ^[a-z0-9-]{{1,64}}$"
        ));
    }
    Ok(id)
}

fn get_string(props: &PropertyMap, name: &str, at: &str) -> Result<Option<String>, String> {
    match props.get_typed(SET, name) {
        None => Ok(None),
        Some(PropertyExpr::StringLit(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{at}: {SET}::{name} is not a string")),
    }
}

fn require_string(props: &PropertyMap, name: &str, at: &str) -> Result<String, String> {
    get_string(props, name, at)?.ok_or_else(|| format!("{at}: {SET}::{name} is missing"))
}

fn require_bool(props: &PropertyMap, name: &str, at: &str) -> Result<bool, String> {
    match props.get_typed(SET, name) {
        Some(PropertyExpr::Boolean(b)) => Ok(*b),
        None => Err(format!("{at}: {SET}::{name} is missing")),
        Some(_) => Err(format!("{at}: {SET}::{name} is not a boolean")),
    }
}

fn get_string_list(props: &PropertyMap, name: &str, at: &str) -> Result<Vec<String>, String> {
    match props.get_typed(SET, name) {
        None => Ok(Vec::new()),
        Some(PropertyExpr::List(items)) => items
            .iter()
            .map(|e| match e {
                PropertyExpr::StringLit(s) => Ok(s.clone()),
                _ => Err(format!("{at}: {SET}::{name} holds a non-string item")),
            })
            .collect(),
        Some(PropertyExpr::StringLit(s)) => Ok(vec![s.clone()]),
        Some(_) => Err(format!("{at}: {SET}::{name} is not a list of strings")),
    }
}

fn one_of(value: String, allowed: &[&str], name: &str, at: &str) -> Result<String, String> {
    if allowed.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(format!(
            "{at}: {SET}::{name} '{value}' is not one of {}",
            allowed.join(" | ")
        ))
    }
}

fn credential(raw: &str, at: &str) -> Result<Credential, String> {
    let mut parts = raw.splitn(3, '|');
    let name = parts.next().unwrap_or("").trim().to_string();
    let kind = parts.next().map(|s| s.trim().to_string());
    let purpose = parts.next().map(|s| s.trim().to_string());
    let Some(kind) = kind else {
        return Err(format!(
            "{at}: {SET}::Credentials item '{raw}' is not \"name|kind|purpose\""
        ));
    };
    if name.is_empty() {
        return Err(format!("{at}: {SET}::Credentials item '{raw}' has no name"));
    }
    let kind = one_of(kind, CREDENTIAL_KIND, "Credentials kind", at)?;
    Ok(Credential {
        name,
        kind,
        purpose: purpose.filter(|p| !p.is_empty()),
    })
}

/// Build the architecture.v1 document as pretty JSON with a final newline.
pub fn emit(inst: &SystemInstance, src: &SourceInfo<'_>) -> Result<String, String> {
    let root: ComponentInstanceIdx = inst.root;
    let mut zones = Vec::new();
    let mut components = Vec::new();
    let mut ids: Vec<(String, String)> = Vec::new();
    let mut seen = |id: &str, at: &str| -> Result<(), String> {
        if let Some((_, first)) = ids.iter().find(|(i, _)| i == id) {
            return Err(format!("{at}: id '{id}' is already used by {first}"));
        }
        ids.push((id.to_string(), at.to_string()));
        Ok(())
    };

    for &child in &inst.components[root].children {
        let c = &inst.components[child];
        let sub = c.name.as_str();
        let at = format!("subcomponent {sub}");
        let props = inst.properties_for(child);
        let kind = require_string(props, "Kind", &at)?;
        let id = to_id(sub, &at)?;
        seen(&id, &at)?;
        let name = get_string(props, "Label", &at)?.unwrap_or_else(|| id.clone());
        if kind == "zone" {
            zones.push(Zone {
                id,
                name,
                trust: one_of(require_string(props, "Trust", &at)?, TRUST, "Trust", &at)?,
                description: get_string(props, "Description", &at)?,
            });
            continue;
        }
        let kind = one_of(kind, COMPONENT_KIND, "Kind", &at)?;
        let zone = to_id(&require_string(props, "Zone", &at)?, &format!("{at} Zone"))?;
        let host = get_string(props, "Host", &at)?
            .map(|h| to_id(&h, &format!("{at} Host")))
            .transpose()?;
        let credentials = get_string_list(props, "Credentials", &at)?
            .iter()
            .map(|raw| credential(raw, &at))
            .collect::<Result<Vec<_>, _>>()?;
        components.push(Component {
            id,
            name,
            kind,
            zone,
            host,
            controller: get_string_list(props, "Controller", &at)?,
            credentials,
            aadl: format!("{}::{} ({sub})", c.package.as_str(), c.type_name.as_str()),
        });
    }

    let mut connections = Vec::new();
    for &ci in &inst.components[root].connections {
        let conn = &inst.connections[ci];
        let cname = conn.name.as_str();
        let at = format!("connection {cname}");
        let props = inst.properties_for_connection(ci);
        let id = to_id(cname, &at)?;
        seen(&id, &at)?;
        let end = |e: Option<&spar_hir_def::instance::ConnectionEnd>, side: &str| match e
            .and_then(|e| e.subcomponent.as_ref())
        {
            Some(s) => to_id(s.as_str(), &format!("{at} {side}")),
            None => Err(format!(
                "{at}: the {side} end is not on a subcomponent of the root"
            )),
        };
        let from = end(conn.src.as_ref(), "source")?;
        let to = end(conn.dst.as_ref(), "destination")?;
        let port = match props.get_typed(SET, "Port_Number") {
            None => None,
            Some(PropertyExpr::Integer(v, _)) if (0..=65535).contains(v) => Some(*v as u16),
            Some(_) => {
                return Err(format!(
                    "{at}: {SET}::Port_Number is not an integer in 0..65535"
                ));
            }
        };
        connections.push(Connection {
            id,
            from,
            to,
            protocol: one_of(
                require_string(props, "Protocol", &at)?,
                PROTOCOL,
                "Protocol",
                &at,
            )?,
            authenticated: require_bool(props, "Authenticated", &at)?,
            encrypted: require_bool(props, "Encrypted", &at)?,
            port,
            credential: get_string(props, "Credential", &at)?,
            controller: get_string_list(props, "Controller", &at)?,
        });
    }

    let doc = Document {
        schema: "architecture.v1",
        generated_at: src.generated_at.to_string(),
        source: Source {
            tool: "spar",
            version: src.version.to_string(),
            model: src.model.to_string(),
            commit: src.commit.map(str::to_string),
        },
        zones,
        components,
        connections,
    };
    let mut out = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

/// The current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(to_id("cloud_api", "t").unwrap(), "cloud-api");
        assert_eq!(to_id("Z_Office", "t").unwrap(), "z-office");
        assert!(to_id(&"a".repeat(65), "t").is_err());
        assert!(to_id("", "t").is_err());
    }

    #[test]
    fn credentials() {
        let c = credential("ssh-key|private-key|SSH to lab hosts", "t").unwrap();
        assert_eq!(
            (c.name.as_str(), c.kind.as_str()),
            ("ssh-key", "private-key")
        );
        assert_eq!(c.purpose.as_deref(), Some("SSH to lab hosts"));
        assert!(
            credential("api-token|token", "t")
                .unwrap()
                .purpose
                .is_none()
        );
        assert!(credential("no-kind", "t").is_err());
        assert!(credential("x|bearer|y", "t").is_err());
    }

    #[test]
    fn now_is_rfc3339_shaped() {
        let n = now_utc();
        assert_eq!(n.len(), 20, "{n}");
        assert!(n.starts_with("20") && n.ends_with('Z'), "{n}");
    }
}
