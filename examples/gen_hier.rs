//! Generates examples/inv/inv_hier.gds — the inverter placed via an SREF in a top
//! cell, with the A pin routed up to met2 through a via. Exercises hierarchy
//! (flatten), metal->metal vias, and enclosure together. Extracts to the same
//! inverter and MATCHES examples/inv/schematic.spice (with examples/inv/hier.rules).
//! Run: `cargo run --example gen_hier`.
use vyges_lvs::layout::gds::{Cell, Element, Library};
use vyges_lvs::layout::geom::Rect;

fn b(layer: i16, r: Rect) -> Element {
    Element::Boundary { layer, datatype: 0, pts: r.as_boundary() }
}
fn t(layer: i16, x: i32, y: i32, s: &str) -> Element {
    Element::Text { layer, texttype: 0, x, y, string: s.into() }
}

fn main() {
    // inv_core: the inverter geometry (NO labels — labels live on the top cell)
    let inv_core = Cell {
        name: "inv_core".into(),
        elements: vec![
            b(1, Rect::new(0, 0, 300, 100)),     // nfet diff
            b(1, Rect::new(0, 300, 300, 400)),   // pfet diff
            b(3, Rect::new(-50, 250, 350, 450)), // nwell
            b(2, Rect::new(130, -50, 170, 450)), // poly gate
            b(5, Rect::new(0, 0, 120, 100)),     // VSS met1
            b(5, Rect::new(0, 300, 120, 400)),   // VDD met1
            b(5, Rect::new(180, 0, 300, 400)),   // Y met1
            b(5, Rect::new(135, 180, 165, 220)), // A met1 (gate pin)
            b(6, Rect::new(40, 40, 60, 60)),     // contacts diff/poly -> met1
            b(6, Rect::new(220, 40, 240, 60)),
            b(6, Rect::new(40, 340, 60, 360)),
            b(6, Rect::new(220, 340, 240, 360)),
            b(6, Rect::new(145, 190, 155, 210)),
            b(9, Rect::new(90, 360, 110, 380)),  // nwell tap
        ],
    };
    // top: place inv_core, route A up to met2 (10) through a via (11), label on top
    let top = Cell {
        name: "inv_hier".into(),
        elements: vec![
            Element::Sref { sname: "inv_core".into(), x: 0, y: 0, reflect: false, mag: 1.0, angle: 0.0 },
            b(10, Rect::new(140, 185, 160, 215)), // met2 over the A pin
            b(11, Rect::new(145, 190, 155, 210)), // via met1 -> met2 (inside both)
            t(8, 150, 200, "A"),  // A pin (now met1+met2 via the via)
            t(8, 240, 200, "Y"),
            t(8, 60, 50, "VSS"),
            t(8, 60, 350, "VDD"),
        ],
    };
    let lib = Library { name: "INVH".into(), cells: vec![inv_core, top], ..Library::default() };
    std::fs::create_dir_all("examples/inv").unwrap();
    lib.save("examples/inv/inv_hier.gds").unwrap();
    println!("wrote examples/inv/inv_hier.gds");
}
