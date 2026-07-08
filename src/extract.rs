//! Native device extraction — provided by the shared `vyges-layout` foundation.
//!
//! The extractor (rules + geometry->devices pass) now lives in
//! `vyges_layout::extract` so it can be shared across engines. This thin wrapper
//! re-exports the rules type and adapts the foundation's neutral `Netlist` to this
//! crate's `spice::Netlist`, which the comparator consumes.

pub use crate::layout::extract::Rules;

use crate::spice::{Device, Netlist};

/// Adapt the foundation's neutral netlist to this crate's SPICE netlist type.
fn adapt(nl: crate::layout::netlist::Netlist) -> Netlist {
    Netlist {
        name: nl.name,
        ports: nl.ports,
        devices: nl
            .devices
            .into_iter()
            .map(|d| Device { kind: d.kind, name: d.name, nodes: d.nodes, model: d.model, params: d.params })
            .collect(),
    }
}

/// Extract a GDS/OASIS layout to this crate's `Netlist` (via the foundation).
pub fn extract_file(gds: &str, top: Option<&str>, rules: &Rules) -> Result<Netlist, String> {
    crate::layout::extract::extract_file(gds, top, rules).map(adapt)
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
