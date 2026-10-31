//! A crash: a coin that ran losing most of its market cap within minutes, and who sold into it.
//!
//! A report told the launch (bundles, snipers, the creator) and nothing after it: $CODEX (DMhs…pump, 2026-10-22) read
//! "0% bundled", risk low, while its page's chart showed it falling from 1,059 SOL of market cap to 43 in 8 minutes,
//! 2.4 hours after launch. Nobody we flagged sold into it (the creator's buy was locked, its snipers long out): 414
//! wallets sold 73% of the supply, 247 of them within 9 seconds. A crash is told as it was read, with who sold into it,
//! and never as a rug unless insiders were the ones selling.
//!
//! What a wallet sold into it is what it let go of: its sells in the crash less its buys in it. CNfz…ZD3 (2026-10-22)
//! read "422 wallets sold 164% of the supply": 620 SOL changed hands in its 14 minutes, the same tokens bought and sold
//! again and again on the way down, and every sell was added up.
//!
//! But a wallet can sell what it was sent, so a seller's net can hold tokens another wallet bought in the crash and
//! passed on, counted in both wallets' nets: $TRUMPOCEAN (G4wm…pump, 2026-10-23) read "210 wallets sold 105% of the
//! supply", one of them (6rify…) selling 65% it was sent after buying 0.28%, while its pool went from 5.8% of the supply
//! to 97.4%. Sellers adding up to more than wallets held when it started (the supply less the pool's reserve then, the
//! whole supply on a curve) is that: then what they let go of is the crash's sells less its buys, what went back into
//! the pool, exactly, and never over 100%.

use crate::pct;
use crate::trades::Trade;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

/// A fall counts when the low is at most this share below the high before it.
pub const DROP: f64 = 0.8;
/// ...within this long of that high.
pub const WINDOW_SECS: i64 = 15 * 60;
/// ...and that high was at least this many times the launch's market cap: a coin that ran. A pump.fun curve can't fall
/// 80% below a high under 5 times its start, and a fall from a few times it is snipers selling their launch buys, which
/// the report tells already.
pub const RUN: f64 = 10.0;
/// The crash starts at the last trade this close to the high before the low, not at the high itself: $CODEX traded a
/// little under its high for minutes before it fell.
const NEAR_HIGH: f64 = 0.9;

/// Where the crash is: its start and its low, and the market cap at each (in the chart's units), and at the last trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    pub from_cap: f64,
    pub to_cap: f64,
    pub now_cap: f64,
}

/// A coin still below this share of where its crash started hasn't come back from it: the risk stays at least high.
pub const STILL_DOWN: f64 = 0.5;
/// Insiders selling this much of the supply into a crash made it: the risk is extreme.
pub const INSIDERS_DUMPED_PCT: f64 = 5.0;

/// The crash, and who sold into it, between its start and its low.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    pub start: i64,
    pub end: i64,
    pub from_cap: f64,
    pub to_cap: f64,
    /// At the last trade read: whether it came back.
    pub now_cap: f64,
    /// How far it fell, as a percentage of `from_cap`.
    pub drop_pct: f64,
    /// Wallets that sold more than they bought in it, and what they let go of (sells less buys), as a share of the supply;
    /// the crash's sells less its buys when those nets add up to more than wallets held (see the module), and each group
    /// below at most that.
    pub sellers: usize,
    pub sold_pct: f64,
    pub insider_sellers: usize,
    pub insider_sold_pct: f64,
    pub sniper_sellers: usize,
    pub sniper_sold_pct: f64,
    /// Every trade in it was read: false when it fell in a window too busy to read whole and sampled, so the sellers and
    /// what they sold are at least these. Set by the report, which knows what it read.
    pub whole: bool,
}

/// The biggest fall in `trades` (oldest first) that counts as a crash, if any. Only reserve-priced trades set the
/// price, as they're exact: a venue priced by what wallets paid has none, and no crash is told on it.
pub fn find(trades: &[Trade], supply: u64) -> Option<Window> {
    let priced: Vec<(i64, f64)> = trades
        .iter()
        .filter(|t| !t.generic && t.virtual_sol > 0 && t.virtual_tokens > 0)
        .map(|t| (t.time, t.market_cap(supply)))
        .collect();
    let launch = priced.first()?.1;
    // The highest cap in the window before each trade (a monotonic queue), and the deepest fall below it.
    let mut highs: VecDeque<usize> = VecDeque::new();
    let mut best: Option<(usize, usize, f64)> = None;
    for (j, &(time, cap)) in priced.iter().enumerate() {
        while highs.front().is_some_and(|&i| priced[i].0 < time - WINDOW_SECS) {
            highs.pop_front();
        }
        while highs.back().is_some_and(|&i| priced[i].1 <= cap) {
            highs.pop_back();
        }
        highs.push_back(j);
        let h = highs[0];
        let drop = 1.0 - cap / priced[h].1;
        if drop >= DROP && priced[h].1 >= launch * RUN && best.is_none_or(|(_, _, d)| drop > d) {
            best = Some((h, j, drop));
        }
    }
    let (h, low, _) = best?;
    let high = priced[h].1;
    let start = (h..low).rev().find(|&i| priced[i].1 >= high * NEAR_HIGH).unwrap_or(h);
    Some(Window {
        start: priced[start].0,
        end: priced[low].0,
        from_cap: priced[start].1,
        to_cap: priced[low].1,
        now_cap: priced.last()?.1,
    })
}

/// The crash in `trades`, with who sold into it: `insider` and `sniper` say which wallets the report flagged as each.
/// `whole` is true here; the report sets it from what it read.
pub fn crash(trades: &[Trade], supply: u64, insider: impl Fn(&str) -> bool, sniper: impl Fn(&str) -> bool) -> Option<Crash> {
    let w = find(trades, supply)?;
    // Each wallet's sells less its buys in it: a wallet that sold and bought back, or bought the dip and sold it, let go
    // of only the difference.
    let mut net: HashMap<&str, i128> = HashMap::new();
    for t in trades.iter().filter(|t| t.time >= w.start && t.time <= w.end) {
        *net.entry(t.wallet.as_str()).or_default() += if t.is_buy { -(t.tokens as i128) } else { t.tokens as i128 };
    }
    // What wallets held when it started: the supply less the pool's reserve before its first trade. A curve's reserves
    // are virtual, so there it's the supply.
    let held = trades.iter().find(|t| t.time >= w.start && t.pool.is_some()).map_or(supply as i128, |t| {
        let before = t.virtual_tokens as i128 + if t.is_buy { t.tokens as i128 } else { -(t.tokens as i128) };
        supply as i128 - before
    });
    let sellers_net: i128 = net.values().filter(|n| **n > 0).sum();
    let cap = if sellers_net > held { net.values().sum::<i128>().max(0) } else { sellers_net };
    let group = |keep: &dyn Fn(&str) -> bool| {
        let sold: Vec<i128> = net.iter().filter(|(w, n)| **n > 0 && keep(w)).map(|(_, n)| *n).collect();
        (sold.len(), pct(sold.iter().sum::<i128>().min(cap) as u128, supply))
    };
    let (sellers, sold_pct) = group(&|_| true);
    let (insider_sellers, insider_sold_pct) = group(&|w| insider(w));
    // A wallet that is both counts as an insider only.
    let (sniper_sellers, sniper_sold_pct) = group(&|w| sniper(w) && !insider(w));
    Some(Crash {
        start: w.start,
        end: w.end,
        from_cap: w.from_cap,
        to_cap: w.to_cap,
        now_cap: w.now_cap,
        drop_pct: (1.0 - w.to_cap / w.from_cap) * 100.0,
        sellers,
        sold_pct,
        insider_sellers,
        insider_sold_pct,
        sniper_sellers,
        sniper_sold_pct,
        whole: true,
    })
}

impl Crash {
    /// Still below `STILL_DOWN` of where it started falling.
    pub fn still_down(&self) -> bool {
        self.now_cap <= self.from_cap * STILL_DOWN
    }

    /// The least risk a coin with this crash reads: extreme when insiders sold into it, high while it's still down.
    pub fn risk_floor(&self) -> Option<&'static str> {
        if self.insider_sold_pct >= INSIDERS_DUMPED_PCT {
            Some("extreme")
        } else if self.still_down() {
            Some("high")
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPLY: u64 = 1_000_000_000_000_000;

    /// A trade at `cap` SOL of market cap, selling or buying `pct_of_supply`.
    fn trade(time: i64, wallet: &str, is_buy: bool, cap: f64, pct_of_supply: f64) -> Trade {
        Trade {
            sig: format!("{wallet}-{time}"),
            time,
            wallet: wallet.into(),
            is_buy,
            lamports: 1_000_000_000,
            tokens: (SUPPLY as f64 * pct_of_supply / 100.0) as u64,
            virtual_sol: (cap / 1000.0 * 1e9) as u64,
            virtual_tokens: 1_000_000_000_000,
            pool: None,
            generic: false,
            quote: None,
        }
    }

    #[test]
    fn a_fall_from_a_run_is_a_crash_and_starts_near_its_high() {
        let t = vec![
            trade(0, "dev", true, 30.0, 5.0),
            trade(600, "a", true, 1_000.0, 1.0),
            trade(900, "b", true, 950.0, 1.0),
            trade(1_000, "c", false, 600.0, 3.0),
            trade(1_010, "dev", false, 100.0, 4.0),
            trade(1_020, "d", false, 60.0, 2.0),
            trade(1_100, "e", true, 65.0, 0.5),
        ];
        let c = crash(&t, SUPPLY, |w| w == "dev", |_| false).unwrap();
        assert_eq!((c.start, c.end), (900, 1_020));
        assert!((c.drop_pct - (1.0 - 60.0 / 950.0) * 100.0).abs() < 0.01, "{}", c.drop_pct);
        assert_eq!((c.sellers, c.insider_sellers), (3, 1));
        assert!((c.sold_pct - 9.0).abs() < 1e-6 && (c.insider_sold_pct - 4.0).abs() < 1e-6);
    }

    #[test]
    fn churn_in_a_crash_counts_once() {
        // "a" sells 3%, buys it back, then sells 6%: it let go of 6%, not 9%. "b" buys the dip and sells it: nothing.
        let t = vec![
            trade(0, "dev", true, 30.0, 5.0),
            trade(600, "a", true, 1_000.0, 3.0),
            trade(900, "c", true, 950.0, 1.0),
            trade(1_000, "a", false, 600.0, 3.0),
            trade(1_005, "a", true, 620.0, 3.0),
            trade(1_008, "b", true, 400.0, 2.0),
            trade(1_010, "a", false, 100.0, 3.0),
            trade(1_015, "b", false, 80.0, 2.0),
            trade(1_020, "a", false, 60.0, 3.0),
        ];
        let c = crash(&t, SUPPLY, |_| false, |_| false).unwrap();
        assert_eq!((c.start, c.end), (900, 1_020));
        assert_eq!(c.sellers, 1);
        assert!((c.sold_pct - 6.0).abs() < 1e-6, "{}", c.sold_pct);
    }

    #[test]
    fn a_slow_bleed_is_not_a_crash() {
        // 90% down, but over an hour.
        let t: Vec<Trade> = (0..=12).map(|i| trade(i * 300, "w", i == 0, if i == 0 { 30.0 } else { 1_000.0 * (1.0 - 0.075 * i as f64) }, 0.1)).collect();
        assert_eq!(find(&t, SUPPLY), None);
    }

    #[test]
    fn snipers_selling_their_launch_buys_is_not_a_crash() {
        // From 5 times the launch back to it in a minute: the snipers' exit, told already.
        let t = vec![trade(0, "dev", true, 30.0, 5.0), trade(5, "s", true, 150.0, 20.0), trade(60, "s", false, 29.0, 20.0)];
        assert_eq!(find(&t, SUPPLY), None);
    }

    #[test]
    fn execution_priced_trades_tell_no_crash() {
        let mut t = vec![trade(0, "dev", true, 30.0, 5.0), trade(60, "a", true, 1_000.0, 1.0), trade(120, "b", false, 50.0, 1.0)];
        for x in t.iter_mut() {
            x.generic = true;
        }
        assert_eq!(find(&t, SUPPLY), None);
    }
}
