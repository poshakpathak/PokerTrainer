#!/usr/bin/env bash
# Solves the calibration spots on a set of flops with pt-solver, for fit_realization.py.
# Usage: scripts/calibration/run.sh OUT_DIR   (from the repository root, after building solver/)
set -euo pipefail
out=${1:?usage: run.sh OUT_DIR}
here=$(dirname "$0")
bin=solver/target/release/pt-solver
flops="AhKc5d Ks7d2c Qh9c4d Jd8c3s Td9d6h 8c7c5d 6s4d2h 7h7c2d Ah8h3h"
for spot in btn_vs_co_3bp co_vs_btn_srp sb_vs_bb_srp; do
  mkdir -p "$out/$spot"
  for flop in $flops; do
    [ -s "$out/$spot/$flop.json" ] && continue
    "$bin" --spot "$here/$spot.json" --flop "$flop" --out "$out/$spot/$flop.json" --accuracy 1.0
  done
done
