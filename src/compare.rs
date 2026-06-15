//! Netlist comparison by graph colour-refinement (1-WL).
//!
//! Two netlists describe the same circuit iff their device/net graphs are
//! isomorphic. We refine colours on the **disjoint union** of both graphs so the
//! colours are directly comparable: a device's colour folds in its kind/model and
//! its terminals' net-colours (in order); a net's colour folds in the multiset of
//! (device-colour, terminal-position) it touches. Iterating to a fixed point, the
//! two sides MATCH iff every colour class has equal counts on each side — and the
//! classes that *don't* balance are the divergence report.
//!
//! Ports are anchored by name (the layout/schematic boundary), so corresponding
//! ports align; internal nets are matched purely by structure. v0 bound: 1-WL can't
//! separate certain symmetric graphs (exact backtracking is the depth pass), and
//! terminals are position-sensitive (source/drain symmetry is a depth item).

use std::collections::BTreeMap;

use crate::spice::Netlist;

#[derive(Debug, Clone)]
pub struct Unbalanced {
    pub what: &'static str, // "device" | "net"
    pub a_count: usize,
    pub b_count: usize,
    pub a_examples: Vec<String>,
    pub b_examples: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LvsResult {
    pub matched: bool,
    pub a_devices: usize,
    pub b_devices: usize,
    pub a_nets: usize,
    pub b_nets: usize,
    pub only_in_a_ports: Vec<String>,
    pub only_in_b_ports: Vec<String>,
    pub device_kind_diff: Vec<(char, usize, usize)>, // (kind, a, b) where they differ
    pub unbalanced: Vec<Unbalanced>,
    pub iterations: usize,
}

struct Graph {
    // devices and nets, each tagged with side 0 (A) / 1 (B)
    dev_side: Vec<u8>,
    dev_label: Vec<String>,         // display name (side-tagged)
    dev_kind: Vec<char>,            // device kind (M/R/C/X/...) for terminal symmetry
    dev_terms: Vec<Vec<usize>>,     // net indices, ordered
    net_side: Vec<u8>,
    net_label: Vec<String>,
    net_init: Vec<String>,          // initial colour seed (port name or generic)
    net_incid: Vec<Vec<(usize, usize)>>, // (device idx, terminal position)
}

fn build(a: &Netlist, b: &Netlist) -> Graph {
    let mut g = Graph {
        dev_side: vec![],
        dev_label: vec![],
        dev_kind: vec![],
        dev_terms: vec![],
        net_side: vec![],
        net_label: vec![],
        net_init: vec![],
        net_incid: vec![],
    };
    for (side, nl) in [(0u8, a), (1u8, b)] {
        let ports: std::collections::BTreeSet<&str> = nl.ports.iter().map(|s| s.as_str()).collect();
        let mut net_id: BTreeMap<String, usize> = BTreeMap::new();
        let mut net = |g: &mut Graph, name: &str| -> usize {
            if let Some(&i) = net_id.get(name) {
                return i;
            }
            let i = g.net_side.len();
            g.net_side.push(side);
            g.net_label.push(format!("{}/{}", if side == 0 { "A" } else { "B" }, name));
            // ports anchored by name (boundary aligns); internals generic
            g.net_init.push(if ports.contains(name) { format!("P:{name}") } else { "n".into() });
            g.net_incid.push(vec![]);
            net_id.insert(name.to_string(), i);
            i
        };
        for d in &nl.devices {
            let did = g.dev_side.len();
            let terms: Vec<usize> = d.nodes.iter().map(|n| net(&mut g, n)).collect();
            for (pos, &nid) in terms.iter().enumerate() {
                g.net_incid[nid].push((did, pos));
            }
            g.dev_side.push(side);
            g.dev_label.push(format!("{}/{}", if side == 0 { "A" } else { "B" }, d.name));
            g.dev_kind.push(d.kind);
            g.dev_terms.push(terms);
        }
    }
    g
}

fn intern(reg: &mut BTreeMap<String, u64>, key: String) -> u64 {
    let next = reg.len() as u64;
    *reg.entry(key).or_insert(next)
}

/// Compare two netlists. `a` is typically the layout-extracted netlist, `b` the
/// schematic/reference.
pub fn compare(a: &Netlist, b: &Netlist) -> LvsResult {
    let mut r = LvsResult {
        a_devices: a.devices.len(),
        b_devices: b.devices.len(),
        ..Default::default()
    };

    // port set diff (boundary)
    let pa: std::collections::BTreeSet<_> = a.ports.iter().cloned().collect();
    let pb: std::collections::BTreeSet<_> = b.ports.iter().cloned().collect();
    r.only_in_a_ports = pa.difference(&pb).cloned().collect();
    r.only_in_b_ports = pb.difference(&pa).cloned().collect();

    // device-count-by-kind diff
    let kinds = |nl: &Netlist| {
        let mut m: BTreeMap<char, usize> = BTreeMap::new();
        for d in &nl.devices {
            *m.entry(d.kind).or_default() += 1;
        }
        m
    };
    let (ka, kb) = (kinds(a), kinds(b));
    let allk: std::collections::BTreeSet<char> = ka.keys().chain(kb.keys()).copied().collect();
    for k in allk {
        let (ca, cb) = (ka.get(&k).copied().unwrap_or(0), kb.get(&k).copied().unwrap_or(0));
        if ca != cb {
            r.device_kind_diff.push((k, ca, cb));
        }
    }

    // build the combined graph + device seeds (seeds gathered here, in push order)
    let g = build(a, b);
    let mut dev_seed: Vec<String> = Vec::new();
    for nl in [a, b] {
        for d in &nl.devices {
            dev_seed.push(format!("D:{}{}", d.kind, d.model));
        }
    }
    r.a_nets = g.net_side.iter().filter(|&&s| s == 0).count();
    r.b_nets = g.net_side.iter().filter(|&&s| s == 1).count();

    // initial colours
    let mut reg: BTreeMap<String, u64> = BTreeMap::new();
    let mut dev_c: Vec<u64> = dev_seed.iter().map(|s| intern(&mut reg, format!("d{s}"))).collect();
    let mut net_c: Vec<u64> = g.net_init.iter().map(|s| intern(&mut reg, format!("x{s}"))).collect();
    let mut prev = reg.len();

    // refine to a fixed point
    let mut iters = 0;
    for _ in 0..64 {
        iters += 1;
        let mut reg: BTreeMap<String, u64> = BTreeMap::new();
        let mut nd = vec![0u64; dev_c.len()];
        let mut nn = vec![0u64; net_c.len()];
        for (i, terms) in g.dev_terms.iter().enumerate() {
            // MOSFET (kind 'M', terminals [d, g, s, b]): source/drain are
            // interchangeable (match {d,s} unordered); gate and **bulk** are positional.
            let cols: Vec<u64> = if g.dev_kind[i] == 'M' && terms.len() >= 4 {
                let (d, gp, sc, bk) = (net_c[terms[0]], net_c[terms[1]], net_c[terms[2]], net_c[terms[3]]);
                vec![d.min(sc), gp, d.max(sc), bk]
            } else {
                terms.iter().map(|&nid| net_c[nid]).collect()
            };
            let mut s = format!("D{}", dev_c[i]);
            for c in cols {
                s.push('|');
                s.push_str(&c.to_string());
            }
            nd[i] = intern(&mut reg, s);
        }
        for (j, incid) in g.net_incid.iter().enumerate() {
            let mut parts: Vec<String> = incid
                .iter()
                .map(|&(d, pos)| {
                    // a MOSFET source/drain pin (pos 0 or 2) is indistinguishable; gate
                    // (1) and bulk (3) stay positional.
                    let p = if g.dev_kind[d] == 'M' && (pos == 0 || pos == 2) { 99 } else { pos };
                    format!("{}:{}", dev_c[d], p)
                })
                .collect();
            parts.sort();
            nn[j] = intern(&mut reg, format!("N{}|{}", net_c[j], parts.join(",")));
        }
        dev_c = nd;
        net_c = nn;
        if reg.len() == prev {
            break;
        }
        prev = reg.len();
    }
    r.iterations = iters;

    // tally each colour class A vs B (devices and nets separately)
    r.unbalanced = unbalanced("device", &dev_c, &g.dev_side, &g.dev_label);
    r.unbalanced.extend(unbalanced("net", &net_c, &g.net_side, &g.net_label));

    r.matched = r.only_in_a_ports.is_empty()
        && r.only_in_b_ports.is_empty()
        && r.a_devices == r.b_devices
        && r.a_nets == r.b_nets
        && r.unbalanced.is_empty();
    r
}

fn unbalanced(what: &'static str, colour: &[u64], side: &[u8], label: &[String]) -> Vec<Unbalanced> {
    let mut by: BTreeMap<u64, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for i in 0..colour.len() {
        let e = by.entry(colour[i]).or_default();
        if side[i] == 0 {
            e.0.push(label[i].clone());
        } else {
            e.1.push(label[i].clone());
        }
    }
    let mut out = Vec::new();
    for (_, (a, b)) in by {
        if a.len() != b.len() {
            out.push(Unbalanced {
                what,
                a_count: a.len(),
                b_count: b.len(),
                a_examples: a.into_iter().take(4).collect(),
                b_examples: b.into_iter().take(4).collect(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nl(t: &str) -> Netlist {
        Netlist::parse(t, None).unwrap()
    }

    const INV_A: &str = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD pfet\nMn Y A VSS VSS nfet\n.ends\n";
    // same circuit, internal nets renamed + device order swapped
    const INV_B: &str = ".subckt inv A Y VDD VSS\nM1 Y A VSS VSS nfet\nM0 Y A VDD VDD pfet\n.ends\n";

    #[test]
    fn identical_circuits_match() {
        let r = compare(&nl(INV_A), &nl(INV_B));
        assert!(r.matched, "renamed/reordered same circuit should MATCH: {r:?}");
        assert_eq!(r.a_devices, 2);
    }

    #[test]
    fn swapped_device_kind_mismatches() {
        // B uses two nfets (a real LVS error): should NOT match
        let bad = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD nfet\nMn Y A VSS VSS nfet\n.ends\n";
        let r = compare(&nl(INV_A), &nl(bad));
        assert!(!r.matched, "pfet->nfet swap must mismatch");
        assert!(!r.unbalanced.is_empty());
    }

    #[test]
    fn missing_device_mismatches() {
        let short = ".subckt inv A Y VDD VSS\nMn Y A VSS VSS nfet\n.ends\n";
        let r = compare(&nl(INV_A), &nl(short));
        assert!(!r.matched);
        assert_eq!((r.a_devices, r.b_devices), (2, 1));
    }
}
