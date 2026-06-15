# vyges-lvs in the Vyges engine flow

> **Cross-engine guide moved.** How all the Vyges engines compose, and where each plugs into an
> OpenROAD / LibreLane / OpenLane 2 flow, is now maintained once at
> **<https://docs.vyges.com/engines/integration.html>** (incl. the "drop these in" pre-P&R vs
> post-layout split). This stub stays so existing links keep working; the cross-engine map no
> longer lives per-repo (to avoid copy-drift).

## Where `vyges-lvs` sits

`vyges-lvs` is the **layout-correctness** check: does the layout implement the schematic? It sits
at sign-off, beside the timing / power / power-integrity engines.

```text
  schematic netlist ─┐
                     ├─► vyges-lvs ─► MATCH / MISMATCH (+ divergence report)
  layout netlist ────┘
   (extracted)
```

It runs on a layout netlist + the schematic, or extracts the layout side natively from GDS
(`layout_gds:` + a layer `.rules` deck). It's the open, legible alternative to Netgen — it
**names** the unmatched device/net classes where Netgen prints a terse "do not match."

## lvs-specific depth (code-coupled, stays in this repo)

- [`correlation/`](../correlation/) — the validated proof: net-level MATCH vs Magic on a real
  sky130 cell, exact 842/842 device parity on the routed counter, and 3/3 verdict agreement vs Netgen.
