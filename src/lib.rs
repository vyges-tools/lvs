//! vyges-lvs — layout-vs-schematic netlist comparison.
//!
//! The comparator half of LVS: given two SPICE netlists — a layout-extracted one
//! and the schematic/reference — decide whether they describe the **same circuit**,
//! independent of net and instance names, and report **where they diverge** when
//! they don't. (The weak open incumbent, Netgen, is correct but its diagnostics are
//! cryptic; clear divergence reporting is the point here.)
//!
//! Boundaries (per the Vyges flow architecture): inputs and outputs are files
//! (two SPICE netlists in, a match report out). The whole v0 is pure std and
//! unit-tested offline — there is no subprocess. The correlation baseline
//! (Netgen) is not a runtime dependency.
//!
//! v0 scope: structural equivalence by **graph colour-refinement** (1-WL) on the
//! device/net bipartite graph — device kind + per-terminal net colours refine until
//! stable, then the two netlists' colour multisets are compared. Catches device/net
//! count and connectivity divergence with per-class diagnostics. Depth reserved:
//! exact isomorphism with backtracking for refinement-equivalent symmetric graphs,
//! source/drain (and other terminal) symmetry, series/parallel device folding, and
//! native layout extraction (GDS → devices, via `vyges-layout`).

pub mod spice;
pub mod compare;
pub mod job;
pub mod engine;
// geometry kernel now comes from the shared vyges-layout foundation (was vendored
// under src/layout/). Aliased so `crate::layout::{boolean,gds,geom,flatten}` keeps
// resolving across the engine.
pub use vyges_layout as layout;
pub mod extract; // native device extraction (GDS -> netlist)

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COPYRIGHT: &str = "© 2026 Vyges. All Rights Reserved.  https://vyges.com";
