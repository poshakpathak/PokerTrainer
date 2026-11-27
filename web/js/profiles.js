// Player types: common kinds of opponents, described by how often they fold, check or call, and
// bet or raise compared with GTO in each street and situation. pt-server locks every decision of
// the player you pick to the type, starting from the GTO solve, and solves for the best way to
// exploit it (see solver/src/profile.rs).

export const STREETS = ["flop", "turn", "river"];
export const FACINGS = [
  ["none", "No bet to call"],
  ["bet", "Facing a bet"],
  ["raise", "Facing a raise"],
];
export const KINDS = [
  ["fold", "Folds"],
  ["passive", "Checks, calls"],
  ["aggressive", "Bets, raises"],
];
/** How far from GTO: every weight is raised to this power. */
export const STRENGTHS = [
  [0.5, "Mild"],
  [1, "Typical"],
  [1.6, "Extreme"],
];

const everyStreet = (rules) => Object.fromEntries(STREETS.map((st) => [st, rules]));

/**
 * Each type's weights per street and situation: {fold, passive, aggressive} scale how often the
 * player takes that kind of action compared with GTO (missing ones are 1), and `bluff` makes the
 * extra bets and raises bluffs with the weakest hands instead of value bets. Situations left out
 * are played like GTO.
 */
export const PROFILES = [
  {
    id: "station",
    name: "Calling station",
    blurb: "Calls far too often, rarely raises, and bets less, almost never as a bluff.",
    rules: {
      flop: { none: { aggressive: 0.55 }, bet: { fold: 0.35, aggressive: 0.35 }, raise: { fold: 0.45, aggressive: 0.35 } },
      turn: { none: { aggressive: 0.5 }, bet: { fold: 0.3, aggressive: 0.35 }, raise: { fold: 0.45, aggressive: 0.35 } },
      river: { none: { aggressive: 0.45 }, bet: { fold: 0.3, aggressive: 0.3 }, raise: { fold: 0.45, aggressive: 0.35 } },
    },
  },
  {
    id: "nit",
    name: "Nit",
    blurb: "Folds too often to bets and raises, and bets and raises only with strong hands.",
    rules: everyStreet({ none: { aggressive: 0.6 }, bet: { fold: 2.2, aggressive: 0.45 }, raise: { fold: 2.5, aggressive: 0.35 } }),
  },
  {
    id: "maniac",
    name: "Maniac",
    blurb: "Bets and raises far too often, mostly as bluffs, and rarely folds.",
    rules: everyStreet({
      none: { aggressive: 2.4, bluff: true },
      bet: { fold: 0.5, aggressive: 2.6, bluff: true },
      raise: { fold: 0.6, aggressive: 2, bluff: true },
    }),
  },
  {
    id: "turnfolder",
    name: "Gives up on the turn",
    blurb: "Calls flop bets too often, then folds too much when you keep betting the turn and river.",
    rules: {
      flop: { bet: { fold: 0.6 } },
      turn: { bet: { fold: 2 }, raise: { fold: 1.8 } },
      river: { bet: { fold: 1.6 }, raise: { fold: 1.6 } },
    },
  },
  {
    id: "riverhonest",
    name: "Rarely bluffs the river",
    blurb: "Plays like GTO until the river, where their bets and raises are almost always value.",
    rules: { river: { none: { aggressive: 0.4 }, bet: { aggressive: 0.25 }, raise: { aggressive: 0.3 } } },
  },
];

export const profileById = (id) => PROFILES.find((p) => p.id === id) || null;

/** The weights of one situation, with 1 for anything left out. */
export function cellOf(profile, street, facing) {
  const c = profile.rules?.[street]?.[facing] || {};
  return { fold: c.fold ?? 1, passive: c.passive ?? 1, aggressive: c.aggressive ?? 1, bluff: !!c.bluff };
}

/** The rule list pt-server takes: one rule per street and situation that isn't GTO. */
export function rulesFor(profile) {
  const out = [];
  for (const street of STREETS) {
    for (const [facing] of FACINGS) {
      const c = cellOf(profile, street, facing);
      if (c.fold !== 1 || c.passive !== 1 || c.aggressive !== 1) out.push({ street, facing, ...c });
    }
  }
  return out;
}

/** Back from pt-server's rule list to per-street rules. */
export function rulesFromList(list) {
  const rules = {};
  for (const r of list || []) {
    rules[r.street] ??= {};
    rules[r.street][r.facing] = { fold: r.fold, passive: r.passive, aggressive: r.aggressive, bluff: !!r.bluff };
  }
  return rules;
}

/** The body for POST /api/profile. */
export function profileRequest(profile, player, strength) {
  return { player, profile: { id: profile.id, name: profile.name, strength, rules: rulesFor(profile) } };
}

export function strengthName(strength) {
  const near = STRENGTHS.reduce((best, s) => (Math.abs(s[0] - strength) < Math.abs(best[0] - strength) ? s : best));
  return Math.abs(near[0] - strength) < 0.01 ? near[1] : `strength ${strength}`;
}

/** "a calling station", "a nit": the type as it reads in a sentence. */
export function typeNoun(profile) {
  const name = profile?.name || "player type";
  const lower = /^[A-Z][a-z]/.test(name) && !/^[A-Z]{2}/.test(name) ? name[0].toLowerCase() + name.slice(1) : name;
  // Types named by what they do ("Gives up on the turn") read as "a player who …".
  if (/^(gives|rarely|never|always|calls|folds|bets|plays|floats)\b/i.test(name)) return `a player who ${lower}`;
  return `${/^[aeiou]/i.test(lower) ? "an" : "a"} ${lower}`;
}

/** "×0.35", or "" for 1. */
export function weightText(w) {
  if (Math.abs(w - 1) < 1e-9) return "";
  return `×${+w.toFixed(2)}`;
}
