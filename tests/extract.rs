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

#[test]
fn extracts_channel_width_and_length() {
    // channel = poly(40 dbu wide) ∩ active(100 dbu): S/D flank it in X, so L is the
    // 40-dbu poly extent and W the 100-dbu active extent; db_unit 1e-9 -> L=40n, W=100n.
    let lay = extracted();
    for d in &lay.devices {
        let w = d.params.get("w").copied().expect("W extracted from geometry");
        let l = d.params.get("l").copied().expect("L extracted from geometry");
        assert!((l - 40e-9).abs() < 1e-12, "{} L = {l}", d.model);
        assert!((w - 100e-9).abs() < 1e-12, "{} W = {w}", d.model);
    }
}

#[test]
fn wrong_drawn_width_mismatches() {
    // the layout draws both devices at W=100n; a schematic that expects the nfet at
    // 50n is a real LVS error caught on the extracted geometry.
    let lay = extracted();
    let sch = Netlist::parse(
        ".subckt inverter VDD VSS A Y\n\
         M0 Y A VDD VDD pfet w=100n l=40n\n\
         M1 Y A VSS VSS nfet w=50n l=40n\n.ends\n",
        Some("inverter"),
    )
    .unwrap();
    let r = compare(&lay, &sch);
    assert!(!r.matched, "a layout drawn at the wrong width must MISMATCH");
    assert!(r.property_diffs.iter().any(|d| d.param == "w"), "{:?}", r.property_diffs);
}
