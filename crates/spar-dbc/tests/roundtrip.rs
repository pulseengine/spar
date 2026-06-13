//! The emitted AADL is verified by running it back through spar's own
//! `parse → ItemTree → GlobalScope → SystemInstance` pipeline. The parser is
//! the oracle: if the DBC→AADL emission is not well-formed, instantiation
//! reports diagnostics and these tests fail.

use spar_hir_def::instance::SystemInstance;
use spar_hir_def::{GlobalScope, HirDefDatabase, Name, file_item_tree};

use spar_dbc::dbc_to_aadl;

/// A small but representative DBC: three nodes (one deliberately named
/// `System`, an AADL reserved word, to exercise keyword escaping on a device),
/// two messages with signals, and a `Vector__XXX` (no-transmitter) frame.
const SAMPLE_DBC: &str = r#"VERSION "1.0"

NS_ :
    NS_DESC_
    CM_
    BA_DEF_
    BA_
    VAL_
    CAT_DEF_
    CAT_
    FILTER
    BA_DEF_DEF_
    EV_DATA_
    ENVVAR_DATA_
    SGTYPE_
    SGTYPE_VAL_
    BA_DEF_SGTYPE_
    BA_SGTYPE_
    SIG_TYPE_REF_
    VAL_TABLE_
    SIG_GROUP_
    SIG_VALTYPE_
    SIGTYPE_VALTYPE_
    BO_TX_BU_
    BA_DEF_REL_
    BA_REL_
    BA_DEF_DEF_REL_
    BU_SG_REL_
    BU_EV_REL_
    BU_BO_REL_
    SG_MUL_VAL_

BS_:

BU_: EngineECU System Gateway

BO_ 256 EngineData: 8 EngineECU
 SG_ RPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" Gateway,System
 SG_ Temp : 16|8@1+ (1,-40) [-40|215] "degC" Gateway

BO_ 512 Status: 2 Gateway
 SG_ State : 0|4@1+ (1,0) [0|15] "" EngineECU

BO_ 1024 Broadcast: 4 Vector__XXX
 SG_ Counter : 0|8@1+ (1,0) [0|255] "" Gateway
"#;

/// Drive AADL source text all the way to a `SystemInstance`, asserting zero
/// diagnostics at every stage. Returns the instance for further inspection.
fn instantiate(aadl: &str, package: &str) -> SystemInstance {
    let parsed = spar_syntax::parse(aadl);
    assert!(
        parsed.ok(),
        "emitted AADL did not parse: {:?}\n--- source ---\n{aadl}",
        parsed.errors()
    );

    let db = HirDefDatabase::default();
    let sf = spar_base_db::SourceFile::new(&db, "dbc.aadl".to_string(), aadl.to_string());
    let tree = file_item_tree(&db, sf);
    let scope = GlobalScope::from_trees(vec![tree]);
    assert!(
        scope.diagnostics.is_empty(),
        "scope diagnostics: {:?}\n--- source ---\n{aadl}",
        scope.diagnostics
    );

    let instance = SystemInstance::instantiate(
        &scope,
        &Name::new(package),
        &Name::new("CAN_Network"),
        &Name::new("impl"),
    );
    assert!(
        instance.diagnostics.is_empty(),
        "instance diagnostics: {:?}\n--- source ---\n{aadl}",
        instance.diagnostics
    );
    instance
}

#[test]
fn sample_dbc_emits_instantiable_aadl() {
    let aadl = dbc_to_aadl(SAMPLE_DBC, "CanDemo").expect("DBC should parse");
    // The keyword-named node must have been escaped (no bare `device System`).
    assert!(
        !aadl.contains("device System\n") && !aadl.contains("device system\n"),
        "reserved word `System` was emitted as a bare device name:\n{aadl}"
    );
    // Message frames carry their byte sizes.
    assert!(
        aadl.contains("data Msg_EngineData"),
        "missing engine frame:\n{aadl}"
    );
    assert!(
        aadl.contains("Data_Size => 8 Bytes;"),
        "missing 8-byte size:\n{aadl}"
    );
    assert!(
        aadl.contains("Data_Size => 2 Bytes;"),
        "missing 2-byte size:\n{aadl}"
    );

    let instance = instantiate(&aadl, "CanDemo");

    // Root system + bus + 3 devices = at least 5 component instances.
    let device_count = instance
        .components
        .iter()
        .filter(|(_, c)| format!("{:?}", c.category).contains("Device"))
        .count();
    assert_eq!(device_count, 3, "expected one device per DBC node");
}

#[test]
fn empty_network_still_instantiates() {
    // A header-only DBC (no nodes, no messages) must still yield a valid,
    // instantiable AADL package — just an empty network system + bus.
    const HEADER_ONLY: &str = "VERSION \"\"\n\nNS_ :\n\nBS_:\n\nBU_:\n";
    let aadl = dbc_to_aadl(HEADER_ONLY, "Empty").expect("header-only DBC should parse");
    let _ = instantiate(&aadl, "Empty");
}

#[test]
fn malformed_dbc_is_an_error() {
    let err = dbc_to_aadl("this is not a dbc file", "X");
    assert!(err.is_err(), "garbage input should be rejected");
}
