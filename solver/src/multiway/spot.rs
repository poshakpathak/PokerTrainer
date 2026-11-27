//! A pot to solve with two or three players: names, ranges, pot, stacks, bet sizes and rake.

use crate::{Rake, CHIPS_PER_BB};
use postflop_solver::{BetSize, BetSizeOptions};
use serde::{Deserialize, Serialize};

/// The most players a pot can have.
pub const MAX_PLAYERS: usize = 3;

/// Bet sizes on one street.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StreetSizes {
    /// Each player's bet sizes when nobody has bet yet on the street, in postflop order
    /// (`"33%, 75%"`, `"e"`, `"a"`). An empty string means that player only checks there.
    pub bet: Vec<String>,
    /// Raise sizes for everyone (`"3x"`, `"50%"`, `"a"`). Empty means no raises on the street.
    #[serde(default)]
    pub raise: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Sizes {
    pub flop: StreetSizes,
    pub turn: StreetSizes,
    pub river: StreetSizes,
}

/// A pot with two or three players, in the format of the presets in
/// `web/data/multiway/presets.json`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MultiSpot {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Player names in postflop order: the first one acts first on every street.
    pub players: Vec<String>,
    /// Each player's range in solver syntax, in the same order.
    pub ranges: Vec<String>,
    /// The pot at the start of the first street solved, in bb.
    pub pot_bb: f64,
    /// What each player has behind at the start, in bb (everyone has the same).
    pub stack_bb: f64,
    pub bets: Sizes,
    /// Raises allowed on a street after the first bet. More raises make the tree much bigger.
    #[serde(default = "default_max_raises")]
    pub max_raises: u32,
    #[serde(default)]
    pub rake: Rake,
}

fn default_max_raises() -> u32 {
    1
}

/// A spot turned into chips and parsed bet sizes.
#[derive(Clone, Debug)]
pub struct Config {
    pub players: usize,
    /// Chips in the pot when the solve starts.
    pub starting_pot: i32,
    /// Chips each player has behind when the solve starts.
    pub stack: i32,
    pub rake_rate: f64,
    /// In chips.
    pub rake_cap: f64,
    /// Opening bet sizes by street, then player.
    pub bets: [Vec<Vec<BetSize>>; 3],
    /// Raise sizes by street.
    pub raises: [Vec<BetSize>; 3],
    pub max_raises: u32,
    /// All-in is added as an option when it's at most this many times the pot (as in the
    /// heads-up solver).
    pub add_allin_threshold: f64,
    /// A bet that would leave less than this share of the pot behind becomes all-in.
    pub force_allin_threshold: f64,
    /// Bet sizes closer than this (as a share of the pot) are merged.
    pub merging_threshold: f64,
}

fn parse_sizes(s: &str, raise: bool, what: &str) -> Result<Vec<BetSize>, String> {
    let opts = if raise { BetSizeOptions::try_from(("", s)) } else { BetSizeOptions::try_from((s, "")) }
        .map_err(|e| format!("{what}: {e}"))?;
    let sizes = if raise { opts.raise } else { opts.bet };
    if sizes.iter().any(|b| matches!(b, BetSize::Additive(..))) {
        return Err(format!("{what}: use % of the pot, x (raises), e (geometric) or a (all-in)"));
    }
    if sizes.len() > 4 {
        return Err(format!("{what}: at most 4 sizes"));
    }
    Ok(sizes)
}

impl MultiSpot {
    /// Checks the spot and converts it for the tree builder.
    pub fn config(&self) -> Result<Config, String> {
        let n = self.players.len();
        if !(2..=MAX_PLAYERS).contains(&n) {
            return Err(format!("A spot needs 2 or 3 players, not {n}."));
        }
        if self.ranges.len() != n {
            return Err(format!("There are {n} players but {} ranges.", self.ranges.len()));
        }
        if !(self.pot_bb > 0.0 && self.pot_bb <= 10_000.0) {
            return Err("The pot must be more than 0bb.".into());
        }
        if !(self.stack_bb > 0.0 && self.stack_bb <= 10_000.0) {
            return Err("The stacks must be more than 0bb.".into());
        }
        if !(0.0..1.0).contains(&self.rake.rate) || self.rake.cap_bb < 0.0 {
            return Err("Rake must be 0 to 100% with a cap of 0bb or more.".into());
        }
        if self.max_raises > 4 {
            return Err("At most 4 raises per street.".into());
        }
        let streets = [&self.bets.flop, &self.bets.turn, &self.bets.river];
        let names = ["flop", "turn", "river"];
        let mut bets: [Vec<Vec<BetSize>>; 3] = Default::default();
        let mut raises: [Vec<BetSize>; 3] = Default::default();
        for (s, sizes) in streets.iter().enumerate() {
            if sizes.bet.len() != n {
                return Err(format!("The {} needs bet sizes for each of the {n} players.", names[s]));
            }
            for (p, b) in sizes.bet.iter().enumerate() {
                let who = self.players.get(p).map(String::as_str).unwrap_or("?");
                bets[s].push(parse_sizes(b, false, &format!("{} bets for {who}", names[s]))?);
            }
            raises[s] = parse_sizes(&sizes.raise, true, &format!("{} raises", names[s]))?;
        }
        let rake_cap = if self.rake.rate > 0.0 && self.rake.cap_bb == 0.0 { f64::INFINITY } else { self.rake.cap_bb * CHIPS_PER_BB };
        Ok(Config {
            players: n,
            starting_pot: (self.pot_bb * CHIPS_PER_BB).round() as i32,
            stack: (self.stack_bb * CHIPS_PER_BB).round() as i32,
            rake_rate: self.rake.rate,
            rake_cap,
            bets,
            raises,
            max_raises: self.max_raises,
            add_allin_threshold: 1.5,
            force_allin_threshold: 0.15,
            merging_threshold: 0.1,
        })
    }

    /// Identifies the game tree this spot builds on a board, ignoring names and ids.
    pub fn tree_key(&self, board: &str) -> String {
        let spot = MultiSpot { id: String::new(), name: String::new(), description: String::new(), players: vec![], ..self.clone() };
        format!("{}|{board}", serde_json::to_string(&spot).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn street(bet: &str, raise: &str, n: usize) -> StreetSizes {
        StreetSizes { bet: vec![bet.to_string(); n], raise: raise.into() }
    }

    #[test]
    fn parses_and_checks() {
        let mut spot = MultiSpot {
            id: "x".into(),
            name: String::new(),
            description: String::new(),
            players: vec!["BB".into(), "CO".into(), "BTN".into()],
            ranges: vec!["AA".into(), "KK".into(), "QQ".into()],
            pot_bb: 8.0,
            stack_bb: 97.5,
            bets: Sizes { flop: street("33%", "3x", 3), turn: street("66%", "", 3), river: street("75%, a", "", 3) },
            max_raises: 1,
            rake: Rake { rate: 0.05, cap_bb: 3.0 },
        };
        let cfg = spot.config().unwrap();
        assert_eq!((cfg.players, cfg.starting_pot, cfg.stack), (3, 160, 1950));
        assert_eq!(cfg.bets[2][1], vec![BetSize::PotRelative(0.75), BetSize::AllIn]);
        assert_eq!(cfg.raises[0], vec![BetSize::PrevBetRelative(3.0)]);
        assert!(cfg.raises[1].is_empty());
        assert_eq!(cfg.rake_cap, 60.0);

        spot.ranges.pop();
        assert!(spot.config().is_err());
        spot.ranges.push("QQ".into());
        spot.bets.turn.bet[0] = "10c".into();
        assert!(spot.config().is_err());
    }
}
