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
//! ports align; internal nets are matched purely by structure. Supply/global nets
//! are held at a fixed colour so a local fault can't cascade across the power rail
//! (see [`is_supply_name`]). 1-WL alone is *necessary but not sufficient* — it can't
//! separate certain symmetric graphs — so a balanced refinement is then confirmed by
//! constructing an explicit device/net bijection ([`verify_iso`]), which both proves
//! a true MATCH and refutes a colour-refinement false positive. MOSFET source/drain
//! symmetry is handled in both the refinement and the bijection search.

use std::collections::BTreeMap;

use crate::spice::{Device, Netlist};

#[derive(Debug, Clone)]
pub struct Unbalanced {
    pub what: &'static str, // "device" | "net"
    pub a_count: usize,
    pub b_count: usize,
    pub a_examples: Vec<String>,
    pub b_examples: Vec<String>,
}

/// A device that matches topologically but carries an out-of-tolerance parameter
/// (a MOSFET drawn at the wrong width, a resistor of the wrong value, …).
#[derive(Debug, Clone)]
pub struct PropDiff {
    pub kind: char,
    pub a_device: String,
    pub b_device: String,
    pub param: String,
    pub a_value: f64,
    pub b_value: f64,
}

/// Relative tolerance for device-parameter equality (1%). Drawn dimensions /
/// values within this band are treated as the same; beyond it is an LVS error.
const PROP_TOL: f64 = 0.01;

/// The parameters worth checking per device kind: MOSFET geometry, passive value.
fn sig_keys(kind: char) -> &'static [&'static str] {
    match kind {
        'M' => &["w", "l", "nf", "m"],
        'R' | 'C' | 'L' => &["value"],
        _ => &[],
    }
}

/// Two parameter values agree within relative tolerance (both ~0 agree).
fn within_tol(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs());
    scale < 1e-18 || (a - b).abs() <= tol * scale
}

/// Whether two devices' significant parameters all agree within tolerance. Only
/// keys present on **both** sides are compared, so a netlist that omits a default
/// never forces a false mismatch.
fn props_compatible(
    kind: char,
    a: &std::collections::BTreeMap<String, f64>,
    b: &std::collections::BTreeMap<String, f64>,
    tol: f64,
) -> bool {
    sig_keys(kind).iter().all(|k| match (a.get(*k), b.get(*k)) {
        (Some(&av), Some(&bv)) => within_tol(av, bv, tol),
        _ => true,
    })
}

#[derive(Debug, Clone, Default)]
pub struct LvsResult {
    pub matched: bool,
    /// `matched` rests on an explicitly **constructed device/net bijection**, not
    /// just balanced colour counts — so the MATCH is sufficient, not merely the
    /// 1-WL necessary condition. `false` on a MISMATCH, or on a MATCH whose
    /// symmetry the bounded search couldn't resolve (see `note`).
    pub verified: bool,
    pub a_devices: usize,
    pub b_devices: usize,
    pub a_nets: usize,
    pub b_nets: usize,
    pub only_in_a_ports: Vec<String>,
    pub only_in_b_ports: Vec<String>,
    pub device_kind_diff: Vec<(char, usize, usize)>, // (kind, a, b) where they differ
    pub unbalanced: Vec<Unbalanced>,
    /// Devices that match topologically but whose parameters (W/L, value) differ
    /// beyond tolerance — a real LVS error a pure connectivity check misses.
    pub property_diffs: Vec<PropDiff>,
    pub iterations: usize,
    /// Human-readable qualifier on the verdict (refuted false-MATCH, parameter
    /// mismatch, or unresolved symmetry). `None` for a clean verified MATCH or a
    /// normal count/colour MISMATCH.
    pub note: Option<String>,
}

/// Outcome of the explicit-isomorphism construction.
enum Verify {
    /// A consistent bijection exists — the MATCH is proven.
    Verified,
    /// Colour counts balance but no consistent bijection exists — 1-WL false MATCH.
    Refuted,
    /// Search budget exhausted on highly symmetric structure — necessary
    /// condition holds, sufficiency unconfirmed.
    Unresolved,
}

struct Graph {
    // devices and nets, each tagged with side 0 (A) / 1 (B)
    dev_side: Vec<u8>,
    dev_label: Vec<String>,         // display name (side-tagged)
    dev_kind: Vec<char>,            // device kind (M/R/C/X/...) for terminal symmetry
    dev_terms: Vec<Vec<usize>>,     // net indices, ordered
    net_side: Vec<u8>,
    net_label: Vec<String>,
    net_name: Vec<String>,          // raw net name (no side prefix), for supply detection
    net_init: Vec<String>,          // initial colour seed (port name / supply / generic)
    net_incid: Vec<Vec<(usize, usize)>>, // (device idx, terminal position)
}

/// Power/ground nets are connected to a huge fraction of the devices, so under
/// plain 1-WL a *single* changed device anywhere shifts the supply net's colour
/// and that change then cascades to every device on the supply — turning one
/// real fault into tens of thousands of spurious "divergence classes" (the exact
/// failure netgen sidesteps by special-casing supplies). We detect supplies two
/// ways — by conventional name, and by degree — and hold their colour fixed so a
/// fault stays local to the gate that owns it.
fn is_supply_name(name: &str) -> bool {
    // strip a leading bus/hier path; compare the leaf, case-insensitively
    let leaf = name.rsplit(['/', '.', ':']).next().unwrap_or(name);
    const SUPPLY: &[&str] = &[
        "0", "vdd", "vss", "gnd", "vcc", "vee", "vpwr", "vgnd", "vnb", "vpb", "vbn", "vbp",
        "vdda", "vssa", "vccd", "vssd", "avdd", "avss", "dvdd", "dvss", "vbb", "vpp", "vsub",
    ];
    let lc = leaf.to_ascii_lowercase();
    SUPPLY.contains(&lc.as_str())
}

fn build(a: &Netlist, b: &Netlist) -> Graph {
    let mut g = Graph {
        dev_side: vec![],
        dev_label: vec![],
        dev_kind: vec![],
        dev_terms: vec![],
        net_side: vec![],
        net_label: vec![],
        net_name: vec![],
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
            g.net_name.push(name.to_string());
            // supplies anchored by their canonical name (so VDD ≠ VSS, side-independent);
            // ports anchored by name (boundary aligns); internals generic
            g.net_init.push(if is_supply_name(name) {
                format!("S:{}", name.to_ascii_uppercase())
            } else if ports.contains(name) {
                format!("P:{name}")
            } else {
                "n".into()
            });
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
    // Merge electrically-parallel devices on both sides first, so a transistor laid
    // out as N fingers matches a single wide schematic device (and vice versa).
    let (ca, cb) = (combine_parallel(a), combine_parallel(b));
    let (a, b) = (&ca, &cb);

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

    // Anchor supplies: a net is held at a fixed colour (excluded from refinement)
    // if it is a named supply OR its degree is large enough to be a global rail —
    // touched by far more terminals than any signal net. The degree gate scales
    // with the design so small circuits rely on the name list alone.
    let ndev = g.dev_side.len().max(1);
    let deg_gate = (ndev / 20).max(32);
    let supply: Vec<bool> = (0..g.net_side.len())
        .map(|j| is_supply_name(&g.net_name[j]) || g.net_incid[j].len() >= deg_gate)
        .collect();

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
            if supply[j] {
                // held fixed — a fault elsewhere on the rail must not cascade here
                nn[j] = net_c[j];
                continue;
            }
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
    // Smallest classes first: a localized fault lands in a tiny (often singleton)
    // class, so the actual offending device/net is named at the top of the report
    // rather than buried under large balanced-but-shifted populations.
    r.unbalanced.sort_by_key(|u| u.a_count + u.b_count);

    r.matched = r.only_in_a_ports.is_empty()
        && r.only_in_b_ports.is_empty()
        && r.a_devices == r.b_devices
        && r.a_nets == r.b_nets
        && r.unbalanced.is_empty();

    // 1-WL balance is *necessary* but not *sufficient* (symmetric graphs can fool
    // it). When the counts balance, construct an explicit device/net bijection that
    // preserves every connection — and that also matches device parameters within
    // tolerance — to prove the MATCH, refute a false positive, or surface a
    // parameter error.
    if r.matched {
        // global device order matches `build`: all of A's devices, then all of B's.
        let a_n = a.devices.len();
        let params: Vec<&std::collections::BTreeMap<String, f64>> =
            a.devices.iter().chain(b.devices.iter()).map(|d| &d.params).collect();
        let prop_ok = |ai: usize, bi: usize| {
            props_compatible(g.dev_kind[ai], params[ai], params[bi], PROP_TOL)
        };

        match verify_iso(&g, &dev_c, &net_c, &prop_ok).0 {
            Verify::Verified => r.verified = true,
            Verify::Unresolved => {
                r.note = Some(
                    "MATCH by colour refinement; explicit isomorphism not constructed \
                     within budget (highly symmetric) — necessary, not confirmed sufficient"
                        .into(),
                );
            }
            Verify::Refuted => {
                // No parameter-respecting bijection. Retry on topology alone: if that
                // succeeds, the topology is fine and the failure is a parameter error
                // — audit the mapping to name the offending device/param.
                let (v2, map) = verify_iso(&g, &dev_c, &net_c, &|_, _| true);
                match v2 {
                    Verify::Verified => {
                        r.property_diffs = audit_props(&map, a_n, a, b, PROP_TOL);
                        if r.property_diffs.is_empty() {
                            r.verified = true; // defensive: nothing actually differs
                        } else {
                            r.matched = false;
                            r.note = Some(
                                "device topology matches but device parameters differ \
                                 beyond tolerance (W/L or value)"
                                    .into(),
                            );
                        }
                    }
                    Verify::Refuted => {
                        r.matched = false;
                        r.note = Some(
                            "colour counts balance but no consistent device/net bijection \
                             exists — refuted 1-WL false MATCH"
                                .into(),
                        );
                    }
                    Verify::Unresolved => {
                        r.note = Some(
                            "MATCH by colour refinement; explicit isomorphism not constructed \
                             within budget (highly symmetric) — necessary, not confirmed sufficient"
                                .into(),
                        );
                    }
                }
            }
        }
    }
    r
}

/// Compare device parameters across the constructed bijection, collecting the
/// out-of-tolerance ones. `map` is `(a_global, b_global)` device indices; global
/// order is all of A's devices then all of B's (so `b_global - a_n` indexes B).
fn audit_props(
    map: &[(usize, usize)],
    a_n: usize,
    a: &Netlist,
    b: &Netlist,
    tol: f64,
) -> Vec<PropDiff> {
    let mut diffs = Vec::new();
    for &(ag, bg) in map {
        let (da, db) = (&a.devices[ag], &b.devices[bg - a_n]);
        for key in sig_keys(da.kind) {
            if let (Some(&av), Some(&bv)) = (da.params.get(*key), db.params.get(*key)) {
                if !within_tol(av, bv, tol) {
                    diffs.push(PropDiff {
                        kind: da.kind,
                        a_device: da.name.clone(),
                        b_device: db.name.clone(),
                        param: (*key).into(),
                        a_value: av,
                        b_value: bv,
                    });
                }
            }
        }
    }
    diffs
}

/// Recursion-free budget for the bijection search. Asymmetric circuits resolve in
/// one forward pass (each device has a single candidate); the budget only bites on
/// pathologically symmetric structure, where we report `Unresolved` rather than
/// guess.
const VERIFY_BUDGET: u64 = 5_000_000;

/// Terminal-correspondence orientations to try for a device kind: a MOSFET's
/// source/drain (positions 0 and 2) are interchangeable, everything else is fixed.
fn orient_count(kind: char, terms: usize) -> usize {
    if kind == 'M' && terms >= 4 {
        2
    } else {
        1
    }
}

/// The (a_pos, b_pos) terminal pairs for matching device `da`→`db` under
/// orientation `o`. Orientation 1 swaps the MOSFET source/drain.
fn term_pairs(kind: char, n: usize, o: usize) -> Vec<(usize, usize)> {
    if kind == 'M' && n >= 4 {
        let mut p = if o == 0 {
            vec![(0, 0), (1, 1), (2, 2), (3, 3)]
        } else {
            vec![(0, 2), (1, 1), (2, 0), (3, 3)]
        };
        p.extend((4..n).map(|i| (i, i))); // any extra positional terminals
        p
    } else {
        (0..n).map(|i| (i, i)).collect()
    }
}

/// Try to bind every terminal of `da`→`db` under orientation `o`; record each new
/// net assignment in `undo`. Returns false (leaving `undo` for the caller to roll
/// back) on the first inconsistency.
#[allow(clippy::too_many_arguments)]
fn assign_device(
    da: usize,
    db: usize,
    o: usize,
    g: &Graph,
    net_c: &[u64],
    net_map: &mut [i64],
    rev_net: &mut [i64],
    undo: &mut Vec<usize>,
) -> bool {
    let (ta, tb) = (&g.dev_terms[da], &g.dev_terms[db]);
    for (ap, bp) in term_pairs(g.dev_kind[da], ta.len(), o) {
        let (a, b) = (ta[ap], tb[bp]);
        if net_map[a] != -1 {
            if net_map[a] != b as i64 {
                return false;
            }
        } else if rev_net[b] != -1 {
            return false; // b already claimed by a different a-net
        } else if net_c[a] != net_c[b] {
            return false; // refined colours must agree
        } else {
            net_map[a] = b as i64;
            rev_net[b] = a as i64;
            undo.push(a);
        }
    }
    true
}

/// Construct an explicit isomorphism between the A and B sides by backtracking
/// over colour classes (an iterative, heap-stacked VF2 — so a 400k-device netlist
/// can't overflow the call stack). Singleton classes force a unique match and the
/// search runs forward; only genuine symmetry causes branching.
///
/// `compatible(a_dev, b_dev)` gates which device pairings are allowed (used to
/// require matching device parameters); pass `|_,_| true` for topology only.
/// Returns the verdict and, on `Verified`, the `(a_dev, b_dev)` global-index
/// bijection it built.
fn verify_iso(
    g: &Graph,
    dev_c: &[u64],
    net_c: &[u64],
    compatible: &dyn Fn(usize, usize) -> bool,
) -> (Verify, Vec<(usize, usize)>) {
    let (ndev, nnet) = (dev_c.len(), net_c.len());

    // candidate B devices grouped by refined colour
    let mut b_by_colour: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    let mut a_devs: Vec<usize> = Vec::new();
    for i in 0..ndev {
        if g.dev_side[i] == 0 {
            a_devs.push(i);
        } else {
            b_by_colour.entry(dev_c[i]).or_default().push(i);
        }
    }
    // match the most-constrained devices first (fewest candidates) to fail fast
    a_devs.sort_by_key(|&i| b_by_colour.get(&dev_c[i]).map_or(0, |v| v.len()));
    let m = a_devs.len();
    if m == 0 {
        return (Verify::Verified, Vec::new());
    }

    let mut net_map = vec![-1i64; nnet];
    let mut rev_net = vec![-1i64; nnet];
    let mut dev_used = vec![false; ndev];

    // Seed forced net anchors: any colour class that is a singleton on both sides
    // (supplies, ports, structurally-unique nets) has only one possible image.
    {
        let mut a_of: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        let mut b_of: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for j in 0..nnet {
            if g.net_side[j] == 0 {
                a_of.entry(net_c[j]).or_default().push(j);
            } else {
                b_of.entry(net_c[j]).or_default().push(j);
            }
        }
        for (col, av) in &a_of {
            if av.len() == 1 {
                if let Some(bv) = b_of.get(col) {
                    if bv.len() == 1 {
                        net_map[av[0]] = bv[0] as i64;
                        rev_net[bv[0]] = av[0] as i64;
                    }
                }
            }
        }
    }

    struct Frame {
        cands: Vec<usize>,
        ci: usize,
        oi: usize,
        undo: Vec<usize>,
        placed: i64,
    }
    let cand_list = |idx: usize| -> Vec<usize> {
        b_by_colour.get(&dev_c[a_devs[idx]]).cloned().unwrap_or_default()
    };

    let mut budget = VERIFY_BUDGET;
    let mut stack: Vec<Frame> = Vec::with_capacity(m);
    stack.push(Frame { cands: cand_list(0), ci: 0, oi: 0, undo: vec![], placed: -1 });

    loop {
        if budget == 0 {
            return (Verify::Unresolved, Vec::new());
        }
        let idx = stack.len() - 1;
        let da = a_devs[idx];
        let mut fr = stack.pop().unwrap();

        // roll back this frame's previous trial before trying the next option
        if fr.placed >= 0 {
            for &a in &fr.undo {
                rev_net[net_map[a] as usize] = -1;
                net_map[a] = -1;
            }
            fr.undo.clear();
            dev_used[fr.placed as usize] = false;
            fr.placed = -1;
        }

        // find the next viable (candidate device, orientation)
        let mut advanced = false;
        while fr.ci < fr.cands.len() {
            let db = fr.cands[fr.ci];
            let orients = orient_count(g.dev_kind[da], g.dev_terms[da].len());
            let same_arity = g.dev_terms[da].len() == g.dev_terms[db].len();
            while fr.oi < orients {
                let o = fr.oi;
                fr.oi += 1;
                budget -= 1;
                if budget == 0 {
                    return (Verify::Unresolved, Vec::new());
                }
                if dev_used[db] || !same_arity || !compatible(da, db) {
                    break; // skip an unusable / parameter-incompatible candidate
                }
                let mut undo = Vec::new();
                if assign_device(da, db, o, g, net_c, &mut net_map, &mut rev_net, &mut undo) {
                    dev_used[db] = true;
                    fr.placed = db as i64;
                    fr.undo = undo;
                    advanced = true;
                    break;
                }
                for &a in &undo {
                    rev_net[net_map[a] as usize] = -1;
                    net_map[a] = -1;
                }
            }
            if advanced {
                break;
            }
            fr.ci += 1;
            fr.oi = 0;
        }

        if advanced {
            let last = idx + 1 == m;
            stack.push(fr);
            if last {
                let map =
                    stack.iter().enumerate().map(|(k, f)| (a_devs[k], f.placed as usize)).collect();
                return (Verify::Verified, map);
            }
            stack.push(Frame { cands: cand_list(idx + 1), ci: 0, oi: 0, undo: vec![], placed: -1 });
        } else {
            // dead end — drop this frame and let the parent try its next option
            match stack.last_mut() {
                Some(p) => {
                    p.ci += 1;
                    p.oi = 0;
                }
                None => return (Verify::Refuted, Vec::new()),
            }
        }
    }
}

/// Canonical signature for parallel grouping: two devices with the same signature
/// are electrically parallel (identical kind, model, and terminal connectivity —
/// honouring MOSFET source/drain and passive two-terminal symmetry). MOSFET `l` is
/// folded in so only same-length fingers merge; capacitor/resistor/inductor are
/// symmetric, diodes/BJTs keep terminal order.
fn parallel_sig(d: &Device) -> (char, String, Vec<String>, i64) {
    let n = &d.nodes;
    let terms: Vec<String> = match d.kind {
        'M' if n.len() >= 4 => {
            let (mut s, mut dr) = (n[0].clone(), n[2].clone());
            if s > dr {
                std::mem::swap(&mut s, &mut dr);
            }
            vec![s, n[1].clone(), dr, n[3].clone()] // {s,d} unordered; gate, bulk fixed
        }
        'R' | 'C' | 'L' if n.len() == 2 => {
            let mut t = vec![n[0].clone(), n[1].clone()];
            t.sort(); // symmetric two-terminal
            t
        }
        _ => n.clone(), // diode/BJT/other: polarity / order preserved
    };
    // fold MOSFET length into the key (nm) so different-L devices never merge
    let lkey = (d.params.get("l").copied().unwrap_or(0.0) * 1e9).round() as i64;
    (d.kind, d.model.clone(), terms, lkey)
}

/// Combine the size parameter of `group` (all mutually parallel) into one device.
/// Width adds for MOSFETs; capacitance adds; resistance/inductance combine in
/// parallel. A missing value on any member drops that combined param (the audit
/// then simply skips it) rather than inventing one.
fn combine_size(kind: char, group: &[&Device]) -> BTreeMap<String, f64> {
    let mut p = group[0].params.clone();
    let all = |key: &str| group.iter().all(|d| d.params.contains_key(key));
    match kind {
        'M' if all("w") => {
            // effective width = Σ w·nf·m, collapsed to a single finger
            let w: f64 = group
                .iter()
                .map(|d| {
                    let g = |k: &str| d.params.get(k).copied().unwrap_or(1.0);
                    d.params["w"] * g("nf") * g("m")
                })
                .sum();
            p.insert("w".into(), w);
            p.remove("nf");
            p.remove("m");
        }
        'C' if all("value") => {
            p.insert("value".into(), group.iter().map(|d| d.params["value"]).sum());
        }
        'R' | 'L' if all("value") && group.iter().all(|d| d.params["value"] > 0.0) => {
            let g: f64 = group.iter().map(|d| 1.0 / d.params["value"]).sum();
            p.insert("value".into(), 1.0 / g);
        }
        _ => {}
    }
    p
}

/// Merge electrically-parallel devices into one each (a netlist normalization run
/// before comparison). Order is preserved by first occurrence; singletons pass
/// through unchanged. `X` instances are never merged.
fn combine_parallel(nl: &Netlist) -> Netlist {
    let mut order: Vec<(char, String, Vec<String>, i64)> = Vec::new();
    let mut groups: BTreeMap<(char, String, Vec<String>, i64), Vec<&Device>> = BTreeMap::new();
    for d in &nl.devices {
        // X subckt instances are never merged — key each by its (unique) name
        let key = if d.kind == 'X' {
            ('X', d.name.clone(), d.nodes.clone(), 0)
        } else {
            parallel_sig(d)
        };
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(d);
    }
    let mut devices: Vec<Device> = Vec::with_capacity(order.len());
    for key in &order {
        let group = &groups[key];
        let first = group[0];
        if group.len() == 1 {
            devices.push(first.clone());
        } else {
            devices.push(Device {
                kind: first.kind,
                name: format!("{}(×{})", first.name, group.len()),
                nodes: first.nodes.clone(),
                model: first.model.clone(),
                params: combine_size(first.kind, group),
            });
        }
    }
    Netlist { name: nl.name.clone(), ports: nl.ports.clone(), devices }
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
        assert!(r.verified, "MATCH should be confirmed by an explicit bijection: {r:?}");
        assert_eq!(r.a_devices, 2);
    }

    #[test]
    fn refutes_1wl_false_match() {
        // The textbook colour-refinement blind spot: a 6-ring vs two 3-rings are
        // both 2-regular, so 1-WL alone calls them equivalent. They are NOT
        // isomorphic — the explicit-bijection pass must refute the false MATCH.
        let ring6 = "R0 n0 n1\nR1 n1 n2\nR2 n2 n3\nR3 n3 n4\nR4 n4 n5\nR5 n5 n0\n";
        let two3 = "R0 m0 m1\nR1 m1 m2\nR2 m2 m0\nR3 m3 m4\nR4 m4 m5\nR5 m5 m3\n";
        let r = compare(&nl(ring6), &nl(two3));
        assert!(!r.matched, "6-ring vs two 3-rings must not MATCH");
        assert!(
            r.note.as_deref().unwrap_or("").contains("refuted"),
            "should report a refuted 1-WL false MATCH, got {:?}",
            r.note
        );
    }

    #[test]
    fn isomorphic_rings_verify() {
        // a genuine match of the same symmetric structure still verifies
        let a = "R0 n0 n1\nR1 n1 n2\nR2 n2 n0\n";
        let b = "R0 m0 m1\nR1 m1 m2\nR2 m2 m0\n";
        let r = compare(&nl(a), &nl(b));
        assert!(r.matched && r.verified, "isomorphic triangles should verify: {r:?}");
    }

    #[test]
    fn swapped_device_kind_mismatches() {
        // B uses two nfets (a real LVS error): should NOT match
        let bad = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD nfet\nMn Y A VSS VSS nfet\n.ends\n";
        let r = compare(&nl(INV_A), &nl(bad));
        assert!(!r.matched, "pfet->nfet swap must mismatch");
        assert!(!r.unbalanced.is_empty());
    }

    // Build N inverters all sharing VDD/VSS; `mutate` optionally rewires one gate.
    fn bank(n: usize, mutate: bool) -> Netlist {
        let mut s = String::from(".subckt bank VDD VSS\n");
        for i in 0..n {
            s += &format!("Mp{i} y{i} a{i} VDD VDD pfet\n");
            let g = if mutate && i == 0 { 1 } else { i }; // plant ONE faulted gate net
            s += &format!("Mn{i} y{i} a{g} VSS VSS nfet\n");
        }
        s += ".ends\n";
        nl(&s)
    }

    #[test]
    fn single_fault_stays_local_not_cascaded() {
        // The regression Rob measured: one planted net fault on a supply-heavy
        // netlist produced 255,103 "divergence classes" and never named the gate.
        let good = bank(200, false);
        let bad = bank(200, true);
        let r = compare(&good, &bad);
        assert!(!r.matched, "a rewired gate must MISMATCH");
        // With supplies anchored the fault no longer cascades across the rail:
        // only the handful of nets/devices around the change diverge.
        assert!(
            r.unbalanced.len() <= 12,
            "fault should stay local, got {} divergence classes",
            r.unbalanced.len()
        );
        // …and the offending neighbourhood (nets a0/a1/y0, gates Mp0/Mn0/Mp1) is
        // named at the top of the report — not lost in a sea of cascaded classes.
        let named: String = r
            .unbalanced
            .iter()
            .flat_map(|u| u.a_examples.iter().chain(&u.b_examples))
            .cloned()
            .collect();
        assert!(
            ["a0", "a1", "y0", "Mp0", "Mn0", "Mp1"].iter().any(|t| named.contains(t)),
            "the faulted neighbourhood should be named, got {named:?}"
        );
    }

    #[test]
    fn supply_heavy_equivalent_still_matches() {
        // anchoring must not create false mismatches on a clean design
        let r = compare(&bank(200, false), &bank(200, false));
        assert!(r.matched, "identical supply-heavy banks must MATCH: {} classes", r.unbalanced.len());
    }

    #[test]
    fn fingered_mosfet_matches_single_wide() {
        // layout draws the device as 4 parallel W=0.5 fingers; schematic is one
        // W=2 device — combining the fingers makes them match (and the widths agree).
        let lay = ".subckt c A Y VDD VSS\n\
            Mp0 Y A VDD VDD pfet w=0.5u l=0.15u\nMp1 Y A VDD VDD pfet w=0.5u l=0.15u\n\
            Mp2 Y A VDD VDD pfet w=0.5u l=0.15u\nMp3 Y A VDD VDD pfet w=0.5u l=0.15u\n\
            Mn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let sch = ".subckt c A Y VDD VSS\nMp Y A VDD VDD pfet w=2u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let r = compare(&nl(lay), &nl(sch));
        assert!(r.matched && r.verified, "4 fingers should combine and MATCH a W=2 device: {r:?}");
        assert_eq!((r.a_devices, r.b_devices), (2, 2), "fingers combined to 2 devices");
    }

    #[test]
    fn fingered_width_total_mismatch_is_caught() {
        // only 3 fingers in the layout -> combined W=1.5 vs schematic W=2 -> error
        let lay = ".subckt c A Y VDD VSS\n\
            Mp0 Y A VDD VDD pfet w=0.5u l=0.15u\nMp1 Y A VDD VDD pfet w=0.5u l=0.15u\n\
            Mp2 Y A VDD VDD pfet w=0.5u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let sch = ".subckt c A Y VDD VSS\nMp Y A VDD VDD pfet w=2u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let r = compare(&nl(lay), &nl(sch));
        assert!(!r.matched, "short by one finger -> total width differs -> MISMATCH");
        assert!(r.property_diffs.iter().any(|d| d.param == "w"));
    }

    #[test]
    fn parallel_resistors_combine() {
        // two parallel 2k resistors == one 1k
        let lay = "R0 a b 2k\nR1 a b 2k\n";
        let sch = "R0 a b 1k\n";
        let r = compare(&nl(lay), &nl(sch));
        assert!(r.matched, "2k||2k should match 1k: {r:?}");
    }

    #[test]
    fn mosfet_width_mismatch_is_caught() {
        // identical topology, but the layout draws the pull-up at half width — a
        // real LVS error a connectivity-only check passes.
        let sch = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD pfet w=2u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let lay = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD pfet w=1u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let r = compare(&nl(lay), &nl(sch));
        assert!(!r.matched, "a wrong transistor width must MISMATCH");
        assert_eq!(r.property_diffs.len(), 1, "exactly the pull-up width differs: {:?}", r.property_diffs);
        let d = &r.property_diffs[0];
        assert_eq!((d.kind, d.param.as_str()), ('M', "w"));
        assert!((d.a_value - 1e-6).abs() < 1e-15 && (d.b_value - 2e-6).abs() < 1e-15);
    }

    #[test]
    fn matching_widths_within_tolerance_pass() {
        // 0.5% apart on a 1% tolerance -> the same device.
        let sch = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD pfet w=2.00u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let lay = ".subckt inv A Y VDD VSS\nMp Y A VDD VDD pfet w=2.01u l=0.15u\nMn Y A VSS VSS nfet w=1u l=0.15u\n.ends\n";
        let r = compare(&nl(lay), &nl(sch));
        assert!(r.matched && r.verified, "within-tolerance widths should verify: {r:?}");
        assert!(r.property_diffs.is_empty());
    }

    #[test]
    fn resistor_value_mismatch_is_caught() {
        let sch = "R1 a b 1k\nR2 b c 2k\n";
        let lay = "R1 a b 1k\nR2 b c 5k\n"; // R2 wrong value
        let r = compare(&nl(lay), &nl(sch));
        assert!(!r.matched, "wrong resistor value must MISMATCH");
        assert!(r.property_diffs.iter().any(|d| d.param == "value" && (d.a_value - 5e3).abs() < 1.0));
    }

    #[test]
    fn missing_device_mismatches() {
        let short = ".subckt inv A Y VDD VSS\nMn Y A VSS VSS nfet\n.ends\n";
        let r = compare(&nl(INV_A), &nl(short));
        assert!(!r.matched);
        assert_eq!((r.a_devices, r.b_devices), (2, 1));
    }
}
