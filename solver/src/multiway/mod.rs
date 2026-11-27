//! Solving pots with three players (or two) from the flop, turn or river: a vector-form
//! Discounted CFR solver with exact card removal between every player's hand.

pub mod cfr;
pub mod eval;
pub mod game;
pub mod service;
pub mod spot;
pub mod tree;
pub mod view;
