//! Native device extraction — a GDS layout + extraction rules → a SPICE `Netlist`,
//! using the vendored geometry kernel (`crate::layout`). This is vyges-lvs Phase 2:
//! LVS straight from layout, no external extractor.
//!
//! Recipe: **devices** are gate∩active (`poly AND active`); **type** is pfet if the
//! channel sits in nwell, else nfet; **source/drain** are the two diffusion regions of
//! `active − poly` adjacent to the channel; **gate** is the poly net over the channel;
//! **bulk** is the nwell net (pfet) or the configured substrate net (nfet).
//!
//! **Connectivity is contact-gated**: shapes on different layers join into one net only
//! where a `contact:` shape overlaps both — a gate (poly) abutting source/drain is *not*
//! joined to it. Same-layer shapes that touch are one net. **Net names** come from TEXT
//! labels; the labelled nets are the cell ports.
//!
//! Honest bounds (depth): contacts/vias are matched by overlap (no enclosure DRC);
//! arrays/hierarchy should be flattened first; source/drain emitted in arbitrary order
//! (the comparator is S/D-symmetric); the boolean is Manhattan.

use std::collections::{BTreeMap, HashMap};

use crate::layout::boolean::{boolean_poly, Op};
use crate::layout::gds::{Cell, Element, Library};
use crate::layout::geom::{self, Rect};
use crate::spice::{Device, Netlist};

type Ld = (i16, i16);

#[derive(Debug, Clone)]
pub struct Rules {
    pub active: Ld,
    pub poly: Ld,
    pub nwell: Ld,
    pub conn: Vec<Ld>,
    pub contacts: Vec<(Ld, Ld, Ld)>, // (contact layer, layer A, layer B)
    pub label: Vec<Ld>,              // TEXT layer(s) carrying net/pin names
    pub substrate: String,           // nfet bulk net (global; also a port)
    pub nfet: String,
    pub pfet: String,
    // model variants: a channel touching this marker layer uses this model instead of
    // the base (e.g. hvtp 78/44 → *_hvt). First match wins.
    pub nfet_variants: Vec<(Ld, String)>,
    pub pfet_variants: Vec<(Ld, String)>,
}

fn parse_ld(s: &str) -> Result<Ld, String> {
    let (a, b) = s.trim().split_once('/').unwrap_or((s.trim(), "0"));
    Ok((
        a.trim().parse().map_err(|_| format!("bad layer/datatype {s:?}"))?,
        b.trim().parse().map_err(|_| format!("bad layer/datatype {s:?}"))?,
    ))
}

impl Rules {
    pub fn parse(text: &str) -> Result<Rules, String> {
        let mut kv: BTreeMap<String, String> = BTreeMap::new();
        let mut contacts = Vec::new();
        let mut nfet_variants = Vec::new();
        let mut pfet_variants = Vec::new();
        for line in text.lines() {
            let l = line.split('#').next().unwrap_or("").trim();
            if let Some((k, v)) = l.split_once(':') {
                let key = k.trim().to_lowercase();
                let t: Vec<&str> = v.split_whitespace().collect();
                match key.as_str() {
                    "contact" if t.len() == 3 => {
                        contacts.push((parse_ld(t[0])?, parse_ld(t[1])?, parse_ld(t[2])?));
                    }
                    "nfet_variant" if t.len() == 2 => nfet_variants.push((parse_ld(t[0])?, t[1].to_string())),
                    "pfet_variant" if t.len() == 2 => pfet_variants.push((parse_ld(t[0])?, t[1].to_string())),
                    _ => {
                        kv.insert(key, v.trim().to_string());
                    }
                }
            }
        }
        let get = |k: &str| kv.get(k).cloned().ok_or_else(|| format!("rules missing key: {k}"));
        let conn = get("conn")?.split(',').filter_map(|s| parse_ld(s.trim()).ok()).collect();
        Ok(Rules {
            active: parse_ld(&get("active")?)?,
            poly: parse_ld(&get("poly")?)?,
            nwell: parse_ld(&get("nwell")?)?,
            conn,
            contacts,
            label: get("label")?.split(',').filter_map(|s| parse_ld(s.trim()).ok()).collect(),
            substrate: kv.get("substrate").cloned().unwrap_or_else(|| "VSUB".into()),
            nfet: kv.get("nfet").cloned().unwrap_or_else(|| "nfet".into()),
            pfet: kv.get("pfet").cloned().unwrap_or_else(|| "pfet".into()),
            nfet_variants,
            pfet_variants,
        })
    }
    pub fn load(path: &str) -> Result<Rules, String> {
        Rules::parse(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?)
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Role {
    Active,
    Poly,
    Well,
    Metal,
}

struct Prim {
    rects: Vec<Rect>, // the shape geometry (tiled) — for TRUE overlap, not a bbox
    role: Role,
    layer: Ld,
}

struct Uf {
    p: Vec<usize>,
}
impl Uf {
    fn new(n: usize) -> Uf {
        Uf { p: (0..n).collect() }
    }
    fn find(&mut self, x: usize) -> usize {
        if self.p[x] != x {
            let r = self.find(self.p[x]);
            self.p[x] = r;
        }
        self.p[x]
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.p[a] = b;
        }
    }
}

fn polys_on(cell: &Cell, ld: Ld) -> Vec<Vec<(i32, i32)>> {
    cell.elements
        .iter()
        .filter_map(|el| match el {
            Element::Boundary { layer, datatype, pts } if (*layer, *datatype) == ld => Some(pts.clone()),
            Element::Box { layer, boxtype, pts } if (*layer, *boxtype) == ld => Some(pts.clone()),
            _ => None,
        })
        .collect()
}
fn bbox_of(p: &[(i32, i32)]) -> Rect {
    geom::bbox(p).unwrap_or(Rect { x0: 0, y0: 0, x1: 0, y1: 0 })
}
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}
fn union_bbox(rects: &[Rect]) -> Rect {
    let mut r = rects[0];
    for x in &rects[1..] {
        r.x0 = r.x0.min(x.x0);
        r.y0 = r.y0.min(x.y0);
        r.x1 = r.x1.max(x.x1);
        r.y1 = r.y1.max(x.y1);
    }
    r
}

/// Channel width and length (in **metres**) from the channel bbox `cb` and the
/// diffusion regions `sd` flanking it (`(net, bbox)`). Current flows along the axis
/// that separates source from drain — that channel extent is the gate **length** L;
/// the perpendicular extent is the gate **width** W. `db_unit` is metres per GDS DB
/// unit. Returns `None` unless exactly two distinct diffusion nets flank the channel
/// (a MOSCAP's single tied diffusion has no defined W/L).
fn channel_wl(cb: &Rect, sd: &[(usize, Rect)], db_unit: f64) -> Option<(f64, f64)> {
    let mut by_net: BTreeMap<usize, Rect> = BTreeMap::new();
    for (net, r) in sd {
        by_net.entry(*net).and_modify(|u| *u = union_bbox(&[*u, *r])).or_insert(*r);
    }
    if by_net.len() != 2 {
        return None;
    }
    let c: Vec<(f64, f64)> = by_net
        .values()
        .map(|r| ((r.x0 + r.x1) as f64 / 2.0, (r.y0 + r.y1) as f64 / 2.0))
        .collect();
    let dx_sep = (c[0].0 - c[1].0).abs();
    let dy_sep = (c[0].1 - c[1].1).abs();
    let (l_dbu, w_dbu) = if dx_sep >= dy_sep {
        ((cb.x1 - cb.x0) as f64, (cb.y1 - cb.y0) as f64) // S/D along X -> L is the X extent
    } else {
        ((cb.y1 - cb.y0) as f64, (cb.x1 - cb.x0) as f64)
    };
    (l_dbu > 0.0 && w_dbu > 0.0).then_some((w_dbu * db_unit, l_dbu * db_unit))
}
/// Is `poly` a single axis-aligned rectangle (the overwhelmingly common shape)?
fn is_rect(poly: &[(i32, i32)]) -> bool {
    let p = if poly.len() >= 2 && poly.first() == poly.last() { &poly[..poly.len() - 1] } else { poly };
    if p.len() != 4 {
        return false;
    }
    let bb = bbox_of(poly);
    p.iter().all(|&(x, y)| (x == bb.x0 || x == bb.x1) && (y == bb.y0 || y == bb.y1))
}
/// Rect-tiling of a rectilinear polygon — preserves true geometry (vs a bbox). A plain
/// rectangle (contacts, vias, most metal) skips the scanline boolean entirely.
fn tile(poly: &[(i32, i32)]) -> Vec<Rect> {
    if is_rect(poly) {
        return vec![bbox_of(poly)];
    }
    boolean_poly(&[poly.to_vec()], &[], Op::Or)
}
/// True (inclusive) geometric touch between two rect sets, with a bbox quick-reject.
fn rects_touch(a: &[Rect], b: &[Rect]) -> bool {
    if a.is_empty() || b.is_empty() || !overlap(&union_bbox(a), &union_bbox(b)) {
        return false;
    }
    a.iter().any(|r| b.iter().any(|s| overlap(r, s)))
}
fn pt_in(rects: &[Rect], x: i32, y: i32) -> bool {
    rects.iter().any(|r| r.x0 <= x && x <= r.x1 && r.y0 <= y && y <= r.y1)
}
fn contains(o: &Rect, c: &Rect) -> bool {
    o.x0 <= c.x0 && c.x1 <= o.x1 && o.y0 <= c.y0 && c.y1 <= o.y1
}
/// Enclosure: is `inner` fully covered by `outer` (a DRC-clean contact sits inside its
/// metal)? `inner − outer` is empty. Used to gate cross-layer connectivity.
fn enclosed(inner: &[Rect], outer: &[Rect]) -> bool {
    if inner.is_empty() || outer.is_empty() || !overlap(&union_bbox(inner), &union_bbox(outer)) {
        return false;
    }
    // Fast path: every inner rect sits wholly inside one outer rect — true for a normal
    // single-cut contact in its metal, and avoids the scanline boolean on the hot path
    // (a routed block has thousands of contacts/vias). Fall back to the exact boolean
    // only for the rare case where a contact straddles several outer rects.
    if inner.iter().all(|c| outer.iter().any(|o| contains(o, c))) {
        return true;
    }
    let ip: Vec<_> = inner.iter().map(|r| r.as_boundary()).collect();
    let op: Vec<_> = outer.iter().map(|r| r.as_boundary()).collect();
    boolean_poly(&ip, &op, Op::Not).is_empty()
}
/// Uniform-grid spatial index over a set of axis-aligned boxes, so overlap queries are
/// ~O(1) instead of O(n). A box spanning many cells (power rails, wells) would pollute
/// thousands of buckets, so anything covering more than `BIG_CELLS` cells goes into a
/// small `big` list checked against every query — bounding both build and query cost.
struct Grid {
    cell: i64,
    minx: i64,
    miny: i64,
    buckets: HashMap<(i32, i32), Vec<usize>>,
    big: Vec<usize>,
}
const BIG_CELLS: i64 = 256;
impl Grid {
    fn build(boxes: &[Rect]) -> Grid {
        let n = boxes.len().max(1);
        let (mut minx, mut miny, mut maxx, mut maxy) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        let (mut wsum, mut hsum) = (0i64, 0i64);
        for r in boxes {
            minx = minx.min(r.x0 as i64);
            miny = miny.min(r.y0 as i64);
            maxx = maxx.max(r.x1 as i64);
            maxy = maxy.max(r.y1 as i64);
            wsum += (r.x1 - r.x0) as i64 + 1;
            hsum += (r.y1 - r.y0) as i64 + 1;
        }
        if boxes.is_empty() {
            (minx, miny, maxx, maxy) = (0, 0, 0, 0);
        }
        // cell size ≈ average box dimension, but never so fine that a normal box spans
        // a huge area — bounded below by span/512.
        let avg = ((wsum + hsum) / (2 * n as i64)).max(1);
        let span = (maxx - minx).max(maxy - miny).max(1);
        let cell = avg.max(span / 512).max(1);
        let mut g = Grid { cell, minx, miny, buckets: HashMap::new(), big: Vec::new() };
        for (i, r) in boxes.iter().enumerate() {
            let (cx0, cy0, cx1, cy1) = g.range(r);
            let ncells = (cx1 - cx0 + 1) as i64 * (cy1 - cy0 + 1) as i64;
            if ncells > BIG_CELLS {
                g.big.push(i);
            } else {
                for cx in cx0..=cx1 {
                    for cy in cy0..=cy1 {
                        g.buckets.entry((cx, cy)).or_default().push(i);
                    }
                }
            }
        }
        g
    }
    fn range(&self, r: &Rect) -> (i32, i32, i32, i32) {
        let c = self.cell;
        (
            ((r.x0 as i64 - self.minx) / c) as i32,
            ((r.y0 as i64 - self.miny) / c) as i32,
            ((r.x1 as i64 - self.minx) / c) as i32,
            ((r.y1 as i64 - self.miny) / c) as i32,
        )
    }
    /// Candidate box indices whose cells touch `r` (a superset of true overlaps; dedup'd).
    fn query(&self, r: &Rect, out: &mut Vec<usize>) {
        out.clear();
        let (cx0, cy0, cx1, cy1) = self.range(r);
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                if let Some(v) = self.buckets.get(&(cx, cy)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.extend_from_slice(&self.big);
        out.sort_unstable();
        out.dedup();
    }
}

fn components(rects: &[Rect]) -> Vec<Vec<Rect>> {
    let n = rects.len();
    let mut uf = Uf::new(n);
    // Two overlapping boxes always share at least one grid cell, so unioning only
    // within-bucket (+ against big boxes) finds every connected component — but in
    // ~O(n) instead of the O(n²) all-pairs scan.
    let g = Grid::build(rects);
    for idxs in g.buckets.values() {
        for a in 0..idxs.len() {
            for b in a + 1..idxs.len() {
                let (i, j) = (idxs[a], idxs[b]);
                if uf.find(i) != uf.find(j) && overlap(&rects[i], &rects[j]) {
                    uf.union(i, j);
                }
            }
        }
    }
    for &i in &g.big {
        for j in 0..n {
            if i != j && uf.find(i) != uf.find(j) && overlap(&rects[i], &rects[j]) {
                uf.union(i, j);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<Rect>> = HashMap::new();
    for i in 0..n {
        groups.entry(uf.find(i)).or_default().push(rects[i]);
    }
    groups.into_values().collect()
}

pub fn extract(lib: &Library, top: Option<&str>, rules: &Rules) -> Result<Netlist, String> {
    let base = match top {
        Some(t) => lib.cells.iter().find(|c| c.name == t).ok_or_else(|| format!("cell {t:?} not found"))?,
        None if lib.cells.len() == 1 => &lib.cells[0],
        None => return Err("pass --top to choose a cell".into()),
    };
    // flatten hierarchy (SREF/AREF cell instances + arrays) before extraction
    let has_refs = base.elements.iter().any(|e| matches!(e, Element::Sref { .. } | Element::Aref { .. }));
    let flat = if has_refs { Some(crate::layout::flatten::flatten(lib, &base.name)?) } else { None };
    let cell: &Cell = flat.as_ref().unwrap_or(base);

    let dbg = std::env::var("VLVS_DEBUG").is_ok();
    let t0 = std::time::Instant::now();
    let mark = |label: &str| {
        if dbg {
            eprintln!("  [t] {label}: {:.2}s", t0.elapsed().as_secs_f64());
        }
    };

    let active = polys_on(cell, rules.active);
    let poly = polys_on(cell, rules.poly);
    let nwell = polys_on(cell, rules.nwell);
    mark("read active/poly/nwell");

    // --- connectivity primitives (same-layer touching shapes already merged) ---
    let mut prims: Vec<Prim> = Vec::new();
    let mut conn = rules.conn.clone();
    if !conn.contains(&rules.nwell) {
        conn.push(rules.nwell); // wells participate in net tracing (taps -> a rail)
    }
    for &cl in &conn {
        if cl == rules.active {
            for comp in components(&boolean_poly(&active, &poly, Op::Not)) {
                prims.push(Prim { rects: comp, role: Role::Active, layer: cl });
            }
        } else {
            let role = if cl == rules.poly {
                Role::Poly
            } else if cl == rules.nwell {
                Role::Well
            } else {
                Role::Metal
            };
            // tile every shape on the layer, then group by TRUE rect overlap (not bbox)
            // — so L-shaped / abutting-bbox routing on one layer doesn't over-merge.
            let mut tiles = Vec::new();
            for s in polys_on(cell, cl) {
                tiles.extend(tile(&s));
            }
            for comp in components(&tiles) {
                prims.push(Prim { rects: comp, role, layer: cl });
            }
        }
    }
    mark("build prims");

    // --- nets: union prims only through contacts (and same-layer, already merged) ---
    let n = prims.len();
    let mut uf = Uf::new(n);
    // spatial index over prim bounding boxes — every prim-vs-geometry scan below (contacts,
    // gate/source/drain, bulk) queries this instead of walking all n prims (O(n²) → ~O(n)).
    let prim_bb: Vec<Rect> = prims.iter().map(|p| union_bbox(&p.rects)).collect();
    let pgrid = Grid::build(&prim_bb);
    let mut cand: Vec<usize> = Vec::new();
    // a contact/via joins two layers only where it is ENCLOSED by a shape on each
    // (DRC-clean overlap, not a bare edge-touch) — this also covers metal↔metal vias,
    // which are just more `contact:` rules.
    for (cl, la, lb) in &rules.contacts {
        for cp in polys_on(cell, *cl) {
            let ct = tile(&cp);
            pgrid.query(&union_bbox(&ct), &mut cand);
            let pa = cand.iter().copied().find(|&pi| prims[pi].layer == *la && enclosed(&ct, &prims[pi].rects));
            let pb = cand.iter().copied().find(|&pi| prims[pi].layer == *lb && enclosed(&ct, &prims[pi].rects));
            if let (Some(i), Some(j)) = (pa, pb) {
                uf.union(i, j);
            }
        }
    }
    let mut canon: BTreeMap<usize, usize> = BTreeMap::new();
    let net_id: Vec<usize> = (0..n)
        .map(|i| {
            let r = uf.find(i);
            let k = canon.len();
            *canon.entry(r).or_insert(k)
        })
        .collect();
    let nnets = canon.len();

    // names from TEXT labels -> labelled nets are ports
    let mut name: Vec<Option<String>> = vec![None; nnets];
    for el in &cell.elements {
        if let Element::Text { layer, texttype, x, y, string } = el {
            if rules.label.contains(&(*layer, *texttype)) {
                // prefer a prim on the label's own base layer (e.g. an nwell label →
                // the nwell net, not a diffusion that happens to be under it), else any.
                let pi = prims
                    .iter()
                    .position(|p| p.layer.0 == *layer && pt_in(&p.rects, *x, *y))
                    .or_else(|| prims.iter().position(|p| pt_in(&p.rects, *x, *y)));
                if let Some(pi) = pi {
                    name[net_id[pi]] = Some(string.clone());
                }
            }
        }
    }
    let net_name = |nid: usize| name[nid].clone().unwrap_or_else(|| format!("n{nid}"));

    mark("net union-find (contacts)");

    // --- devices: channels = poly ∩ active ---
    if dbg {
        let cnt = |r: Role| prims.iter().filter(|p| p.role == r).count();
        eprintln!(
            "prims {} (active {} poly {} well {} metal {}); nets {}",
            prims.len(),
            cnt(Role::Active),
            cnt(Role::Poly),
            cnt(Role::Well),
            cnt(Role::Metal),
            nnets
        );
    }
    let mut devices = Vec::new();
    let mut skipped = 0usize;
    let channels = components(&boolean_poly(&poly, &active, Op::And));
    mark("channel boolean (poly ∩ active)");
    for (i, ch) in channels.iter().enumerate() {
        let cb = union_bbox(ch);
        pgrid.query(&cb, &mut cand);
        let gate = cand
            .iter()
            .copied()
            .find(|&pi| prims[pi].role == Role::Poly && rects_touch(&prims[pi].rects, ch))
            .map(|pi| net_id[pi]);
        // diffusion regions flanking the channel: their nets (for source/drain) and
        // bboxes (for the source→drain axis that sets which channel extent is L vs W)
        let sd_regions: Vec<(usize, Rect)> = cand
            .iter()
            .copied()
            .filter(|&pi| prims[pi].role == Role::Active && rects_touch(&prims[pi].rects, ch))
            .map(|pi| (net_id[pi], union_bbox(&prims[pi].rects)))
            .collect();
        let mut sd: Vec<usize> = sd_regions.iter().map(|(n, _)| *n).collect();
        sd.sort();
        sd.dedup();
        let is_p = nwell.iter().any(|w| overlap(&bbox_of(w), &cb));
        if dbg {
            eprintln!("  ch{i}: cb=({},{},{},{}) gate={:?} sd={} is_p={}", cb.x0, cb.y0, cb.x1, cb.y1, gate, sd.len(), is_p);
        }
        // source/drain nets adjacent to the channel: two distinct for an ordinary
        // transistor; **one** for a MOSCAP (decap / tie cell with source tied to drain) —
        // which Magic still counts as a device, so we must not drop it. 0 (no diffusion)
        // or ≥3 (ambiguous junction) are skipped (and counted as skips below).
        let Some(g) = gate else {
            skipped += 1;
            continue;
        };
        let (s, d) = match sd.as_slice() {
            [a, b] => (*a, *b),
            [a] => (*a, *a),
            _ => {
                skipped += 1;
                continue;
            }
        };
        // bulk: pfet -> the nwell net over the channel; nfet -> the substrate net
        let bulk = if is_p {
            cand
                .iter()
                .copied()
                .find(|&pi| prims[pi].role == Role::Well && rects_touch(&prims[pi].rects, ch))
                .map(|pi| net_name(net_id[pi]))
                .unwrap_or_else(|| rules.substrate.clone())
        } else {
            rules.substrate.clone()
        };
        // base model by type, then a variant if the channel touches a marker layer
        let mut model = if is_p { rules.pfet.clone() } else { rules.nfet.clone() };
        let variants = if is_p { &rules.pfet_variants } else { &rules.nfet_variants };
        for (mlayer, vmodel) in variants {
            if polys_on(cell, *mlayer).iter().any(|p| rects_touch(&tile(p), ch)) {
                model = vmodel.clone();
                break;
            }
        }
        // gate W/L from the channel geometry (skipped for a MOSCAP's tied diffusion),
        // so the comparator's property audit can check drawn dimensions vs schematic
        let params = match (s != d).then(|| channel_wl(&cb, &sd_regions, lib.db_unit)).flatten() {
            Some((w, l)) => BTreeMap::from([("w".to_string(), w), ("l".to_string(), l)]),
            None => BTreeMap::new(),
        };
        devices.push(Device {
            kind: 'M',
            name: format!("M{i}"),
            nodes: vec![net_name(s), net_name(g), net_name(d), bulk],
            model,
            params,
        });
    }
    if dbg && skipped > 0 {
        eprintln!("skipped {skipped} channel(s) (no gate, or 0 / ≥3 diffusion nets)");
    }
    mark("device loop");

    let mut ports: Vec<String> = name.iter().flatten().cloned().collect();
    // the substrate net is a real pin when an nfet ties its bulk to it
    if devices.iter().any(|d| d.nodes.get(3) == Some(&rules.substrate)) {
        ports.push(rules.substrate.clone());
    }
    ports.sort();
    ports.dedup();
    Ok(Netlist { name: cell.name.clone(), ports, devices })
}

pub fn extract_file(gds: &str, top: Option<&str>, rules: &Rules) -> Result<Netlist, String> {
    let lib = Library::load_any(gds)?;
    extract(&lib, top, rules)
}

/// Render an extracted netlist back to SPICE text.
pub fn to_spice(nl: &Netlist) -> String {
    let mut s = String::new();
    s.push_str(&format!(".subckt {} {}\n", nl.name, nl.ports.join(" ")));
    for d in &nl.devices {
        s.push_str(&format!("{} {} {}\n", d.name, d.nodes.join(" "), d.model));
    }
    s.push_str(".ends\n");
    s
}
