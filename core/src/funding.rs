//! Where insiders got their SOL before the launch.
//!
//! For each insider we read its transactions before the launch and find the
//! most recent SOL it received. A funder that itself got SOL minutes earlier
//! and passed it straight on is a throwaway, and we follow it one hop back.
//! Exchange hot wallets are recognised from `labels`.

use crate::labels;
use crate::tx::Tx;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Ignore incoming SOL below this: rent top-ups and dust.
pub const MIN_FUNDING_LAMPORTS: i64 = 10_000_000;
/// A funder that received SOL this soon before forwarding it is a pass-through.
pub const PASS_THROUGH_SECS: i64 = 30 * 60;
/// Wallets funded within this window of each other were funded together.
pub const TOGETHER_SECS: i64 = 5 * 60;
/// Idle this long before funding, then funded just before launch: woken up.
pub const WOKEN_IDLE_DAYS: f64 = 30.0;
pub const WOKEN_FUNDED_WITHIN_SECS: i64 = 24 * 3600;
/// Funded at least this long before launch counts as prepared in advance.
pub const AGED_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Funding {
    pub funder: String,
    pub funder_label: Option<String>,
    pub amount: f64,
    pub time: i64,
    pub seconds_before_launch: i64,
    /// Where the SOL came from before any pass-through wallets.
    pub root: Option<String>,
    pub root_label: Option<String>,
    pub hops: u32,
    /// Days the wallet sat idle before this funding; `None` if it had no
    /// earlier activity in what we read (a fresh wallet).
    pub idle_days: Option<f64>,
}

/// One incoming SOL transfer.
#[derive(Debug, Clone, PartialEq)]
pub struct Incoming {
    pub from: String,
    pub lamports: i64,
    pub time: i64,
    pub sig: String,
}

/// SOL `wallet` received in `txs` from another wallet that signed and paid. Or from a privacy pool (`labels::Kind::Privacy`)
/// that paid it out: its withdrawal is signed by a relayer that pays only the fee (Privacy Cash, `4AV2…vfGh`), and
/// naming the relayer gave every wallet it withdrew to the same funder. Only a known pool: an unknown one, followed back
/// as a pass-through, would lead to whoever deposited last.
pub fn incoming(wallet: &str, txs: &[Tx]) -> Vec<Incoming> {
    let mut out = vec![];
    for tx in txs.iter().filter(|t| t.succeeded()) {
        let signers = tx.signers();
        if signers.contains(&wallet) {
            continue;
        }
        let deltas = tx.sol_deltas();
        let Some(&(_, got)) = deltas.iter().find(|(k, _)| *k == wallet) else {
            continue;
        };
        if got < MIN_FUNDING_LAMPORTS {
            continue;
        }
        let pool = deltas
            .iter()
            .find(|(k, d)| -d >= got / 2 && labels::label(k).is_some_and(|l| l.kind == labels::Kind::Privacy));
        let from = pool
            .or_else(|| deltas.iter().filter(|(k, d)| signers.contains(k) && *d < 0).min_by_key(|(_, d)| *d))
            .map(|(k, _)| k.to_string());
        if let (Some(from), Some(time)) = (from, tx.block_time) {
            out.push(Incoming {
                from,
                lamports: got,
                time,
                sig: tx.signature().to_string(),
            });
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.time));
    out
}

/// SOL `wallet` sent in `txs`: to each wallet that got `MIN_FUNDING_LAMPORTS` or more in a transaction `wallet` signed
/// and paid the most in, as `incoming` reads it from the other side (`Incoming::from` is the receiver). Oldest first.
pub fn outgoing(wallet: &str, txs: &[Tx]) -> Vec<Incoming> {
    let mut out = vec![];
    for tx in txs.iter().filter(|t| t.succeeded()) {
        let signers = tx.signers();
        let deltas = tx.sol_deltas();
        let payer = deltas.iter().filter(|(k, d)| signers.contains(k) && *d < 0).min_by_key(|(_, d)| *d).map(|(k, _)| *k);
        let (Some(payer), Some(time)) = (payer, tx.block_time) else { continue };
        if payer != wallet {
            continue;
        }
        for (to, got) in deltas.iter().filter(|(k, d)| !signers.contains(k) && *d >= MIN_FUNDING_LAMPORTS) {
            out.push(Incoming { from: to.to_string(), lamports: *got, time, sig: tx.signature().to_string() });
        }
    }
    out.sort_by_key(|i| i.time);
    out
}

/// The latest funding of `wallet` at or before `cutoff`, plus how long the
/// wallet sat idle before it.
pub fn last_funding(wallet: &str, txs: &[Tx], cutoff: i64, launch: i64) -> Option<Funding> {
    let inc = incoming(wallet, txs);
    let f = inc.iter().find(|i| i.time <= cutoff)?;
    let earlier = txs.iter().filter_map(|t| t.block_time).filter(|t| *t < f.time).max();
    let label = labels::label(&f.from).map(|l| l.name);
    Some(Funding {
        funder: f.from.clone(),
        funder_label: label.clone(),
        amount: f.lamports as f64 / 1e9,
        time: f.time,
        seconds_before_launch: launch - f.time,
        root: None,
        root_label: None,
        hops: 0,
        idle_days: earlier.map(|e| (f.time - e) as f64 / 86_400.0),
    })
}

/// If `funder` received SOL shortly before passing it on at `forwarded_at`,
/// returns who funded it: the next hop back.
pub fn pass_through(funder: &str, funder_txs: &[Tx], forwarded_at: i64) -> Option<Incoming> {
    incoming(funder, funder_txs)
        .into_iter()
        .find(|i| i.time <= forwarded_at && forwarded_at - i.time <= PASS_THROUGH_SECS)
}

#[derive(Debug, Clone, Serialize)]
pub struct FundingSignal {
    pub code: &'static str,
    pub weight: u32,
    pub params: BTreeMap<&'static str, f64>,
}

fn sig(code: &'static str, weight: u32, params: &[(&'static str, f64)]) -> FundingSignal {
    FundingSignal {
        code,
        weight,
        params: params.iter().copied().collect(),
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Red flags from how the insiders were funded. `funding` holds insiders only.
pub fn signals(funding: &HashMap<String, Funding>) -> Vec<FundingSignal> {
    let mut out = vec![];
    let mut fs: Vec<&Funding> = funding.values().collect();
    fs.sort_by_key(|f| f.time);

    // Largest group funded within TOGETHER_SECS of each other.
    let mut best: &[&Funding] = &[];
    for i in 0..fs.len() {
        let j = fs[i..]
            .iter()
            .take_while(|f| f.time - fs[i].time <= TOGETHER_SECS)
            .count();
        if j > best.len() {
            best = &fs[i..i + j];
        }
    }
    if best.len() >= 2 {
        let minutes = median(best.iter().map(|f| f.seconds_before_launch as f64 / 60.0).collect());
        let spread = (best.last().unwrap().time - best[0].time) as f64;
        let sources = best
            .iter()
            .map(|f| f.root.as_deref().unwrap_or(&f.funder))
            .collect::<std::collections::HashSet<_>>()
            .len();
        let hops: u32 = best.iter().map(|f| f.hops).sum();
        let weight = if minutes < 60.0 { 25 } else { 15 };
        out.push(sig(
            "funded_together",
            weight,
            &[
                ("wallets", best.len() as f64),
                ("minutes", minutes.round()),
                ("spread", spread),
                ("sources", sources as f64),
                ("hops", hops as f64),
            ],
        ));
    }

    let throwaway = fs
        .iter()
        .filter(|f| f.hops > 0 && f.seconds_before_launch <= 3600)
        .count();
    if throwaway > 0 {
        out.push(sig("throwaway_funding", 20, &[("wallets", throwaway as f64)]));
    }

    let woken: Vec<f64> = fs
        .iter()
        .filter(|f| f.seconds_before_launch <= WOKEN_FUNDED_WITHIN_SECS)
        .filter_map(|f| f.idle_days.filter(|d| *d >= WOKEN_IDLE_DAYS))
        .collect();
    if !woken.is_empty() {
        out.push(sig(
            "woken_wallets",
            15,
            &[("wallets", woken.len() as f64), ("days", median(woken).round())],
        ));
    }

    let aged = fs.iter().filter(|f| f.seconds_before_launch >= AGED_SECS).count();
    if fs.len() >= 3 && aged * 10 >= fs.len() * 6 {
        out.push(sig("aged_funding", 5, &[("wallets", aged as f64)]));
    }

    // Several insiders funded by the same exchange at spaced-out times, to
    // look unrelated.
    let mut by_exchange: HashMap<String, Vec<i64>> = HashMap::new();
    for f in &fs {
        if let Some(ex) = labels::exchange(f.root.as_deref().unwrap_or(&f.funder)) {
            by_exchange.entry(ex).or_default().push(f.time);
        }
    }
    if let Some(times) = by_exchange.values().filter(|t| t.len() >= 3).max_by_key(|t| t.len()) {
        let gaps: Vec<f64> = times.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
        if gaps.iter().all(|g| *g >= 30.0 && *g <= 3600.0) {
            out.push(sig("staggered_cex_funding", 15, &[("gaps", median(gaps))]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(funder: &str, time: i64, launch: i64, hops: u32, idle: Option<f64>) -> Funding {
        Funding {
            funder: funder.into(),
            funder_label: None,
            amount: 1.0,
            time,
            seconds_before_launch: launch - time,
            root: None,
            root_label: None,
            hops,
            idle_days: idle,
        }
    }

    #[test]
    fn funding_signals() {
        let launch = 100_000;
        let m: HashMap<String, Funding> = [
            ("a".into(), f("x", launch - 300, launch, 1, None)),
            ("b".into(), f("y", launch - 290, launch, 1, Some(45.0))),
            ("c".into(), f("z", launch - 280, launch, 0, None)),
            ("d".into(), f("x", launch - 50_000, launch, 0, Some(2.0))),
        ]
        .into();
        let s = signals(&m);
        let get = |c: &str| s.iter().find(|x| x.code == c);
        let together = get("funded_together").expect("three funded within 20 s");
        assert_eq!(together.params["wallets"], 3.0);
        assert_eq!(together.weight, 25, "minutes before launch");
        assert_eq!(get("throwaway_funding").unwrap().params["wallets"], 2.0);
        assert_eq!(get("woken_wallets").unwrap().params["wallets"], 1.0);
        assert!(get("aged_funding").is_none(), "only one of four funded a day early");
    }
}
