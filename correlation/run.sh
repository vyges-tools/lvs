#!/usr/bin/env bash
# Verdict-parity correlation: vyges-lvs vs Netgen on a real sky130 block.
#
# Synthesizes counter.v to sky130 (yosys), emits a cell-level SPICE, then builds a
# rename+reorder copy (should MATCH) and two injected-error variants (a dropped cell
# and a swapped net — should MISMATCH), and runs BOTH tools on each, confirming they
# agree on the verdict.
#
# Needs: yosys; a sky130 hd .lib + the sky130 Netgen setup.tcl; a Netgen; a built
# vyges-lvs. Netgen commonly lives inside the OpenLane2 / LibreLane container, so
# NETGEN can be a wrapper, e.g.:
#
#   NETGEN='docker run --rm -v '"$PWD"':/work -v "$PDK_ROOT":/pdk:ro -w /work \
#           ghcr.io/efabless/openlane2:2.3.10 netgen'
#   NETGEN_SETUP=/pdk/.../sky130A/libs.tech/netgen/setup.tcl   # path INSIDE the container
#   SKY130_LIB=.../sky130_fd_sc_hd__tt_025C_1v80.lib  VYGES_LVS=.../vyges-lvs  ./run.sh
set -euo pipefail
cd "$(dirname "$0")"
: "${SKY130_LIB:?set SKY130_LIB}"; : "${NETGEN_SETUP:?set NETGEN_SETUP}"
LVS="${VYGES_LVS:-vyges-lvs}"; NG="${NETGEN:-netgen}"
PORTS="clk rst_n enable count.0 count.1 count.2 count.3 count.4 count.5 count.6 count.7"

yosys -q -p "
  read_verilog counter.v
  synth -top counter -flatten
  dfflibmap -liberty $SKY130_LIB
  abc -liberty $SKY130_LIB
  opt_clean -purge
  write_spice -top counter counter_cells.spice
"
wrap() { { echo ".subckt counter $PORTS"; cat; echo ".ends"; }; }
grep -E '^X' counter_cells.spice | wrap > schematic.spice
grep -E '^X' counter_cells.spice | tac | sed 's/\b_09_\b/ext9/g' | wrap > layout.spice          # rename + reorder
grep -E '^X' counter_cells.spice | grep -v '^X20 ' | wrap > bug_drop.spice                        # dropped cell
grep -E '^X' counter_cells.spice | sed 's/^X8 _07_ _11_ count.6/X8 _07_ count.6 _11_/' | wrap > bug_swap.spice  # swapped net

cells=$(grep -cE '^X' schematic.spice)
printf "\nverdict parity (sky130 counter, %s cells):\n\n" "$cells"
printf "  %-8s %-12s %s\n" case vyges-lvs netgen
for c in "match:layout.spice" "drop:bug_drop.spice" "swap:bug_swap.spice"; do
  label=${c%%:*}; lay=${c##*:}
  printf "layout: %s\nschematic: schematic.spice\ntop: counter\n" "$lay" > case.lvs
  v=$("$LVS" run case.lvs 2>&1 | grep -oE 'MATCH|MISMATCH' | head -1)
  $NG -batch lvs "$lay counter" "schematic.spice counter" "$NETGEN_SETUP" "ng_$label.out" >/dev/null 2>&1 || true
  n=$(grep -iE 'match uniquely|do not match' "ng_$label.out" | tail -1 | sed 's/^[[:space:]]*//')
  printf "  %-8s %-12s %s\n" "$label" "$v" "${n:-?}"
done
echo
echo "PASS if vyges-lvs and netgen agree on every row (MATCH vs 'match uniquely',"
echo "MISMATCH vs 'do not match')."
