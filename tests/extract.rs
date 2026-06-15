//! Phase 2: native GDS extraction (via the vendored vyges-layout kernel) -> LVS.

use std::collections::BTreeSet;

use vyges_lvs::compare::compare;
use vyges_lvs::extract::{extract_file, Rules};
use vyges_lvs::spice::Netlist;

fn extracted() -> Netlist {
    let rules = Rules::load("examples/inv/inv.rules").expect("rules");
    extract_file("examples/inv/inverter.gds", Some("inverter"), &rules).expect("extract")
}

#[test]
fn extracts_two_transistors_with_correct_types_and_ports() {
    let lay = extracted();
    assert_eq!(lay.devices.len(), 2, "an inverter = 2 transistors");
    assert_eq!(lay.ports, ["A", "VDD", "VSS", "Y"], "ports from TEXT labels");
    let models: BTreeSet<&str> = lay.devices.iter().map(|d| d.model.as_str()).collect();
    assert_eq!(models, BTreeSet::from(["nfet", "pfet"]), "one nfet (n-diff) + one pfet (in nwell)");
    // bulk extracted: pfet bulk = nwell net (VDD via tap), nfet bulk = substrate (VSS)
    let bulk = |m: &str| lay.devices.iter().find(|d| d.model == m).unwrap().nodes[3].clone();
    assert_eq!(bulk("pfet"), "VDD", "pfet bulk = nwell tap -> VDD");
    assert_eq!(bulk("nfet"), "VSS", "nfet bulk = substrate");
}

#[test]
fn extracted_layout_matches_schematic() {
    let lay = extracted();
    let sch = Netlist::parse(&std::fs::read_to_string("examples/inv/schematic.spice").unwrap(), Some("inverter")).unwrap();
    let r = compare(&lay, &sch);
    assert!(r.matched, "extracted-from-GDS netlist should MATCH the schematic: {r:?}");
}

#[test]
fn hierarchical_with_via_matches() {
    // inv_hier places the inverter via an SREF and routes A up to met2 through a via;
    // extraction flattens it and resolves the via -> the same inverter.
    let rules = Rules::load("examples/inv/hier.rules").expect("rules");
    let lay = extract_file("examples/inv/inv_hier.gds", Some("inv_hier"), &rules).expect("extract");
    assert_eq!(lay.devices.len(), 2, "flattened SREF -> 2 transistors");
    assert_eq!(lay.ports, ["A", "VDD", "VSS", "Y"], "A stays one net across the met1->met2 via");
}

#[test]
fn wrong_schematic_mismatches() {
    let lay = extracted();
    // both-nfet is wrong (the real pfet is in nwell)
    let bad = Netlist::parse(
        ".subckt inverter VDD VSS A Y\nM0 Y A VDD VDD nfet\nM1 Y A VSS VSS nfet\n.ends\n",
        Some("inverter"),
    )
    .unwrap();
    assert!(!compare(&lay, &bad).matched, "a pfet->nfet error must MISMATCH");
}
