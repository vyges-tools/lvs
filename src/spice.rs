//! Minimal SPICE netlist reader for LVS comparison.
//!
//! Parses a flat or single-`.subckt` netlist into typed devices (kind, instance
//! name, ordered terminal nets, model/value). Handles `+` line continuation,
//! `*`/`;`/`$` comments, `.subckt`/`.ends` scoping, and the common device kinds
//! (M, Q, R, C, L, D, X). Other dot-commands are ignored. Case-insensitive.
//!
//! Depth reserved: hierarchical flattening of `X` subckt instances, inline `X`
//! parameters, and `.param` evaluation.

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
    /// single subckt, else the flat top-level devices.
    pub fn parse(text: &str, top: Option<&str>) -> Result<Netlist, SpiceError> {
        let stmts = statements(text);
        // collect subckts
        let mut subckts: Vec<Netlist> = Vec::new();
        let mut flat: Vec<Device> = Vec::new();
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
            if head.starts_with('.') {
                continue; // .model/.param/.include/.global/.end/...
            }
            if let Some(dev) = parse_device(&toks)? {
                match cur.as_mut() {
                    Some(n) => n.devices.push(dev),
                    None => flat.push(dev),
                }
            }
        }

        if let Some(want) = top {
            return subckts
                .into_iter()
                .find(|s| s.name.eq_ignore_ascii_case(want))
                .ok_or_else(|| SpiceError(format!("subckt {want:?} not found")));
        }
        match subckts.len() {
            0 => Ok(Netlist { name: "(top)".into(), ports: Vec::new(), devices: flat }),
            1 => Ok(subckts.into_iter().next().unwrap()),
            _ => Err(SpiceError(format!(
                "{} subckts; pass `top:` to choose ({})",
                subckts.len(),
                subckts.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

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
}
