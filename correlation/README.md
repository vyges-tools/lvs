# Correlation — vyges-lvs vs Netgen (verdict parity, real sky130)

A reproducible **verdict-parity** correlation of `vyges-lvs` against the open
baseline (**Netgen**) on a real sky130 block. `counter.v` is synthesized to
`sky130_fd_sc_hd` (yosys) and written as a cell-level SPICE; `run.sh` then builds a
rename+reorder copy and two injected-error variants and runs **both tools** on each,
checking they reach the same MATCH / MISMATCH verdict.

`run.sh` needs `yosys`, a sky130 hd `.lib` + the sky130 Netgen `setup.tcl`, a Netgen,
and a built `vyges-lvs`. Netgen typically lives inside the **OpenLane2 / LibreLane
container**, so `NETGEN` can be a container wrapper (see the header of `run.sh`).

## Result (sky130 counter, 23 cells)

| Case | What changed | vyges-lvs | Netgen | Agree? |
| --- | --- | --- | --- | --- |
| **match** | internal net renamed + instances reordered | **MATCH** | "Circuits match uniquely." | ✅ |
| **drop**  | one cell instance removed | **MISMATCH** | "Netlists do not match." | ✅ |
| **swap**  | two nets swapped on one instance | **MISMATCH** | "Netlists do not match." | ✅ |

**3/3 verdict parity** — `vyges-lvs` reaches the same conclusion as Netgen on a real
sky130 netlist across a clean (name-independent) case and two injected errors. The
clean case proves name/order independence (both tools see the renamed, reordered
netlist as the same circuit); the error cases prove both detect a missing device and a
mis-wire.

## What it validates / what's next

- **Verdict parity with the open baseline** on real sky130 cells — the core LVS
  contract (same circuit ⇒ match; any structural change ⇒ mismatch) holds, matching Netgen.
- **`vyges-lvs` leads on diagnostics.** Where Netgen prints terse "do not match",
  `vyges-lvs` names the unmatched device/net classes and the per-kind count diff.
- **Next (depth):** a larger injected-error battery (duplicate, short, model swap) with
  localized root-causing; symmetric-graph cases where 1-WL ties (the exact-isomorphism
  escalation); and **Phase 2** — native GDS extraction via `vyges-layout` so the *layout*
  side comes from real geometry (Magic/Netgen extraction is the baseline there too).

Honest bound: this correlates the **comparator** on cell-level netlists; full
transistor-level LVS with native layout extraction is the Phase-2 depth pass.
