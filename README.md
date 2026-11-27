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
   - **Postflop trainer** deals you a random hand in a solved spot, you pick an action, and it
     scores you by EV lost against the solver. Hands you get wrong are saved, and
     **Review mistakes** mode replays them until you answer them correctly. Stats are kept in
     your browser.
   - **Preflop** drills open-or-fold decisions from every seat against the opening charts in
     `web/data/preflop.json`, focusing on hands near the edge of each range.
   - **Study** is a strategy browser: the 13×13 grid shows each hand class's action mix (or
     its EV or equity), you can step through the action tree, and hovering a cell shows every
     combo's frequencies, EVs and equity. Each node also shows both ranges' equity, EV and
     equity realization.
   - **Reports** compares one decision (e.g. the c-bet after a check) across every solved flop,
     sortable by action frequency and filterable by texture, so you can learn the patterns
     instead of memorizing single boards.
   - **Solver** solves any spot you set up (ranges, board, pot, stacks, bet sizes, rake) on your
     own machine and lets you browse the whole tree: flop, turn and river, on any turn or river
     card, with a report of the strategy on every card. You can also **lock** how a player plays
     at any decision (see [Node locking](#node-locking)) and solve again to find the best
     exploit, side by side with GTO. Needs the local server below.
   - **Ranges** shows the preflop ranges each spot was solved with.

## Run it

With the solver server (everything, including the Solver tab):

```sh
cd solver
cargo run --release --bin pt-server
# open http://127.0.0.1:8080
```

Options: `--port 8080`, `--threads N`, `--max-memory-gb 8` (solves whose tree needs more memory
than this are refused instead of swapping your machine to death), `--web <path to web/>`.
The server only listens on 127.0.0.1.

Without Rust, everything except the Solver tab works from any static file server:

```sh
cd web
python3 -m http.server 8000
# open http://localhost:8000
```

Any static file server works (the page fetches `data/*.json`, so opening `index.html`
directly from disk won't). You can also publish the `web/` folder with GitHub Pages.

## Node locking

Node locking answers "what if my opponent doesn't play GTO here?". You fix how one player plays
at a decision, solve again, and the solver finds the best way to exploit it. Use it to turn
reads on real players (overfolds to river bets, never raises the turn, always c-bets) into a
strategy.

1. Solve a spot in the Solver tab and click through to the decision, e.g. BB facing a river bet.
2. Click **Lock BB's play here** and set how often BB takes each action, e.g. fold 70%. You
   pick which hands switch first: the ones that lose the least EV doing so (the default),
   the weakest, or the strongest. You can also **paint** hands in the grid to fix them to one
   action, e.g. "always calls with top pair"; the frequencies are then met with the rest of the
   range. The grid previews the locked strategy as you go.
3. On the turn or river, tick **On every river card** to lock the same decision on every card
   that can come, not just the one shown ("BB overfolds rivers" rather than "BB overfolds the
   2d river").
4. **Keep BB's earlier decisions on this line as they are now** is on by default. A real player doesn't
   change their flop and turn play to make up for a river leak, but a solver would, which
   would also change which hands get to the locked spot. Turn it off to let the solver adjust
   them.
5. Save the lock, add more if you like, and click **Solve with this lock**.

The solve with locks is kept next to the GTO solve, so you can switch between **Exploit** and
**GTO** at any node. The locks panel shows how much EV each player gains or loses over the whole
hand, every action shows its frequency in the other solution, and the hand detail adds the
other solution's row for the hovered hand class. Locks belong to the spot and board they were
made on: solving a different spot clears them, and they're kept in the server's memory, so
restarting pt-server clears them too. Both solutions have to fit under `--max-memory-gb`.

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
- The precomputed library only exports flop decisions. Use the Solver tab to explore turns
  and rivers.
- The preflop ranges (both the spot ranges and the opening charts) are hand-written
  approximations of 100bb 6-max ranges, not solver output.

## Solver API

`pt-server` exposes a small JSON API the Solver tab uses; you can script against it too.

| Request | Body | Returns |
| --- | --- | --- |
| `POST /api/solve` | `{spot, board, accuracy, max_iters, locks}`; `spot` has the same shape as `spots/*.json`; `locks: true` applies the saved locks | starts a solve (replacing any current one) |
| `GET /api/status` | | state, iteration, exploitability, time, memory, the finished solutions and the saved locks |
| `POST /api/stop` | | stops early and keeps the current strategy |
| `POST /api/node` | `{history, solution}` | the node after `history`: a decision (strategy, EV and equity per combo, and the same decision in the other solution), a chance node (a summary for every card that can come), or terminal |
| `POST /api/lock/preview` | `{history, solution, targets, hands, order}` | the strategy per combo a lock would set, and the frequencies before and after |
| `POST /api/locks/add` | `{history, solution, targets, hands, order, wild, keep_path, label}` | saves a lock (replacing one on the same decision) and returns all locks |
| `POST /api/locks/remove` | `{id}` | removes a lock |
| `POST /api/locks/clear` | | removes every lock |

`history` lists action indexes at decision nodes and card ids (0 to 51, `4 * rank + suit` with
2 = 0 and clubs, diamonds, hearts, spades = 0 to 3) at chance nodes. `solution` is `"gto"` or
`"exploit"` (the last solve without or with locks); it defaults to the latest.

A lock's `targets` are the range-wide action frequencies, one per action; `order` is which
hands change first to reach them: `"ev"` (least EV lost, the default), `"weak"` or `"strong"`
(by equity). `hands` fixes hand classes or combos to a mix, e.g. `{"AKs": [0, 1], "QhJh": [1, 0]}`.
`wild` lists the steps of `history` that are cards to treat as any card, and `keep_path`
also locks the same player's earlier decisions on the line as they are.

## Roadmap

- Preflop solver, so the ranges going into each spot come from a solve instead of charts.
- Aggregated turn and river reports across many flops.
- Multiway pots.

## License

The solver crate links postflop-solver, which is AGPL-3.0, so `solver/` is AGPL-3.0 as well.
That's fine for personal use; if you ever run it as a public service, the AGPL applies.
