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
