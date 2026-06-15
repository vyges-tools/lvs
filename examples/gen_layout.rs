//! Generates examples/inv/inverter.gds — a contact-gated inverter layout for native
//! LVS extraction. Layers (see examples/inv/inv.rules): active 1, poly 2, nwell 3,
//! met1 5, contact 6 (diff/poly->met1), tap 9 (nwell->met1), label 8.
//! Run: `cargo run --example gen_layout`.
use vyges_lvs::layout::gds::{Cell, Element, Library};
use vyges_lvs::layout::geom::Rect;

fn b(layer: i16, r: Rect) -> Element {
    Element::Boundary { layer, datatype: 0, pts: r.as_boundary() }
}
fn t(layer: i16, x: i32, y: i32, s: &str) -> Element {
    Element::Text { layer, texttype: 0, x, y, string: s.into() }
}

fn main() {
    let cell = Cell {
        name: "inverter".into(),
        elements: vec![
            // diffusion (active): nfet strip (bottom) + pfet strip (top)
            b(1, Rect::new(0, 0, 300, 100)),
            b(1, Rect::new(0, 300, 300, 400)),
            // nwell around the pfet
            b(3, Rect::new(-50, 250, 350, 450)),
            // shared poly gate crossing both strips
            b(2, Rect::new(130, -50, 170, 450)),
            // met1: VSS, VDD, Y, and a small A pin over the gate
            b(5, Rect::new(0, 0, 120, 100)),     // VSS
            b(5, Rect::new(0, 300, 120, 400)),   // VDD
            b(5, Rect::new(180, 0, 300, 400)),   // Y (both drains)
            b(5, Rect::new(135, 180, 165, 220)), // A (gate pin)
            // contacts (diff/poly -> met1): source, drain x2, and the gate
            b(6, Rect::new(40, 40, 60, 60)),     // nfet source diff -> VSS
            b(6, Rect::new(220, 40, 240, 60)),   // nfet drain  diff -> Y
            b(6, Rect::new(40, 340, 60, 360)),   // pfet source diff -> VDD
            b(6, Rect::new(220, 340, 240, 360)), // pfet drain  diff -> Y
            b(6, Rect::new(145, 190, 155, 210)), // poly gate        -> A
            // well tap (nwell -> met1): ties the nwell to VDD
            b(9, Rect::new(90, 360, 110, 380)),
            // pin labels
            t(8, 60, 50, "VSS"),
            t(8, 60, 350, "VDD"),
            t(8, 240, 200, "Y"),
            t(8, 150, 200, "A"),
        ],
    };
    let lib = Library { name: "INV".into(), cells: vec![cell], ..Library::default() };
    std::fs::create_dir_all("examples/inv").unwrap();
    lib.save("examples/inv/inverter.gds").unwrap();
    println!("wrote examples/inv/inverter.gds");
}
