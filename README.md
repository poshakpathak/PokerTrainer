# PokerTrainer

A free, self-hosted study tool in the spirit of GTO Wizard, for No-Limit Hold'em cash games.

It has three parts:

1. **Solver engine** (`solver/`): a small Rust CLI built on the open-source
   [postflop-solver](https://github.com/b-inary/postflop-solver). It solves one flop for one
   spot and exports every flop decision (strategy and EV for every combo) as JSON. It also
   has its own 6-max preflop solver (see [Preflop solver](#preflop-solver)).
2. **Solution library** (`spots/`, `flops/`, `scripts/build_library.py`, `web/data/`): spot
   definitions (preflop ranges, pot, stacks, bet sizes) and a script that batch-solves spots ×
   flops into `web/data/`.
3. **Web app** (`web/`): a static site with no build step.
   - **Drill** deals you spots from the solvers, preflop and postflop, and scores every
     decision by the EV it loses against the solution. It can play whole hands against your own
     solve, shows which situations cost you the most, and replays the hands you got wrong (see
     [Drill](#drill)).
   - **Study** is a strategy browser: the 13×13 grid shows each hand class's action mix (or
     its EV or equity), you can step through the action tree, and hovering a cell shows every
     combo's frequencies, EVs and equity. Each node also shows both ranges' equity, EV and
     equity realization.
   - **Reports** compares one decision (e.g. the c-bet after a check) across every solved flop,
     sortable by action frequency and filterable by texture, so you can learn the patterns
     instead of memorizing single boards.
   - **Preflop solver** shows GTO preflop ranges for 6-max cash games: every seat's opens,
     3-bets, 4-bets and calls as 13×13 grids, with the EV of every hand and action. Browse the
     bundled charts, or solve your own stack depth, bet sizes and rake with the local server.
     Any heads-up pot it reaches opens in the Postflop solver with both ranges filled in.
   - **Postflop solver** solves any spot you set up (ranges, board, pot, stacks, bet sizes,
     rake) on your own machine and lets you browse the whole tree: flop, turn and river, on any turn or river
     card, with a report of the strategy on every card. You can also **lock** how a player plays
     at any decision (see [Node locking](#node-locking)) and solve again to find the best
     exploit, side by side with GTO. Needs the local server below.
   - **Ranges** shows the preflop ranges each spot was solved with.

## Run it

With the solver server (everything, including both solvers):

```sh
cd solver
cargo run --release --bin pt-server
# open http://127.0.0.1:8080
```

Options: `--port 8080`, `--threads N`, `--max-memory-gb 8` (solves whose tree needs more memory
than this are refused instead of swapping your machine to death), `--web <path to web/>`.
The server only listens on 127.0.0.1.

Without Rust, everything except solving works from any static file server (the Preflop solver
tab still shows the bundled charts):

```sh
cd web
python3 -m http.server 8000
# open http://localhost:8000
```

Any static file server works (the page fetches `data/*.json`, so opening `index.html`
directly from disk won't). You can also publish the `web/` folder with GitHub Pages.

## Drill

The **Drill** tab deals you decisions and scores each one by the EV it loses against the
solver. Pick where the spots come from:

- **Preflop**: any decision in a Preflop solver chart: opens, facing an open, squeezes, facing
  a 3-bet and facing a 4-bet or more. Pick the chart (or your last preflop solve on pt-server),
  the situations and your seat. **Close decisions** (the default) deals only the hands the
  solver mixes with or that sit at the edge of a range; **Every hand** deals hands as often as
  they reach the spot.
- **Flops**: flop decisions from the solution library: the first decision, facing a bet or
  raise, or anywhere on the flop.
- **Your solve**: whole hands, flop to river, on the spot you solved in the Postflop solver
  tab. The other player is dealt a real hand from their range and plays the solver's strategy
  for it, turn and river cards come at random, and at showdown you see what they had. Pick
  your seat and the streets you decide on; the solver plays your hand on the others. If you've
  also solved with locks, **Play against: Your locks** has the locked player play your locks
  while you're scored against the best exploit. Needs pt-server.

After each decision you get every action's solver frequency, EV and EV lost, how the solver
plays every hand at that point, and a link to the same spot in the Preflop solver, Study or
Postflop solver. Keys 1 to 9 pick an action and Space goes on.

- **Grades.** An action is **Correct** if the solver takes it at least 20% of the time with
  your hand, or it loses at most 0.5% of the pot. Otherwise it's a **Small mistake** (up to 3%
  of the pot), a **Mistake** (up to 10%) or a **Blunder**.
- **Leaks.** The Leaks panel groups your decisions by situation (BB facing an open, flop
  facing a bet, river checked to you, and so on), the costliest first; click one to drill only
  that. **Summary** shows the session's grades, its most expensive decisions and the same
  breakdown.
- **Review mistakes** deals the decisions you got wrong again, until you get them right.
- **From the solver tabs.** **Drill this spot** in the Preflop solver and **Drill this
  decision** in the Postflop solver drill that exact decision; **Play whole hands in the
  Drill** starts hands on the Postflop solver's spot.

If you take an action the solver never takes with your hand, it has no strategy for your hand
after it, so a whole hand goes on as if you'd made the solver's play (the hand log shows
both). A fold always ends the hand. Stats and saved mistakes are kept in your browser.

## Node locking

Node locking answers "what if my opponent doesn't play GTO here?". You fix how one player plays
at a decision, solve again, and the solver finds the best way to exploit it. Use it to turn
reads on real players (overfolds to river bets, never raises the turn, always c-bets) into a
strategy.

1. Solve a spot in the Postflop solver tab and click through to the decision, e.g. BB facing a river bet.
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

## Preflop solver

The **Preflop solver** tab shows a whole 6-max preflop solution: for every seat and every line
(opens, 3-bets, squeezes, 4-bets, all-ins, cold calls), how each of the 169 hand classes plays
and what each action is worth. Pick a chart under **Solution**, then click actions to play the
hand out. The seat strip shows who's still in (click a seat to see its range at that point),
**Jump to** goes straight to every open, every answer to an open and every answer to a 3-bet,
and the **EV** tab colors the grid by each hand's EV instead of its strategy. When the hand
reaches a flop you get every player's range, and a heads-up pot that isn't all-in has
**Open in postflop Solver**, which fills in the Postflop solver's ranges, pot, stacks and rake
(and bet sizes from the matching bundled spot) so you only pick a board.

The bundled charts in `web/data/preflop/` all use 2.5bb opens (3bb from the SB, which raises
or folds), 3-bets to 3x the open in position and 4x out of position (plus one open per caller
when squeezing), 4-bets to 2.2x in position and 2.5x out, and all-in as an option from the
4-bet on:

| Chart | Stacks | Rake |
| --- | --- | --- |
| 100bb, no rake | 100bb | none |
| 100bb, rake | 100bb | 5%, capped at 3bb |
| 50bb, rake | 50bb | 5%, capped at 3bb |
| 200bb, rake | 200bb | 5%, capped at 3bb |

To solve other settings, run pt-server and use **New solve** in the tab, or the command line:

```sh
cd solver
cargo run --release --bin pt-preflop -- --set stack_bb=60 --set rake.rate=0.05 --set rake.cap_bb=3 \
  --out ../web/data/preflop/60bb.json --summary
```

`--set` changes any setting below (`rake.rate` style for nested ones) and `--config file.json`
reads several at once. `--summary` prints every seat's opening range and the answers to it;
add the file to `web/data/preflop/index.json` to list it in the tab. `--iters` and `--target`
(in mbb per hand) say when to stop. The first run builds the all-in equity tables (heads-up and
three-way, about 30 seconds on 4 cores) and caches them in `solver/cache/`; after that a 100bb
solve takes under a minute.

| Setting | Default | Meaning |
| --- | --- | --- |
| `stack_bb` | 100 | everyone's stack, in bb (5 to 400) |
| `open_bb`, `sb_open_bb` | 2.5, 3 | the open size, and from the SB |
| `sb_limp`, `iso_bb` | false, 3.5 | whether the SB may limp, and the BB's raise over a limp |
| `threebet_ip`, `threebet_oop` | 3, 4 | 3-bet size as a multiple of the open, in and out of position |
| `squeeze_per_caller` | 1 | opens added to a 3-bet for each caller |
| `fourbet_ip`, `fourbet_oop`, `fivebet` | 2.2, 2.5, 2.2 | later raises, as a multiple of the raise before |
| `allin_from` | 3 | the raise from which all-in is always an option: 1 the open, 2 the 3-bet, 3 the 4-bet, 4 the 5-bet |
| `allin_spr` | 1 | a raise that would leave less than this stack-to-pot ratio behind is an all-in instead |
| `cold_call_3bet` | false | whether a player who hasn't put money in yet may call a 3-bet |
| `max_flop_players` | 3 | how many players may see a flop (2 or 3) |
| `rake` | none | e.g. `{"rate": 0.05, "cap_bb": 3}`, taken from pots that see a flop |
| `postflop` | `calibrated` | how pots that see a flop are shared (below), or `equity` for raw all-in equity |
| `start_seat` | `UTG` | the first seat to act; the seats before it fold, so `SB` solves blind against blind |

### How it works

- **Tree.** Every player can fold, call or make one raise of the set size at each level, plus
  all-in where it's allowed. Only the SB can limp. At 100bb that's about 2,700 decisions.
- **Solving.** Discounted CFR updates all 169 hand classes of a seat at once, on every core.
  Exploitability, how much the seats would win on average by best-responding to the solution,
  is computed exactly; the bundled charts are at about 0.1 mbb (a ten-thousandth of a bb) per
  hand.
- **Cards.** Your hand removes cards from every other player's range. Between two other
  players hands are treated as independent, except in three-way all-ins, which use exact
  three-way equity.
- **After the flop.** A preflop solver can't play the flop, so a hand's share of the pot when
  it sees one comes from a model fit to this repo's own postflop solves. Per matchup, with
  `x = equity - 1/2`:

  ```text
  share = 1/2 + alpha * x + cubic * x^3 ± position ± initiative + play(you) - play(opponent)
  ```

  Close matchups end up nearer to even than their equity, hands that dominate win more than
  theirs, the player in position gets about 5% of the pot from the one out of position, the
  preflop raiser gets a little more (much more in 3-bet pots), and pairs and suited hands play
  better than offsuit ones. The model is fit separately for single-raised and 3-bet pots and
  slides toward raw equity as the stack-to-pot ratio gets smaller; three-way pots use
  three-way equity with two thirds of the position and playability terms. The numbers are in
  `solver/src/preflop/model.rs`; `scripts/fit_realization.py` refits them from the library and
  the extra spots in `scripts/calibration/` (`run.sh` solves those).

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
- The precomputed library only exports flop decisions. Use the Postflop solver tab to explore
  turns and rivers.
- The library's spot ranges are hand-written approximations of 100bb 6-max ranges, not solver
  output.
- The Preflop solver's charts are only as good as its postflop model: it values each hand from
  a fit to postflop solves instead of playing the flop out, so expect them to differ from
  full-game solvers by a few percent of hands at the edges of ranges. At 100bb without rake
  it opens 16% of hands UTG, 20% HJ, 25% CO, 37% BTN and 40% SB, and the BB defends 56%
  against a BTN open (41% calls, 15% 3-bets); next to published solver charts that's a little
  tight on the BTN and SB. The tree also leaves out some lines: one raise size per level,
  limps only from the SB, and at most three players see a flop.

## Solver API

`pt-server` exposes a small JSON API the two solver tabs use; you can script against it too.

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
| `POST /api/preflop/solve` | `{config, max_iters, target_mbb}`; `config` takes the settings in [Preflop solver](#preflop-solver) | starts a preflop solve (replacing any current one) |
| `GET /api/preflop/status` | | state (`tables` while building the equity tables), iteration, exploitability and its history, time |
| `POST /api/preflop/stop` | | stops early and keeps the current strategy |
| `GET /api/preflop/solution` | | the last preflop solve, in the same format as `web/data/preflop/*.json` |

`history` lists action indexes at decision nodes and card ids (0 to 51, `4 * rank + suit` with
2 = 0 and clubs, diamonds, hearts, spades = 0 to 3) at chance nodes. `solution` is `"gto"` or
`"exploit"` (the last solve without or with locks); it defaults to the latest.

A lock's `targets` are the range-wide action frequencies, one per action; `order` is which
hands change first to reach them: `"ev"` (least EV lost, the default), `"weak"` or `"strong"`
(by equity). `hands` fixes hand classes or combos to a mix, e.g. `{"AKs": [0, 1], "QhJh": [1, 0]}`.
`wild` lists the steps of `history` that are cards to treat as any card, and `keep_path`
also locks the same player's earlier decisions on the line as they are.

## Roadmap

- Rebuild the library's spots from the preflop solver's ranges.
- Calibrate the preflop solver's postflop model on more spots and flops.
- Aggregated turn and river reports across many flops.
- Multiway pots.

## License

The solver crate links postflop-solver, which is AGPL-3.0, so `solver/` is AGPL-3.0 as well.
That's fine for personal use; if you ever run it as a public service, the AGPL applies.
