//! Turning a solve into something to read: action labels, lines, frequencies, ranges, and the
//! JSON the web app shows.

use super::cards::{combo_counts, range_string, NUM_CLASSES};
use super::cfr::{Exploitability, Solver, Vec169};
use super::tree::{ActKind, Kind, Node, Tree, SEATS};
use serde_json::{json, Value};

const H: usize = NUM_CLASSES;

/// A bb amount without trailing zeros: 2.5, 10, 24.75.
pub fn bb(x: f64) -> String {
    let s = format!("{:.2}", x);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn action_label(node: &Node, i: usize) -> String {
    let a = node.actions[i];
    match a.kind {
        ActKind::Fold => "Fold".into(),
        ActKind::Check => "Check".into(),
        ActKind::Call if node.level == 0 => "Limp".into(),
        ActKind::Call => "Call".into(),
        ActKind::Raise => format!("Raise {}", bb(a.to)),
        ActKind::AllIn => format!("All-in {}", bb(a.to)),
    }
}

/// The actions from the root to a node: (seat, label) per step.
pub fn line(tree: &Tree, id: usize) -> Vec<(usize, String)> {
    let mut node = 0usize;
    let mut out = Vec::new();
    for step in tree.history(id) {
        let n = &tree.nodes[node];
        out.push((n.seat, action_label(n, step)));
        node = n.children[step] as usize;
    }
    out
}

/// The line as text, leaving out the folds: "UTG Raise 2.5, BTN Call".
pub fn line_text(tree: &Tree, id: usize) -> String {
    let steps: Vec<String> = line(tree, id).into_iter().filter(|(_, l)| l != "Fold").map(|(s, l)| format!("{} {l}", SEATS[s])).collect();
    if steps.is_empty() { "Folded to".into() } else { steps.join(", ") }
}

/// How often the player to act takes each action, over the combos that get to the node.
pub fn frequencies(s: &Solver, id: usize) -> Vec<f64> {
    let node = &s.tree.nodes[id];
    let reach = s.reach(id)[node.seat];
    let sigma = s.strategy(id);
    let w = combo_counts();
    let total: f64 = (0..H).map(|a| (w[a] * reach[a]) as f64).sum();
    sigma.iter().map(|sg| (0..H).map(|a| (w[a] * reach[a] * sg[a]) as f64).sum::<f64>() / total.max(1e-12)).collect()
}

/// The share of all combos in a reach vector.
pub fn range_share(reach: &Vec169) -> f64 {
    let w = combo_counts();
    (0..H).map(|a| (w[a] * reach[a]) as f64).sum::<f64>() / 1326.0
}

/// The decision where everyone before `seat` folded (None if it isn't in the tree).
pub fn folded_to(tree: &Tree, seat: usize) -> Option<usize> {
    let mut id = 0usize;
    loop {
        let n = &tree.nodes[id];
        if n.kind != Kind::Decision {
            return None;
        }
        if n.seat == seat {
            return Some(id);
        }
        let fold = n.actions.iter().position(|a| a.kind == ActKind::Fold)?;
        id = n.children[fold] as usize;
    }
}

/// Follows a line given as (seat, action kind) steps, folding every seat in between.
pub fn find(tree: &Tree, steps: &[(usize, ActKind)]) -> Option<usize> {
    let mut id = 0usize;
    for &(seat, kind) in steps {
        loop {
            let n = &tree.nodes[id];
            if n.kind != Kind::Decision {
                return None;
            }
            if n.seat == seat {
                let i = n.actions.iter().position(|a| a.kind == kind)?;
                id = n.children[i] as usize;
                break;
            }
            let fold = n.actions.iter().position(|a| a.kind == ActKind::Fold)?;
            id = n.children[fold] as usize;
        }
    }
    Some(id)
}

/// The whole solution in a compact form the web app can browse without the server: every
/// node, with strategies in thousandths and values in hundredths of a bb, for the hands that
/// get there. Lines that happen less often than `min_line` (the chance that a hand plays out
/// that way) are left out; their nodes are marked `"cut": true`.
pub fn pack_json(s: &Solver, e: &Exploitability, min_line: f64) -> Value {
    let tree = &s.tree;
    let mut nodes = vec![Value::Null; tree.nodes.len()];
    // Walk the tree with each player's reach, so each node's share of hands is known.
    let mut stack = vec![(0usize, [[1.0f32; H]; 6])];
    while let Some((id, reach)) = stack.pop() {
        let node = &tree.nodes[id];
        let line_prob: f64 = (0..6).map(|p| range_share(&reach[p])).product();
        if id != 0 && line_prob < min_line {
            nodes[id] = json!({ "cut": true });
            continue;
        }
        match node.kind {
            Kind::Fold => nodes[id] = json!({ "t": "fold", "s": node.seat }),
            Kind::Showdown => {
                let sd = &tree.showdowns[node.showdown];
                nodes[id] = json!({ "t": "flop", "p": sd.players, "pot": sd.pot, "rake": (sd.rake * 1000.0).round() / 1000.0, "spr": (sd.spr * 100.0).round() / 100.0 });
            }
            Kind::Decision => {
                let q = node.seat;
                let sigma = s.strategy(id);
                let ev = s.action_values(id);
                let live = |a: usize| reach[q][a] > 1e-4;
                let f: Vec<Vec<i32>> = sigma.iter().map(|v| (0..H).map(|a| if live(a) { (v[a] * 1000.0).round() as i32 } else { 0 }).collect()).collect();
                let v: Vec<Vec<i32>> = ev.iter().map(|v| (0..H).map(|a| if live(a) { (v[a] * 100.0).round() as i32 } else { 0 }).collect()).collect();
                let acts: Vec<Value> = node.actions.iter().map(|a| json!([a.kind, a.to])).collect();
                nodes[id] = json!({ "s": q, "a": acts, "c": node.children, "f": f, "v": v });
                for (i, &c) in node.children.iter().enumerate() {
                    let mut next = reach;
                    for a in 0..H {
                        next[q][a] *= sigma[i][a];
                    }
                    stack.push((c as usize, next));
                }
            }
        }
    }
    json!({
        "format": "pt-preflop-1",
        "config": tree.config,
        "iterations": s.iteration,
        "exploitability_mbb": (e.mbb() * 100.0).round() / 100.0,
        "ev": e.ev.map(|x| (x * 1000.0).round() / 1000.0),
        "min_line": min_line,
        "nodes": nodes,
    })
}

/// A plain-text summary of the main spots: every seat's open, and the answers to it.
pub fn summary(s: &Solver) -> String {
    let tree = &s.tree;
    let mut out = String::new();
    let mut add = |id: usize| {
        let node = &tree.nodes[id];
        let freq = frequencies(s, id);
        let reach = s.reach(id)[node.seat];
        let parts: Vec<String> = (0..node.actions.len()).map(|i| format!("{} {:.1}%", action_label(node, i), freq[i] * 100.0)).collect();
        out += &format!("{} to act after {}: {}\n", SEATS[node.seat], line_text(tree, id), parts.join(", "));
        let sigma = s.strategy(id);
        for i in 0..node.actions.len() {
            if node.actions[i].kind == ActKind::Fold || freq[i] < 0.001 {
                continue;
            }
            let mut r = reach;
            for a in 0..H {
                r[a] *= sigma[i][a];
            }
            out += &format!("    {}: {}\n", action_label(node, i), range_string(&r, 0.01));
        }
    };
    for seat in tree.players() {
        if let Some(id) = folded_to(tree, seat) {
            add(id);
        }
    }
    for opener in tree.players() {
        for responder in (opener + 1)..6 {
            if let Some(id) = find(tree, &[(opener, ActKind::Raise), (responder, ActKind::Fold)]) {
                // The node where `responder` faces the open is the parent of its fold.
                let parent = tree.nodes[id].parent as usize;
                if tree.nodes[parent].seat == responder && (responder == 5 || responder == opener + 1 || responder == 3) {
                    add(parent);
                }
            }
        }
    }
    out
}
