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
    // M(3) + Q(2) + R(3) + C(1) = 9 devices each side
    assert_eq!((r.a_devices, r.b_devices), (9, 9));
    assert!(r.only_in_a_ports.is_empty() && r.only_in_b_ports.is_empty());
    assert!(r.unbalanced.is_empty(), "a true match has no unbalanced classes");
}

#[test]
fn analog_bandgap_miswired_resistor_mismatches() {
    let job = LvsJob::load("examples/bandgap/mismatch.lvs").expect("load bandgap mismatch job");
    let r = engine::run_job(&job).expect("run");
    assert!(!r.matched, "a mis-wired sense resistor must MISMATCH");
    // pure CONNECTIVITY error: device counts (and per-kind counts) still match —
    // unlike the dropped-device case above, this is not a count divergence.
    assert_eq!((r.a_devices, r.b_devices), (9, 9));
    assert!(r.device_kind_diff.is_empty(), "no device-count/kind divergence here");
    // the divergence surfaces as unmatched refinement classes
    assert!(!r.unbalanced.is_empty());
}
