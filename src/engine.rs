//! Engine: load the two netlists, compare, render the verdict + divergence report.

use crate::compare::{self, LvsResult};
use crate::job::LvsJob;
use crate::spice::Netlist;

fn load(path: &str, top: Option<&str>) -> Result<Netlist, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    Netlist::parse(&text, top).map_err(|e| format!("{path}: {e}"))
}

pub fn run_job(job: &LvsJob) -> Result<LvsResult, String> {
    // side A: a layout-extracted SPICE netlist, OR natively extracted from a GDS
    let a = match (&job.layout_gds, &job.layout) {
        (Some(gds), _) => {
            let rpath = job
                .rules
                .as_deref()
                .ok_or("`layout_gds` requires `rules`")?;
            let rules = crate::extract::Rules::load(&job.resolve(rpath))?;
            crate::extract::extract_file(&job.resolve(gds), job.top.as_deref(), &rules)?
        }
        (None, Some(spice)) => load(&job.resolve(spice), job.top.as_deref())?,
        (None, None) => return Err("need `layout` or `layout_gds`".into()),
    };
    let b = load(&job.resolve(&job.schematic), job.top.as_deref())?;
    Ok(compare::compare(&a, &b))
}

/// A built-in matching pair — `vyges-lvs demo`.
pub fn demo() -> LvsResult {
    let a = Netlist::parse(DEMO_A, None).unwrap();
    let b = Netlist::parse(DEMO_B, None).unwrap();
    compare::compare(&a, &b)
}

pub fn render_report(r: &LvsResult) -> String {
    let mut s = String::new();
    let verdict = if r.matched {
        "MATCH ✓"
    } else {
        "MISMATCH ✗"
    };
    s.push_str(&format!("vyges-lvs — {verdict}\n"));
    s.push_str(&format!(
        "  devices   A {}  B {}\n  nets      A {}  B {}\n  refine    {} iteration(s)\n",
        r.a_devices, r.b_devices, r.a_nets, r.b_nets, r.iterations
    ));
    if !r.only_in_a_ports.is_empty() || !r.only_in_b_ports.is_empty() {
        s.push_str(&format!(
            "  ports     only in layout: [{}]   only in schematic: [{}]\n",
            r.only_in_a_ports.join(", "),
            r.only_in_b_ports.join(", ")
        ));
    }
    for (k, a, b) in &r.device_kind_diff {
        s.push_str(&format!(
            "  device count differs: '{k}'  layout {a}  schematic {b}\n"
        ));
    }
    if r.matched {
        if r.verified {
            s.push_str("\n  the two netlists are structurally equivalent (verified by explicit isomorphism).\n");
        } else {
            s.push_str("\n  the two netlists are structurally equivalent.\n");
            if let Some(n) = &r.note {
                s.push_str(&format!("  note: {n}\n"));
            }
        }
        return s;
    }
    if !r.property_diffs.is_empty() {
        s.push_str("\n  device parameter mismatch (topology matches):\n");
        for d in r.property_diffs.iter().take(12) {
            s.push_str(&format!(
                "    {} {}: {} layout {} vs schematic {}\n",
                d.kind, d.a_device, d.param, d.a_value, d.b_value
            ));
        }
        if r.property_diffs.len() > 12 {
            s.push_str(&format!("    … {} more\n", r.property_diffs.len() - 12));
        }
        return s;
    }
    if let Some(n) = &r.note {
        s.push_str(&format!("\n  {n}\n"));
        return s;
    }
    s.push_str("\n  divergence (unmatched refinement classes):\n");
    for u in r.unbalanced.iter().take(12) {
        s.push_str(&format!(
            "    {} class: layout {} vs schematic {}\n      layout:    {}\n      schematic: {}\n",
            u.what,
            u.a_count,
            u.b_count,
            if u.a_examples.is_empty() {
                "—".into()
            } else {
                u.a_examples.join(", ")
            },
            if u.b_examples.is_empty() {
                "—".into()
            } else {
                u.b_examples.join(", ")
            },
        ));
    }
    if r.unbalanced.len() > 12 {
        s.push_str(&format!(
            "    … {} more class(es)\n",
            r.unbalanced.len() - 12
        ));
    }
    s
}

pub fn report_json(r: &LvsResult) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"matched\": {},\n", r.matched));
    s.push_str(&format!("  \"verified\": {},\n", r.verified));
    // The single LVS verdict, tri-state on purpose.
    //
    // A MATCH that the bounded search could not confirm with an explicit
    // bijection (`matched` without `verified`) is not the same claim as a proven
    // MATCH: only the necessary condition held, and the sufficient one was left
    // open. Reporting that as a pass would overstate the evidence, and reporting
    // it as a mismatch would invent a defect — so it is `null`, and `note` says
    // why. Consumers wanting one pass/fail read this, not `matched`.
    let lvs_met = match (r.matched, r.verified) {
        (false, _) => "false".to_string(), // MISMATCH — a real, reported difference
        (true, true) => "true".to_string(), // MATCH proven by explicit isomorphism
        (true, false) => "null".to_string(), // MATCH unconfirmed — inconclusive
    };
    s.push_str(&format!("  \"lvs_met\": {lvs_met},\n"));
    if let Some(n) = &r.note {
        s.push_str(&format!("  \"note\": {},\n", jstr(n)));
    }
    s.push_str(&format!(
        "  \"a_devices\": {}, \"b_devices\": {},\n",
        r.a_devices, r.b_devices
    ));
    s.push_str(&format!(
        "  \"a_nets\": {}, \"b_nets\": {},\n",
        r.a_nets, r.b_nets
    ));
    s.push_str(&format!("  \"iterations\": {},\n", r.iterations));
    s.push_str(&format!(
        "  \"only_in_a_ports\": [{}],\n",
        jlist(&r.only_in_a_ports)
    ));
    s.push_str(&format!(
        "  \"only_in_b_ports\": [{}],\n",
        jlist(&r.only_in_b_ports)
    ));
    s.push_str("  \"unbalanced\": [\n");
    for (k, u) in r.unbalanced.iter().enumerate() {
        let comma = if k + 1 < r.unbalanced.len() { "," } else { "" };
        s.push_str(&format!(
            "    {{\"what\": {}, \"a_count\": {}, \"b_count\": {}, \"a\": [{}], \"b\": [{}]}}{}\n",
            jstr(u.what),
            u.a_count,
            u.b_count,
            jlist(&u.a_examples),
            jlist(&u.b_examples),
            comma
        ));
    }
    s.push_str("  ],\n");
    s.push_str("  \"property_diffs\": [\n");
    for (k, d) in r.property_diffs.iter().enumerate() {
        let comma = if k + 1 < r.property_diffs.len() {
            ","
        } else {
            ""
        };
        s.push_str(&format!(
            "    {{\"kind\": {}, \"a_device\": {}, \"b_device\": {}, \"param\": {}, \
             \"a_value\": {}, \"b_value\": {}}}{}\n",
            jstr(&d.kind.to_string()),
            jstr(&d.a_device),
            jstr(&d.b_device),
            jstr(&d.param),
            d.a_value,
            d.b_value,
            comma
        ));
    }
    s.push_str("  ]\n}\n");
    s
}

fn jstr(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
fn jlist(items: &[String]) -> String {
    items.iter().map(|s| jstr(s)).collect::<Vec<_>>().join(", ")
}

const DEMO_A: &str = "\
.subckt inv A Y VDD VSS
Mp Y A VDD VDD pfet
Mn Y A VSS VSS nfet
.ends
";
// same inverter — instance names + device order changed (layout-extracted style)
const DEMO_B: &str = "\
.subckt inv A Y VDD VSS
M_2 Y A VSS VSS nfet
M_1 Y A VDD VDD pfet
.ends
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_matches() {
        let r = demo();
        // same inverter, renamed/reordered -> a clean MATCH
        assert!(r.matched, "demo should MATCH: {r:?}");
        let txt = render_report(&r);
        assert!(txt.contains("MATCH"));
        assert!(report_json(&r).contains("\"matched\": true"));
        // a proven MATCH is a pass, not merely a match
        assert!(r.verified);
        assert!(report_json(&r).contains("\"lvs_met\": true"));
    }

    /// The three `lvs_met` states, in particular the middle one: a MATCH the
    /// bounded search could not confirm is inconclusive, not a pass.
    #[test]
    fn lvs_met_is_tri_state() {
        let mut r = LvsResult {
            matched: true,
            verified: true,
            ..Default::default()
        };
        assert!(
            report_json(&r).contains("\"lvs_met\": true"),
            "proven MATCH -> true"
        );

        r.verified = false;
        assert!(
            report_json(&r).contains("\"lvs_met\": null"),
            "unconfirmed MATCH must not claim a pass"
        );

        r.matched = false;
        assert!(
            report_json(&r).contains("\"lvs_met\": false"),
            "MISMATCH -> false"
        );
    }
}
