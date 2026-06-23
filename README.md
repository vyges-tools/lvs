# vyges-lvs

**Layout-vs-schematic** netlist comparison: two SPICE netlists in — a
layout-extracted one and the schematic/reference — a **MATCH / MISMATCH verdict
with clear divergence diagnostics** out.

> **Vyges open EDA tools.** Commercial-grade silicon sign-off capability, built on
> open standards and plain file formats — meant to be accessible to everyone, not
> only teams who can license a six-figure tool. `vyges-lvs` opens up LVS.

**Docs:** [docs.vyges.com](https://docs.vyges.com) — this engine's chapter, the
[cross-engine integration guide](https://docs.vyges.com/engines/integration.html), and the
job-file formats. In-repo depth: [`docs/engines-integration.md`](docs/engines-integration.md). **Integrating at the binary
level and need help?** → <https://vyges.com/contact>.

## Why this exists

A layout is only correct if it implements the schematic — same devices, same
connectivity. LVS proves that by matching the two netlists as graphs, independent
of how nets and instances happen to be named. When they *don't* match, the only
thing that matters is **where** they diverge — and that is exactly where the open
incumbent is weakest.

## How this is solved today

In production, LVS is one of the commercial LVS tools — gated
behind major licenses. The open baseline is **Netgen** (used in the sky130 flow):
correct, but its divergence output is famously cryptic, so debugging a mismatch is
slow. `vyges-lvs` is an open engine in that space, behind plain SPICE, that leads
with **readable diagnostics** — the unmatched devices and nets, named.

**Describe the job, not the script.** A small declarative `.lvs` file — readable,
diffable — instead of a tool-specific rule script.

## The job (`.lvs`)

```text
layout:    block_layout.spice      # the layout-extracted netlist (side A) …
# … OR extract the layout natively from a GDS (Phase 2):
layout_gds: block.gds              # layout geometry
rules:      block.rules            # layer roles for extraction
schematic: block_schematic.spice   # the reference / schematic netlist (side B)
top:       block                   # subckt to compare (optional; else top-level)
```

```sh
cargo build --release            # std-only, no external deps
vyges-lvs run   examples/inv_chain/match.lvs            # -> MATCH
vyges-lvs run   examples/inv_chain/mismatch.lvs         # -> MISMATCH + diagnostics
vyges-lvs run   examples/inv_chain/mismatch.lvs --fail-on-mismatch   # exit 3 (CI gate)
vyges-lvs run   examples/inv_chain/match.lvs --json     # machine-readable
vyges-lvs demo                                          # built-in pair, no files
# common flags: -o FILE · --json · -q/--quiet · -v/--verbose · -h/--help · -V/--version
```

## How it compares (v0)

Name-independent **graph colour-refinement** (1-WL) on the device/net bipartite
graph: a device's colour folds in its kind/model and its terminals' net-colours (in
order); a net's colour folds in the multiset of (device-colour, terminal-position) it
touches. Refined to a fixed point on the **disjoint union** of both netlists, the two
sides MATCH iff every colour class balances — and the classes that *don't* balance are
the divergence report. Ports are anchored by name (the boundary aligns); internal nets
match purely by structure. SPICE reader handles `.subckt`/`.ends`, `+` continuation,
comments, and M/Q/R/C/L/D/X devices.

**Source/drain are matched symmetrically** (a MOSFET's S/D are interchangeable); **bulk is
matched** (gate and bulk positional).

## Domain coverage — digital *and* analog / mixed-signal

The compare is **device-kind-agnostic**: it matches the netlist *graph*, seeded only by each
device's kind/model, over the generic SPICE primitives `M/Q/R/C/L/D/X`. Nothing in the path
assumes standard cells, Liberty, or a clocked digital netlist — so `vyges-lvs` runs on
**analog and mixed-signal** blocks exactly as it does on digital ones (the only kind-specific
rule is electrically-correct MOSFET S/D symmetry, which analog needs too).

- Digital: `examples/inv_chain/` — a standard-cell inverter chain.
- Analog: `examples/bandgap/` — a bandgap reference exercising bipolar transistors (`Q`),
  resistors (`R`) and a capacitor (`C`) alongside a PMOS mirror (`M`). `match.lvs` → MATCH on a
  renamed/reordered layout; `mismatch.lvs` → MISMATCH on a mis-wired sense resistor (a pure
  connectivity divergence — device counts still balance).

```sh
vyges-lvs run examples/bandgap/match.lvs       # -> MATCH
vyges-lvs run examples/bandgap/mismatch.lvs    # -> MISMATCH (mis-wired R, named)
```

Mixed-signal scope here is **physical connectivity (LVS)**; analog *functional/timing* sign-off
is out of scope (it leans on external SPICE/behavioral tools).

## Native extraction (Phase 2) — `vyges-layout`

`vyges-lvs` can extract the layout netlist **from a GDS** itself, using the vendored
geometry kernel ([`vyges-layout`](https://github.com/vyges-tools/layout)): devices are
gate∩active (`poly AND active`), type from nwell, source/drain from `active − poly`, **bulk**
from the nwell net (pfet) or the substrate net (nfet). **Connectivity is contact-gated** —
shapes on different layers join only where a `contact:` shape overlaps both, so a gate
abutting its source/drain is *not* shorted to them — and **net names** come from TEXT labels.
A small `.rules` file maps layer roles + contacts (per-PDK; the NDA-plugin boundary):

```sh
vyges-lvs extract block.gds --rules block.rules --top block   # GDS -> SPICE
# or in a job: layout_gds: + rules:  ->  extract then compare to the schematic
```

`examples/inv/` is a worked inverter: `inverter.gds` extracts to 2 transistors
(nfet + pfet, correct bulk) and **matches** `schematic.spice`. On a **real sky130 cell**
(`sky130_fd_sc_hd__inv_1`), extraction is **net-level identical to Magic's** golden netlist
(full LVS **MATCH**, hvt included); on a **28-T flop** (`dfrtp_1`) the topology matches Magic
(28 devices, 21 nets); and on a **placed-and-routed multi-cell block** (the counter through
OpenLane, 229 cell instances) it flattens + extracts ~600 transistors end-to-end — see
[`correlation/`](correlation/).

The extractor handles **hierarchy** (SREF/AREF flattened first), **metal-to-metal vias**
(just more `contact:` rules), and **enclosure-gated** contacts (a contact joins two layers
only where it is *enclosed* by a shape on each). Same-layer connectivity is **true geometric
overlap** (shapes tiled into rects, not bounding boxes).

**Honest bounds (depth reserved).** The comparator: 1-WL can't separate certain symmetric
graphs (exact isomorphism with backtracking is depth); a single change can perturb many
colour classes, so the device/net **count and per-kind diffs are the precise headline**.
The extractor (v0): contacts/vias by enclosure (no full DRC), model variants reported as
device **type** (`hvt` resolved via marker-layer rules; `special_nfet` the remaining depth item),
Manhattan boolean — all on the `vyges-layout` depth path. **Net-level LVS parity with Magic holds
on real sky130 cells today** (a 2-T inverter exactly incl. hvt, a 28-T flop's topology). On a
**placed-and-routed multi-cell block** (the counter through OpenLane, 229 instances) extraction
reaches **exact device parity with Magic** — 842 transistors (421 n / 421 p), including decap
MOSCAPs (source=drain) — in **~1.5 s** (release) via a uniform-grid spatial index. **Parasitics**
are handled by the sibling `vyges-extract` engine off the **same routed layout** — the DEF
extracts to standard SPEF (R + coupling C); with a **calibrated sky130A deck** the total cap
tracks OpenRCX to **0.997**, closing the LVS + PEX loop on one block (see
[`correlation/routed-counter.md`](correlation/routed-counter.md)).

## Open core, certified fab plugins

`vyges-lvs` is open (Apache-2.0) and contains **no foundry-confidential data** — it
compares the netlists you supply. Per-PDK **device-recognition / extraction decks**
(Phase 2, layout-side) ship as separate plugins under that foundry's terms, never in
this repository.

## Current state (v0)

SPICE compare with MATCH/MISMATCH verdict, device/net counts, per-kind diffs, and
unmatched-class diagnostics (source/drain symmetric); **native GDS extraction** (Phase 2,
via the vendored `vyges-layout` kernel); text + JSON; a `--fail-on-mismatch` CI gate. Pure
std, unit + example tested offline, no subprocess.

**Verdict-parity correlated against Netgen on a real sky130 block** (see
[`correlation/`](correlation/)): on a synthesized sky130 counter (23 cells), `vyges-lvs`
and Netgen agree 3/3 — MATCH on a renamed/reordered netlist, MISMATCH on a dropped cell
and on a swapped net — with `vyges-lvs` naming the unmatched device/net classes where
Netgen prints a terse "do not match".
