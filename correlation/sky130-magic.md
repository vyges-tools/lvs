# Native extraction — vyges-lvs vs Magic on the real sky130 inverter

Correlating `vyges-lvs`'s native GDS extraction against the golden open extractor
(**Magic** `ext2spice`) on a **real sky130 standard cell**:
`sky130_fd_sc_hd__inv_1`. Both tools read the *same* layout and produce a transistor
netlist; we compare what they extract. Magic runs inside the OpenLane2/LibreLane
container; `vyges-lvs` reads the GDS Magic writes for the cell.

## Setup (on a host with the sky130 PDK + the OpenLane2 container)

```sh
# Magic: golden extract + a single-cell GDS for vyges-lvs (in the container)
magic -dnull -noconsole -rcfile $PDK/libs.tech/magic/sky130A.magicrc <<EOF
load sky130_fd_sc_hd__inv_1
extract all
ext2spice lvs
ext2spice -o magic_inv.spice
gds write inv_1.gds
EOF

# vyges-lvs: native extraction from the same GDS, with sky130 layer rules
vyges-lvs extract inv_1.gds --rules sky130.rules --top sky130_fd_sc_hd__inv_1
```

`sky130.rules` maps the sky130 layers: active 65/20, poly 66/20, nwell 64/20,
li1 67/20, licon1 66/44 (diff/poly→li1), mcon 67/44 (li1→met1); pin labels on
67/5 (li1), 68/5 (met1), 64/5 (nwell); substrate `VNB`.

## Result — full net-level LVS MATCH ✅

`vyges-lvs` extracts a netlist **identical to Magic's** from the real sky130 inverter:

```text
.subckt sky130_fd_sc_hd__inv_1 A VGND VNB VPB VPWR Y
M0 Y A VGND VNB sky130_fd_pr__nfet_01v8        # nfet: d=Y g=A s=VGND b=VNB
M1 Y A VPWR VPB sky130_fd_pr__pfet_01v8_hvt    # pfet: d=Y g=A s=VPWR b=VPB
.ends
```

| | Magic (golden) | vyges-lvs |
| --- | --- | --- |
| Devices | 2 | **2** |
| Types | nfet + pfet (hvt) | **nfet + pfet** (pfet via nwell) |
| Nets | 6 (A, Y, VPWR, VGND, VPB, VNB) | **6 — same connectivity** |
| Bulk | nfet→VNB, pfet→VPB | **nfet→VNB, pfet→VPB (nwell)** |

Running the comparator on the vyges extraction vs Magic's golden (in M-device form):

```text
vyges-lvs — MATCH ✓
  devices A 2  B 2 ; nets A 6  B 6 ; the two netlists are structurally equivalent.
```

What made it work on production geometry:
- **geometric (not bbox) same-layer connectivity** — shapes on a layer are tiled into
  rects and grouped by *true* overlap, so the real `li1` routing no longer over-merges
  (gate/source/drain stay distinct);
- **contact-gated** cross-layer joins (licon/mcon), **bulk** from the nwell net and the
  substrate, and **multi-layer pin labels** (a label prefers a prim on its own base layer,
  so the `nwell` label names the body net VPB).

## Scale — a real 28-transistor flop

On `sky130_fd_sc_hd__dfrtp_1` (a flip-flop, **28 transistors**, with the cell's full
internal li1/met routing), vyges-lvs extracts and **matches Magic's full topology**:

| | Magic (golden) | vyges-lvs |
| --- | --- | --- |
| Devices | 28 (14 nfet, 14 pfet) | **28 (14 nfet, 14 pfet)** |
| Nets | 21 | **21** |
| Verdict (generic models) | — | **MATCH ✓** (structurally equivalent) |

The device and **net counts match exactly**, and with model variants normalised to device
type (nfet/pfet) the netlists are **structurally equivalent**. The only divergence at full
strictness is lib **variant naming** — Magic distinguishes `special_nfet_01v8` / `pfet_…_hvt`,
which vyges-lvs now resolves via `pfet_variant:`/`nfet_variant:` rules (a channel touching a
marker layer → the variant model): with `pfet_variant: 78/44 …_hvt` (hvtp), **inv_1 matches
Magic's golden exactly, hvt included**. `special_nfet` (no single marker layer) is the
remaining variant depth item.

## Hierarchy + metal vias + enclosure

`examples/inv/inv_hier.gds` places the inverter via an **SREF** and routes the A pin up to
**met2 through a via**: extraction **flattens** the hierarchy, joins the layers only through
**enclosure-gated** contacts/vias, and **MATCHES** the schematic.

Honest bound: contacts/vias matched by enclosure (no full DRC), model variants via
marker-layer rules (hvt validated exactly; `special_nfet` a depth item), and parasitics not
yet extracted. But **net-level LVS parity with Magic holds on real sky130 cells — a 2-T
inverter (exact, hvt included) and a 28-T flop (topology)** — with hierarchy, metal vias,
enclosure, and variant detection all exercised. The next integration step is a
placed-and-routed multi-cell block (the synthesized counter through OpenLane), flat vs Magic.
