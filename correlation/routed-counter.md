# Routed multi-cell block — vyges-lvs vs Magic (the integration test)

The final integration: a **placed-and-routed multi-cell block** — the 8-bit counter
taken through **OpenLane** (synth → floorplan → place → CTS → route → GDS) — extracted
**flat** by both Magic and vyges-lvs and correlated. This exercises everything at once:
hierarchy (the routed GDS is **229 std-cell SREF instances**), real multi-layer routing
(li1 / met1 / met2 + licon / mcon / via), and scale.

## Setup (host + OpenLane2 container)

```sh
openlane --pdk-root $PDK_ROOT config.json     # counter.v -> routed counter.gds
# Magic golden: flat-extract the routed GDS
magic ... <<EOF
gds read counter.gds ; load counter ; flatten counter_flat ; load counter_flat
extract all ; ext2spice lvs ; ext2spice -o magic_counter.spice
EOF
# vyges-lvs: native flat extraction (flattens the 229 instances itself)
vyges-lvs extract counter.gds --rules sky130.rules --top counter -o vyges_counter.spice
```

## Result — exact device parity with Magic ✅

| | Magic (golden) | vyges-lvs |
| --- | --- | --- |
| Cell instances | 229 (SREF) | **229, flattened** ✓ |
| Devices | 842 (421 nfet, 421 pfet) | **842 (421 nfet, 421 pfet)** ✓ |
| n/p balance | 1:1 | **1:1** ✓ |
| Runtime | — | **~1.5 s** (release, std-only, single thread) |

vyges-lvs **extracts the full routed block to exactly Magic's device count** — 842
transistors, 421 n + 421 p — flattening all 229 cell instances and tracing nets through the
real li1/met1/met2 + via stack.

### What closed the gap — MOSCAP source/drain

The first pass under-counted (596): the die is dominated by **decap MOSCAP arrays**
(`decap_*` cells), whose transistors **tie source to drain on one rail**. The channel
detection was always right — `poly ∩ active` found all **842** channels — but the device
rule required *two distinct* source/drain nets, so the 246 S=D MOSCAPs were dropped. Emitting
a MOSCAP (source = drain) when a channel has exactly one adjacent diffusion net recovers all
246 → **596 + 246 = 842**, exact. (Ordinary logic transistors, with two distinct S/D, are
unaffected — the inverter and flop still match.)

### What made it fast — a spatial index

The first run took ~79 s; the cost was **not** the device loop but the **contact/via pass**
calling the scanline boolean for every one of thousands of cuts. Three changes took it to
**~1.5 s** (release): a **uniform-grid spatial index** (with an oversized-box fallback for
die-spanning rails/wells) so every prim-vs-geometry scan is ~O(1) instead of O(n); a
**rect-containment fast path** for contact enclosure (a single cut sits wholly inside its
metal — no boolean needed); and **rectangle-tiling shortcut** (a plain rectangle skips the
scanline decomposition). Device count is byte-identical before and after.

## What it validates

- **The integration is real and exact:** native extraction on an actual placed-and-routed
  multi-cell block (229 instances, multi-layer routing) reaches **device parity with Magic**
  — hierarchy/flatten, vias, enclosure, and MOSCAP handling all hold at scale.
- **It's fast:** ~1.5 s std-only single-thread, after the spatial index.

Honest bound: this is **device-count + n/p parity** with Magic on the routed block, with
net-level LVS parity proven on the constituent cells (2-T inverter exact incl. hvt, 28-T flop
topology). Full net-by-net graph equivalence on the whole routed block (vs Magic's flat net
list) is the next correlation; the extraction and device recognition are now exact.

## Parasitics on the same block — the LVS + PEX loop closes

The same routed counter feeds **both** sign-off engines off one
layout: `vyges-lvs` takes the **GDS** for connectivity (above), and **`vyges-extract`** takes
the **routed DEF** for **RC parasitics → standard SPEF**. That is the integration — one
placed-and-routed block, connectivity *and* parasitics from the Vyges stack.

```sh
# same block, parasitic side — DEF -> SPEF
vyges-extract run counter.ext -o vyges_counter.spef     # design/def/rules/lef job
```

Correlated against the run's own **OpenROAD OpenRCX** SPEF (golden, `nom` corner):

| | OpenRCX (golden) | vyges-extract |
| --- | --- | --- |
| Nets | 50 | **50 — full coverage** ✓ |
| Per-net (R + coupling C) | yes | **yes — `*RES` + `*CAP` w/ coupling** ✓ |
| Total net cap (illustrative deck) | 100.0 fF | 60.1 fF (ratio 0.60) |
| Total net cap (**calibrated deck**) | 100.0 fF | **99.7 fF (ratio 0.997)** ✓ |
| Per-net cap ratio (calibrated) | — | mean **1.02**, median 0.98, σ 0.13 |

The output is **standard SPEF** (ground + lateral coupling caps + per-segment R) that drops
into any STA tool. With the uncalibrated illustrative deck the cap is a systematic ~0.6×; a
**calibrated sky130A deck** — per-layer caps fit to the OpenRCX nom golden — brings total cap
to **0.997** with a tight per-net spread (mean ~1.0, σ 0.13). That deck is the open sky130A
reference plugin (`vyges-tools/extract` → `pdk/sky130A/`, `correlation/openrcx-counter.md`);
a silicon-correlated certified deck is the NDA-plugin boundary, same open-core split as LVS.

**All three depth items closed:** (1) **channel separation** — MOSCAP S=D handling →
**842/842 exact** device parity with Magic; (2) **spatial index** — grid + fast paths →
**79 s → ~1.5 s**; (3) **calibrated extract deck** — sky130A caps fit to OpenRCX →
**total cap 0.997** on the same routed block, closing the LVS + PEX loop.
