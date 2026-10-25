# PokerTrainer

A free, self-hosted study tool in the spirit of GTO Wizard, for No-Limit Hold'em cash games.

It has three parts:

1. **Solver engine** (`solver/`): a small Rust CLI built on the open-source
   [postflop-solver](https://github.com/b-inary/postflop-solver). It solves one flop for one
   spot and exports every flop decision (strategy and EV for every combo) as JSON.
2. **Solution library** (`spots/`, `flops/`, `scripts/build_library.py`, `web/data/`): spot
   definitions (preflop ranges, pot, stacks, bet sizes) and a script that batch-solves spots ×
   flops into `web/data/`.
3. **Web app** (`web/`): a static site with no build step.
   - **Trainer** deals you a random hand in a solved spot, you pick an action, and it scores
     you by EV lost against the solver. Stats and recent hands are kept in your browser.
   - **Study** is a strategy browser: the 13×13 grid shows each hand class's action mix,
     you can step through the action tree, and hovering a cell shows every combo's
     frequencies and EVs.
   - **Ranges** shows the preflop ranges each spot was solved with.

## Run it

```sh
cd web
python3 -m http.server 8000
# open http://localhost:8000
```

Any static file server works (the page fetches `data/*.json`, so opening `index.html`
directly from disk won't). You can also publish the `web/` folder with GitHub Pages.

## Solve more spots and flops

You need Rust (https://rustup.rs).

```sh
cd solver && cargo build --release && cd ..
python3 scripts/build_library.py                          # all spots × flops/starter.txt
python3 scripts/build_library.py --spots btn_vs_bb_srp    # one spot
python3 scripts/build_library.py --flops flops/my_list.txt --accuracy 0.5 --iters 400
```

Already-solved flops are skipped, so you can stop the script and resume later. It rewrites
`web/data/index.json` after each flop so the app picks up new solutions on reload.

**How long it takes.** On a 4-core machine with the bundled bet sizes, a single-raised pot
flop takes about 3 to 4 minutes and uses about 2 GB of RAM; a 3-bet pot flop takes under a
minute. A full 18-flop set for one single-raised pot is about an hour. More cores make it
proportionally faster.

### Add your own spot

Copy a file in `spots/` and change it:

- `ranges.oop` / `ranges.ip`: preflop ranges in solver syntax, e.g. `22+,A2s+,KQo,QJs:0.5`
  (`:0.5` means half the combos).
- `pot_bb`, `stack_bb`: pot and effective stack at the start of the flop.
- `bets`: sizes per street and player, as `%` of pot, `e` (geometric), `a` (all-in),
  and raise sizes like `3x`. An empty raise string means no raises on that street.
  Every extra size makes the tree, and the solve time, much bigger. The bundled spots use two
  flop sizes, one turn size and one river size, with raises only on the flop.
- `rake.rate` / `rake.cap_bb` (optional): e.g. `{"rate": 0.05, "cap_bb": 3}`.

Then run the build script with `--spots <your id>`.

### Add flops

Put one flop per line in a text file (e.g. `KsQd4c`) and pass it with `--flops`. Solutions
don't account for suit isomorphism, so `As7d2c` and `Ah7c2d` are separate solves; pick one
representative per texture.

## Accuracy and limits

- The bundled library targets 1% of the pot exploitability. Lower `--accuracy` for
  tighter solutions at the cost of time.
- The trees are simplified (few bet sizes, no turn/river raises) so they solve on a laptop.
  Treat the output as a solid baseline, not the final word in every spot.
- Only flop decisions are exported. Turn and river are solved (they're needed for correct flop
  strategy) but not shown yet.
- The preflop ranges are hand-written approximations of 100bb 6-max ranges, not solver output.

## License

The solver crate links postflop-solver, which is AGPL-3.0, so `solver/` is AGPL-3.0 as well.
That's fine for personal use; if you ever run it as a public service, the AGPL applies.
