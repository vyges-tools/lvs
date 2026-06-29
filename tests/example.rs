//! End-to-end: the inverter-chain example matches; the bugged one diverges.

use vyges_lvs::engine;
use vyges_lvs::job::LvsJob;

#[test]
fn renamed_layout_matches_schematic() {
    let job = LvsJob::load("examples/inv_chain/match.lvs").expect("load match job");
    let r = engine::run_job(&job).expect("run");
    assert!(r.matched, "renamed/reordered same chain should MATCH: {r:?}");
    assert_eq!((r.a_devices, r.b_devices), (4, 4));
    assert!(r.only_in_a_ports.is_empty() && r.only_in_b_ports.is_empty());
}

#[test]
fn dropped_device_mismatches_with_diagnostics() {
    let job = LvsJob::load("examples/inv_chain/mismatch.lvs").expect("load mismatch job");
    let r = engine::run_job(&job).expect("run");
    assert!(!r.matched, "a dropped pfet must MISMATCH");
    assert_eq!((r.a_devices, r.b_devices), (3, 4));
    // device-count-by-kind diff is surfaced
    assert!(r.device_kind_diff.iter().any(|(k, a, b)| *k == 'M' && *a == 3 && *b == 4));
    // and at least one unmatched refinement class is reported
    assert!(!r.unbalanced.is_empty());
}

// --- Analog (mixed-signal) coverage: bandgap reference ---------------------
// Exercises device kinds a digital standard-cell LVS never sees — bipolar
// transistors (Q), resistors (R), a capacitor (C) — alongside a PMOS mirror (M),
// proving the graph compare is domain-agnostic, not std-cell-specific.

#[test]
fn analog_bandgap_renamed_layout_matches_schematic() {
    let job = LvsJob::load("examples/bandgap/match.lvs").expect("load bandgap match job");
    let r = engine::run_job(&job).expect("run");
    assert!(r.matched, "renamed/reordered analog bandgap should MATCH: {r:?}");
    // The two series resistors on the private node a3 combine, so both sides
    // normalize to M(3) + Q(2) + R(2) + C(1) = 8 devices.
    assert_eq!((r.a_devices, r.b_devices), (8, 8));
    assert!(r.only_in_a_ports.is_empty() && r.only_in_b_ports.is_empty());
    assert!(r.unbalanced.is_empty(), "a true match has no unbalanced classes");
}

#[test]
fn analog_bandgap_miswired_resistor_mismatches() {
    let job = LvsJob::load("examples/bandgap/mismatch.lvs").expect("load bandgap mismatch job");
    let r = engine::run_job(&job).expect("run");
    assert!(!r.matched, "a mis-wired sense resistor must MISMATCH");
    // The miswire moves R_d off a3, so a3 no longer forms a private series node:
    // the bug's resistors don't combine (3) while the schematic's do (2). The error
    // surfaces as a resistor-count divergence plus unmatched classes — still caught.
    assert_eq!((r.a_devices, r.b_devices), (9, 8));
    assert!(r.device_kind_diff.iter().any(|(k, _, _)| *k == 'R'));
    assert!(!r.unbalanced.is_empty());
}
