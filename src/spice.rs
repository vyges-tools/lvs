//! Minimal SPICE netlist reader for LVS comparison.
//!
//! Parses a flat or hierarchical netlist into typed devices (kind, instance
//! name, ordered terminal nets, model/value). Handles `+` line continuation,
//! `*`/`;`/`$` comments, `.subckt`/`.ends` scoping, `.global`, and the common
//! device kinds (M, Q, R, C, L, D, X). Other dot-commands are ignored.
//! Case-insensitive.
//!
//! **Hierarchical flattening**: every `X` subckt instance whose definition is
//! present is recursively expanded down to primitive devices — formal ports are
//! bound to the actual nets at the call site, internal nets are uniquified by the
//! instance path, and `.global` (plus SPICE node `0`) names stay global. This is
//! what turns a cell-level connectivity check into a *transistor-level* compare:
//! two layouts that instantiate the same cells but wire their internals
//! differently now diverge. An `X` whose subckt is *not* defined degrades to an
//! opaque device (its connectivity is still mapped through the hierarchy).
//!
//! Depth reserved: inline `X` parameters and `.param` evaluation.

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub kind: char,         // normalized uppercase first letter (M/Q/R/C/L/D/X)
    pub name: String,       // instance name (as written)
    pub nodes: Vec<String>, // ordered terminal nets
    pub model: String,      // model / subckt / value (may be empty)
}

#[derive(Debug, Clone, Default)]
pub struct Netlist {
    pub name: String,
    pub ports: Vec<String>,
    pub devices: Vec<Device>,
}

#[derive(Debug)]
pub struct SpiceError(pub String);
impl std::fmt::Display for SpiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "spice error: {}", self.0)
    }
}
impl std::error::Error for SpiceError {}

/// Terminal count for a device kind (X is variable -> handled separately).
fn fixed_terms(kind: char) -> Option<usize> {
    match kind {
        'M' => Some(4), // d g s b
        'Q' => Some(3), // c b e
        'R' | 'C' | 'L' | 'D' => Some(2),
        _ => None,
    }
}

impl Netlist {
    pub fn load(path: &str) -> Result<Netlist, SpiceError> {
        let text = std::fs::read_to_string(path).map_err(|e| SpiceError(format!("{path}: {e}")))?;
        Netlist::parse(&text, None)
    }

    /// Parse `text`; if `top` is given, return that `.subckt`, else the named
    /// single subckt, else the flat top-level devices. The chosen cell is
    /// **flattened to primitive devices** against the full subckt table (see
    /// [`flatten`]).
    pub fn parse(text: &str, top: Option<&str>) -> Result<Netlist, SpiceError> {
        let stmts = statements(text);
        // collect subckts + global nets
        let mut subckts: Vec<Netlist> = Vec::new();
        let mut flat: Vec<Device> = Vec::new();
        let mut globals: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        globals.insert("0".into()); // SPICE global ground
        let mut cur: Option<Netlist> = None;

        for st in &stmts {
            let toks: Vec<&str> = st.split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            let head = toks[0].to_ascii_lowercase();
            if head == ".subckt" {
                let name = toks.get(1).map(|s| s.to_string()).unwrap_or_default();
                let ports = toks[2..].iter().map(|s| s.to_string()).collect();
                cur = Some(Netlist { name, ports, devices: Vec::new() });
                continue;
            }
            if head == ".ends" {
                if let Some(n) = cur.take() {
                    subckts.push(n);
                }
                continue;
            }
            if head == ".global" {
                globals.extend(toks[1..].iter().map(|s| s.to_string()));
                continue;
            }
            if head.starts_with('.') {
                continue; // .model/.param/.include/.end/...
            }
            if let Some(dev) = parse_device(&toks)? {
                match cur.as_mut() {
                    Some(n) => n.devices.push(dev),
                    None => flat.push(dev),
                }
            }
        }

        // subckt-definition table (case-insensitive lookup by name)
        let table: FlatMap<String, Netlist> =
            subckts.iter().map(|s| (s.name.to_ascii_lowercase(), s.clone())).collect();

        let chosen = if let Some(want) = top {
            table
                .get(&want.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| SpiceError(format!("subckt {want:?} not found")))?
        } else {
            match subckts.len() {
                0 => Netlist { name: "(top)".into(), ports: Vec::new(), devices: flat },
                1 => subckts.into_iter().next().unwrap(),
                _ => {
                    return Err(SpiceError(format!(
                        "{} subckts; pass `top:` to choose ({})",
                        subckts.len(),
                        subckts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
                    )))
                }
            }
        };

        Ok(flatten(&chosen, &table, &globals))
    }
}

use std::collections::BTreeMap as FlatMap;

/// Recursively expand the cell's `X` instances down to primitive devices.
///
/// At each level a node is resolved in the current namespace: a formal port maps
/// to the actual net passed at the call site, a global stays itself, and any
/// other (internal) net is prefixed with the instance path so two instances of
/// the same cell get distinct internal nets. An `X` whose subckt is absent from
/// `table` is emitted as an opaque device with its nodes mapped — connectivity is
/// preserved even when the definition is unavailable.
pub fn flatten(
    top: &Netlist,
    table: &FlatMap<String, Netlist>,
    globals: &std::collections::BTreeSet<String>,
) -> Netlist {
    let mut out: Vec<Device> = Vec::new();
    // current frame: formal->resolved-actual substitution + instance-path prefix
    let resolve = |node: &str, subst: &FlatMap<String, String>, prefix: &str| -> String {
        if let Some(actual) = subst.get(node) {
            actual.clone()
        } else if globals.contains(node) {
            node.to_string()
        } else {
            format!("{prefix}{node}")
        }
    };
    // explicit recursion guarded by depth; globals/ports captured in `resolve`
    expand(top, table, &FlatMap::new(), "", 0, &resolve, &mut out);
    Netlist { name: top.name.clone(), ports: top.ports.clone(), devices: out }
}

#[allow(clippy::too_many_arguments)]
fn expand(
    cell: &Netlist,
    table: &FlatMap<String, Netlist>,
    subst: &FlatMap<String, String>,
    prefix: &str,
    depth: usize,
    resolve: &dyn Fn(&str, &FlatMap<String, String>, &str) -> String,
    out: &mut Vec<Device>,
) {
    for d in &cell.devices {
        let nodes: Vec<String> = d.nodes.iter().map(|n| resolve(n, subst, prefix)).collect();
        let sub = (d.kind == 'X')
            .then(|| table.get(&d.model.to_ascii_lowercase()))
            .flatten();
        match sub {
            Some(def) if depth < MAX_HIER_DEPTH && def.ports.len() == nodes.len() => {
                // bind formals -> resolved actuals; recurse into the child namespace
                let child_subst: FlatMap<String, String> =
                    def.ports.iter().cloned().zip(nodes.iter().cloned()).collect();
                let child_prefix = format!("{prefix}{}/", d.name);
                expand(def, table, &child_subst, &child_prefix, depth + 1, resolve, out);
            }
            _ => out.push(Device {
                kind: d.kind,
                name: format!("{prefix}{}", d.name),
                nodes,
                model: d.model.clone(),
            }),
        }
    }
}

/// Recursion guard — a malformed self-referential subckt won't expand forever.
const MAX_HIER_DEPTH: usize = 100;

fn parse_device(toks: &[&str]) -> Result<Option<Device>, SpiceError> {
    let name = toks[0].to_string();
    let kind = name.chars().next().unwrap_or('?').to_ascii_uppercase();
    if kind == 'X' {
        // Xname n1 .. nk subcktname  (inline params after the name are not handled)
        if toks.len() < 3 {
            return Ok(None);
        }
        let model = toks[toks.len() - 1].to_string();
        let nodes = toks[1..toks.len() - 1].iter().map(|s| s.to_string()).collect();
        return Ok(Some(Device { kind, name, nodes, model }));
    }
    let Some(nt) = fixed_terms(kind) else {
        return Ok(None); // unknown device kind -> skip (don't guess connectivity)
    };
    if toks.len() < 1 + nt {
        return Err(SpiceError(format!("device {name} needs {nt} terminals: {:?}", toks.join(" "))));
    }
    let nodes = toks[1..1 + nt].iter().map(|s| s.to_string()).collect();
    let model = toks.get(1 + nt).map(|s| s.to_string()).unwrap_or_default();
    Ok(Some(Device { kind, name, nodes, model }))
}

/// Split into statements: strip comments, join `+` continuations.
fn statements(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        // strip `*` full-line and `;`/`$` inline comments
        let mut line = raw;
        if line.trim_start().starts_with('*') {
            continue;
        }
        if let Some(i) = line.find([';', '$']) {
            line = &line[..i];
        }
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with('+') {
            // continuation -> append to previous statement
            let cont = line.trim_start().trim_start_matches('+').trim();
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(cont);
                continue;
            }
        }
        out.push(line.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NL: &str = "\
.subckt inv A Y VDD VSS
Mp Y A VDD VDD pfet w=1 l=0.15
Mn Y A VSS VSS nfet w=0.5 l=0.15
.ends
";

    #[test]
    fn parses_subckt_devices() {
        let n = Netlist::parse(NL, None).unwrap();
        assert_eq!(n.name, "inv");
        assert_eq!(n.ports, ["A", "Y", "VDD", "VSS"]);
        assert_eq!(n.devices.len(), 2);
        assert_eq!(n.devices[0].kind, 'M');
        assert_eq!(n.devices[0].nodes, ["Y", "A", "VDD", "VDD"]);
        assert_eq!(n.devices[0].model, "pfet");
    }

    #[test]
    fn continuation_and_comments() {
        let t = "* header\nR1 a b\n+ 1k ; trailing\nC1 b 0 1p\n";
        let n = Netlist::parse(t, None).unwrap();
        assert_eq!(n.devices.len(), 2);
        assert_eq!(n.devices[0].nodes, ["a", "b"]);
        assert_eq!(n.devices[0].model, "1k");
    }

    const HIER: &str = "\
.subckt inv A Y VDD VSS
Mp Y A VDD VDD pfet
Mn Y A VSS VSS nfet
.ends
.subckt buf A Y VDD VSS
Xi1 A M VDD VSS inv
Xi2 M Y VDD VSS inv
.ends
";

    #[test]
    fn flattens_to_transistors() {
        let n = Netlist::parse(HIER, Some("buf")).unwrap();
        // two inverters expand to four MOSFETs; no X survives
        assert_eq!(n.devices.len(), 4);
        assert!(n.devices.iter().all(|d| d.kind == 'M'));
        // ports stay at the boundary; the internal node M is shared between the
        // two instances (passed as an actual), not path-prefixed
        let nets: std::collections::BTreeSet<&str> =
            n.devices.iter().flat_map(|d| d.nodes.iter().map(|s| s.as_str())).collect();
        assert!(nets.contains("M"), "shared internal net M should survive: {nets:?}");
        assert!(nets.contains("VDD") && nets.contains("VSS"));
        // the inverter's own internal nodes (none here) would be path-prefixed;
        // instance device names are path-qualified and unique
        let names: std::collections::BTreeSet<&str> =
            n.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), 4, "instance device names must be unique: {names:?}");
    }

    #[test]
    fn internal_wiring_difference_is_visible_after_flatten() {
        use crate::compare::compare;
        // same cells, but the schematic chains inv->inv where the layout shorts
        // both inputs to A — a real connectivity bug a cell-level check misses.
        let bad = "\
.subckt inv A Y VDD VSS
Mp Y A VDD VDD pfet
Mn Y A VSS VSS nfet
.ends
.subckt buf A Y VDD VSS
Xi1 A M VDD VSS inv
Xi2 A Y VDD VSS inv
.ends
";
        let good = Netlist::parse(HIER, Some("buf")).unwrap();
        let broken = Netlist::parse(bad, Some("buf")).unwrap();
        let r = compare(&good, &broken);
        assert!(!r.matched, "miswired internal net must MISMATCH at transistor level");
    }

    #[test]
    fn undefined_subckt_stays_opaque() {
        let t = "\
.subckt top A Y VDD VSS
Xb A Y VDD VSS blackbox
.ends
";
        let n = Netlist::parse(t, Some("top")).unwrap();
        assert_eq!(n.devices.len(), 1);
        assert_eq!(n.devices[0].kind, 'X');
        assert_eq!(n.devices[0].nodes, ["A", "Y", "VDD", "VSS"]);
    }
}
