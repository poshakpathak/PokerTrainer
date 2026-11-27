#!/usr/bin/env python3
"""Fits the preflop solver's postflop model (solver/src/preflop/model.rs) to postflop solves.

A preflop solve can't play the flop, so it needs each hand's share of the pot when it sees one.
The model gives the player out of position, holding hand class a against b, this share:

    share(a, b) = 1/2 + alpha * x + cubic * x^3 - D + play(a) - play(b),   x = equity(a, b) - 1/2

and the player in position the rest. D is the in-position player's edge (position, plus or minus
initiative for the preflop raiser) and play() scores playability from four features: suited,
pair, straight potential and kicker height.

For every solved spot, each hand class's average share of the flop pot (its EV after the flop
divided by the pot) is taken over the solved flops. Against the opponent's preflop range, with
card removal, the model predicts that share from the averages of x and x^3 over the opponent's
hands and from the playability difference, so alpha, cubic and the feature weights are fit by
weighted least squares per pot type (single-raised and 3-bet pots), with one constant per spot
and side. Each spot's two constants give its D, and spots where the preflop raiser is in
position against spots where the caller is split D into position and initiative.

Usage:
    python3 scripts/fit_realization.py                       # fit the saved per-class data
    python3 scripts/fit_realization.py --extract CALIB_DIR    # re-extract it from solves first

--extract reads the library in web/data/ and the calibration solves made by
scripts/calibration/run.sh CALIB_DIR, and saves scripts/calibration/realization_data.json.
The fit reads the preflop equity table from solver/cache/preflop-equity.bin, which pt-preflop
and pt-server build the first time they run. Needs numpy.
"""

import argparse
import collections
import glob
import itertools
import json
import os

import numpy as np

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "scripts", "calibration", "realization_data.json")
EQUITY = os.path.join(ROOT, "solver", "cache", "preflop-equity.bin")
RANKS = "AKQJT98765432"
FEATURES = ["suited", "pair", "straights", "kicker"]

# Every spot used: its solutions folder (relative to the library or the calibration folder),
# its definition, the pot type, and whether the preflop raiser is in position.
SPOTS = {
    "btn_vs_bb_srp": ("library", "spots/btn_vs_bb_srp.json", "srp", True),
    "co_vs_bb_srp": ("library", "spots/co_vs_bb_srp.json", "srp", True),
    "co_vs_btn_srp": ("calibration", "scripts/calibration/co_vs_btn_srp.json", "srp", False),
    "sb_vs_bb_srp": ("calibration", "scripts/calibration/sb_vs_bb_srp.json", "srp", False),
    "bb_vs_btn_3bp": ("library", "spots/bb_vs_btn_3bp.json", "3bp", False),
    "btn_vs_co_3bp": ("calibration", "scripts/calibration/btn_vs_co_3bp.json", "3bp", True),
}


# Hand classes in the solver's order: row * 13 + col of the grid, suited above the diagonal.
def class_name(i):
    r, c = divmod(i, 13)
    if r == c:
        return RANKS[r] * 2
    return RANKS[min(r, c)] + RANKS[max(r, c)] + ("s" if r < c else "o")


CLASSES = [class_name(i) for i in range(169)]
CLASS_INDEX = {c: i for i, c in enumerate(CLASSES)}


def class_combos(i):
    r, c = divmod(i, 13)
    if r == c:
        return [((r, s1), (r, s2)) for s1, s2 in itertools.combinations(range(4), 2)]
    hi, lo = min(r, c), max(r, c)
    if r < c:
        return [((hi, s), (lo, s)) for s in range(4)]
    return [((hi, s1), (lo, s2)) for s1 in range(4) for s2 in range(4) if s1 != s2]


def hand_class(combo):
    r1, s1, r2, s2 = combo
    if RANKS.index(r1) > RANKS.index(r2):
        r1, s1, r2, s2 = r2, s2, r1, s1
    if r1 == r2:
        return r1 + r2
    return r1 + r2 + ("s" if s1 == s2 else "o")


def features(cls):
    """Suited, pair, straight potential (share of the 4 most), kicker height (0 deuce, 1 ace)."""
    hi, lo = RANKS.index(cls[0]), RANKS.index(cls[1])
    pair = cls[0] == cls[1]
    suited = cls.endswith("s")
    straights = 0
    if not pair:
        r1, r2 = 14 - hi, 14 - lo  # ace = 14
        for low in range(1, 11):
            ranks = set(range(low, low + 5))
            has = lambda r: r in ranks or (r == 14 and 1 in ranks)
            if has(r1) and has(r2):
                straights += 1
    return np.array([float(suited), float(pair), straights / 4.0, (12 - lo) / 12.0])


def parse_range(text):
    """Weight per class of a range in solver syntax: "22+,A2s+,KQo,QJs:0.5,T9s-T7s"."""
    w = np.zeros(169)
    for tok in filter(None, (t.strip() for t in text.split(","))):
        weight = 1.0
        if ":" in tok:
            tok, x = tok.split(":")
            weight = float(x)
        if "-" in tok:
            a, b = tok.split("-")
            if a[0] == a[1]:
                lo, hi = sorted([RANKS.index(a[0]), RANKS.index(b[0])])
                names = [RANKS[k] * 2 for k in range(lo, hi + 1)]
            else:
                lo, hi = sorted([RANKS.index(a[1]), RANKS.index(b[1])])
                names = [a[0] + RANKS[k] + a[2] for k in range(lo, hi + 1)]
        elif tok.endswith("+"):
            t = tok[:-1]
            if t[0] == t[1]:
                names = [RANKS[k] * 2 for k in range(RANKS.index(t[0]) + 1)]
            else:
                names = [t[0] + RANKS[k] + t[2] for k in range(RANKS.index(t[0]) + 1, RANKS.index(t[1]) + 1)]
        else:
            names = [tok]
        for n in names:
            w[CLASS_INDEX[n]] = weight
    return w


def extract_spot(folder):
    """Per class and side: summed reach * share, reach * equity, and reach over the flops."""
    acc = {"oop": collections.defaultdict(lambda: np.zeros(3)), "ip": collections.defaultdict(lambda: np.zeros(3))}
    files = sorted(glob.glob(os.path.join(folder, "*.json")))
    for path in files:
        d = json.load(open(path))
        nodes = {tuple(n["path"]): n for n in d["nodes"]}
        root = nodes[()]
        pot = root["pot_bb"]
        k = len(root["actions"])
        freq = np.zeros(k)
        total = 0.0
        for combo, row in zip(d["hands"]["oop"], root["hands"]):
            if not row:
                continue
            reach, strat, ev, eq = row[0], row[1:1 + k], row[1 + k:1 + 2 * k], row[1 + 2 * k]
            value = sum(s * e for s, e in zip(strat, ev))
            acc["oop"][hand_class(combo)] += np.array([reach * value / pot, reach * eq, reach])
            freq += reach * np.array(strat)
            total += reach
        freq /= total
        # In position: its value after each of OOP's first actions, weighted by how often OOP
        # takes them.
        labels = []
        for a in root["actions"]:
            t = a["type"]
            labels.append("Check" if t == "check" else (f"Bet {a['pct']}%" if t == "bet" else "All-in"))
        value, equity, reach_ip = collections.defaultdict(float), collections.defaultdict(float), {}
        for ai, label in enumerate(labels):
            node = nodes[(label,)]
            kk = len(node["actions"])
            for combo, row in zip(d["hands"]["ip"], node["hands"]):
                if not row:
                    continue
                strat, ev = row[1:1 + kk], row[1 + kk:1 + 2 * kk]
                value[combo] += freq[ai] * sum(s * e for s, e in zip(strat, ev))
                equity[combo] += freq[ai] * row[1 + 2 * kk]
                reach_ip[combo] = row[0]
        for combo, reach in reach_ip.items():
            acc["ip"][hand_class(combo)] += np.array([reach * value[combo] / pot, reach * equity[combo], reach])
    out = {}
    for side in ("oop", "ip"):
        out[side] = [[c, v[0] / v[2], v[1] / v[2], v[2]] for c, v in sorted(acc[side].items()) if v[2] > 1e-9]
    return out, len(files)


def extract(calib_dir):
    data = {}
    for spot, (where, definition, _, _) in SPOTS.items():
        folder = os.path.join(ROOT, "web", "data", spot) if where == "library" else os.path.join(calib_dir, spot)
        rows, n = extract_spot(folder)
        d = json.load(open(os.path.join(ROOT, definition)))
        data[spot] = {"flops": n, "pot_bb": d["pot_bb"], "stack_bb": d["stack_bb"], "ranges": d["ranges"], **rows}
        print(f"{spot}: {n} flops, {len(rows['oop'])} + {len(rows['ip'])} classes")
    json.dump(data, open(DATA, "w"), indent=1)
    print(f"saved {os.path.relpath(DATA, ROOT)}")
    return data


def load_equity():
    """Heads-up equity between classes, from the preflop solver's cache."""
    if not os.path.exists(EQUITY):
        raise SystemExit(f"{os.path.relpath(EQUITY, ROOT)} is missing: run pt-preflop once to build it.")
    raw = np.fromfile(EQUITY, dtype="<f4", count=4 + 169 * 169)
    return raw[4:].reshape(169, 169).astype(np.float64)


def removal():
    """removal[a, b]: how many combos of class b are left, on average, given a combo of a."""
    sets = [[set(c) for c in class_combos(i)] for i in range(169)]
    out = np.zeros((169, 169))
    for a in range(169):
        for b in range(169):
            out[a, b] = sum(1 for x in sets[a] for y in sets[b] if not x & y) / len(sets[a])
    return out


def fit(data, pot_type, equity, avail):
    """Pooled fit of one pot type: alpha, cubic, the feature weights, R2 and per spot its D."""
    spots = [s for s, v in SPOTS.items() if v[2] == pot_type and s in data]
    sides = [(s, side) for s in spots for side in ("oop", "ip")]
    feats = np.array([features(c) for c in CLASSES])
    x = equity - 0.5
    X, y, w = [], [], []
    for j, (s, side) in enumerate(sides):
        other = "ip" if side == "oop" else "oop"
        m = avail * parse_range(data[s]["ranges"][other])[None, :]
        mass = m.sum(1)
        x1, x3 = (m * x).sum(1) / mass, (m * x ** 3).sum(1) / mass
        f_opp = (m @ feats) / mass[:, None]
        for cls, share, _, reach in data[s][side]:
            a = CLASS_INDEX[cls]
            X.append([x1[a], x3[a], *(feats[a] - f_opp[a]), *[1.0 if i == j else 0.0 for i in range(len(sides))]])
            y.append(share)
            w.append(reach)
    X, y, w = np.array(X), np.array(y), np.array(w)
    sw = np.sqrt(w)
    beta, *_ = np.linalg.lstsq(X * sw[:, None], y * sw, rcond=None)
    pred = X @ beta
    r2 = 1 - np.sum(w * (y - pred) ** 2) / np.sum(w * (y - np.average(y, weights=w)) ** 2)
    consts = dict(zip(sides, beta[6:]))
    # The OOP side's constant is 1/2 - D and the IP side's 1/2 + D.
    result = {s: ((consts[(s, "ip")] - consts[(s, "oop")]) / 2, data[s]["stack_bb"] / data[s]["pot_bb"],
                  consts[(s, "ip")] + consts[(s, "oop")]) for s in spots}
    return beta[0], beta[1], beta[2:6], r2, result


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--extract", metavar="CALIB_DIR", help="re-extract the per-class data from solves first")
    args = ap.parse_args()
    data = extract(args.extract) if args.extract else json.load(open(DATA))
    equity, avail = load_equity(), removal()
    rust = []
    for pot_type, name in (("srp", "SINGLE_RAISED"), ("3bp", "THREE_BET")):
        alpha, cubic, gamma, r2, spots = fit(data, pot_type, equity, avail)
        print(f"\n{pot_type}: alpha {alpha:.3f}, cubic {cubic:.3f}, play weights {np.round(gamma, 3).tolist()}"
              f" ({', '.join(FEATURES)}), R2 {r2:.3f}")
        raiser_ip, caller_ip, sprs = [], [], []
        for s, (d, spr, total) in spots.items():
            print(f"  {s}: in-position edge {d:+.4f} (SPR {spr:.1f}, preflop raiser"
                  f" {'in' if SPOTS[s][3] else 'out of'} position; constants sum to {total:.3f})")
            (raiser_ip if SPOTS[s][3] else caller_ip).append(d)
            sprs.append(spr)
        a, c = np.mean(raiser_ip), np.mean(caller_ip)
        position, initiative = (a + c) / 2, (a - c) / 2
        print(f"  position {position:+.4f}, initiative {initiative:+.4f} (mean SPR {np.mean(sprs):.2f})")
        play = ", ".join(f"{g:.3f}" for g in gamma)
        rust.append(f"const {name}: (f64, Params) = (\n    {np.mean(sprs):.2f},\n    Params {{ alpha: {alpha:.3f},"
                    f" cubic: {cubic:.3f}, position: {position:.4f}, initiative: {initiative:.4f}, play: [{play}] }},\n);")
    print("\nFor solver/src/preflop/model.rs:\n\n" + "\n\n".join(rust))


if __name__ == "__main__":
    main()
