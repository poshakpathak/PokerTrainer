#!/usr/bin/env python3
"""Batch-solve spots x flops into web/data/ and refresh web/data/index.json.

Examples:
  python3 scripts/build_library.py                      # every spot, flops/starter.txt
  python3 scripts/build_library.py --spots btn_vs_bb_srp --flops flops/starter.txt
  python3 scripts/build_library.py --index-only         # just rebuild index.json

Already-solved flops are skipped, so you can stop it and start it again at any time.
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPOTS_DIR = ROOT / "spots"
DATA_DIR = ROOT / "web" / "data"
SOLVER = ROOT / "solver" / "target" / "release" / "pt-solver"


def read_flops(path):
    flops = []
    for line in Path(path).read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            flops.append(line)
    return flops


def load_spots(ids):
    spots = {}
    for f in sorted(SPOTS_DIR.glob("*.json")):
        spot = json.loads(f.read_text())
        if not ids or spot["id"] in ids:
            spots[spot["id"]] = (f, spot)
    missing = set(ids or []) - set(spots)
    if missing:
        sys.exit(f"unknown spot(s): {', '.join(sorted(missing))}")
    return spots


REPORT_NODES = {"": "oop_first", "Check": "ip_vs_check"}


def summarize(sol):
    """Range-wide action frequencies at the first decisions, for the Reports view."""
    report = {}
    for node in sol["nodes"]:
        key = REPORT_NODES.get(" / ".join(node["path"]))
        if key is None:
            continue
        n = len(node["actions"])
        freq = [0.0] * n
        total = 0.0
        for row in node["hands"]:
            if not row:
                continue
            reach = row[0]
            total += reach
            for a in range(n):
                freq[a] += row[1 + a] * reach
        report[key] = {
            "actions": node["actions"],
            "freq": [round(f / total, 4) if total else 0 for f in freq],
            "equity": node.get("equity"),
            "ev_bb": node.get("ev_bb"),
        }
    return report


def write_index():
    spots = []
    for f in sorted(SPOTS_DIR.glob("*.json")):
        spot = json.loads(f.read_text())
        solved_dir = DATA_DIR / spot["id"]
        flops = []
        if solved_dir.is_dir():
            for sol in sorted(solved_dir.glob("*.json")):
                meta = json.loads(sol.read_text())
                flops.append({
                    "flop": meta["flop"],
                    "exploitability_pct": meta["exploitability_pct"],
                    "report": summarize(meta),
                })
        spots.append({
            "id": spot["id"],
            "name": spot["name"],
            "description": spot.get("description", ""),
            "oop": spot["oop"],
            "ip": spot["ip"],
            "pot_bb": spot["pot_bb"],
            "stack_bb": spot["stack_bb"],
            "ranges": spot["ranges"],
            "flops": flops,
        })
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    (DATA_DIR / "index.json").write_text(json.dumps({"spots": spots}, indent=1) + "\n")
    total = sum(len(s["flops"]) for s in spots)
    print(f"index.json: {len(spots)} spots, {total} solved flops")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--spots", nargs="*", help="spot ids (default: all in spots/)")
    ap.add_argument("--flops", default=str(ROOT / "flops" / "starter.txt"))
    ap.add_argument("--iters", type=int, default=200)
    ap.add_argument("--accuracy", type=float, default=1.0, help="target exploitability, %% of pot")
    ap.add_argument("--threads", type=int)
    ap.add_argument("--force", action="store_true", help="re-solve flops that already exist")
    ap.add_argument("--index-only", action="store_true")
    args = ap.parse_args()

    if not args.index_only:
        if not SOLVER.exists():
            sys.exit("solver not built: run `cargo build --release` in solver/")
        flops = read_flops(args.flops)
        spots = load_spots(args.spots)
        jobs = [(sid, path, flop) for sid, (path, _) in spots.items() for flop in flops]
        for i, (sid, path, flop) in enumerate(jobs, 1):
            out = DATA_DIR / sid / f"{flop}.json"
            if out.exists() and not args.force:
                continue
            print(f"[{i}/{len(jobs)}] {sid} {flop}", flush=True)
            cmd = [str(SOLVER), "--spot", str(path), "--flop", flop, "--out", str(out),
                   "--iters", str(args.iters), "--accuracy", str(args.accuracy)]
            if args.threads:
                cmd += ["--threads", str(args.threads)]
            t0 = time.time()
            if subprocess.run(cmd).returncode != 0:
                print(f"  failed: {sid} {flop}", file=sys.stderr)
                continue
            print(f"  done in {time.time() - t0:.0f}s", flush=True)
            write_index()
    write_index()


if __name__ == "__main__":
    main()
