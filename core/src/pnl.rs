//! Realized PnL of a wallet from its pump.fun and PumpSwap trades: SOL from
//! selling tokens it bought, minus what it paid for the part it sold. Tokens it
//! only received (transfers, airdrops) don't count, as on the original.

use crate::trades::{self, Trade};
use crate::tx::Tx;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Pnl {
    /// SOL gained on what was sold, against the average cost of what was bought.
    pub realized: f64,
    /// SOL spent buying.
    pub bought: f64,
    /// SOL received selling.
    pub sold: f64,
    /// Distinct tokens traded.
    pub tokens: usize,
}

/// `wallet`'s own trades in `txs`, across every token they touch.
pub fn wallet_trades(wallet: &str, txs: &[Tx]) -> Vec<(String, Trade)> {
    let mut out = vec![];
    for tx in txs {
        for mint in tx.mints() {
            for t in trades::trades(tx, mint) {
                if t.wallet == wallet {
                    out.push((mint.to_string(), t));
                }
            }
        }
    }
    out
}

pub fn pnl(trades: &[(String, Trade)]) -> Pnl {
    #[derive(Default)]
    struct Pos {
        bought_tokens: u128,
        sold_tokens: u128,
        sol_in: f64,
        sol_out: f64,
    }
    let mut by_mint: HashMap<&str, Pos> = HashMap::new();
    for (mint, t) in trades {
        let p = by_mint.entry(mint).or_default();
        if t.is_buy {
            p.bought_tokens += t.tokens as u128;
            p.sol_in += t.sol();
        } else {
            p.sold_tokens += t.tokens as u128;
            p.sol_out += t.sol();
        }
    }
    let mut out = Pnl {
        tokens: by_mint.len(),
        ..Default::default()
    };
    for p in by_mint.values() {
        out.bought += p.sol_in;
        out.sold += p.sol_out;
        if p.bought_tokens == 0 {
            // Sold tokens it never bought here: received, not traded.
            continue;
        }
        let sold_share = (p.sold_tokens as f64 / p.bought_tokens as f64).min(1.0);
        // Count only the proceeds from tokens it actually bought.
        let proceeds = if p.sold_tokens as f64 > p.bought_tokens as f64 {
            p.sol_out * (p.bought_tokens as f64 / p.sold_tokens as f64)
        } else {
            p.sol_out
        };
        out.realized += proceeds - p.sol_in * sold_share;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(is_buy: bool, tokens: u64, lamports: u64) -> Trade {
        Trade {
            sig: String::new(),
            time: 0,
            wallet: "w".into(),
            is_buy,
            lamports,
            tokens,
            virtual_sol: 0,
            virtual_tokens: 1,
            pool: None,
            generic: false,
            quote: None,
        }
    }

    #[test]
    fn realized_pnl_counts_only_what_was_bought_and_sold() {
        let trades = vec![
            // Bought 100 for 1 SOL, sold half for 2 SOL: +1.5 realized.
            ("A".to_string(), t(true, 100, 1_000_000_000)),
            ("A".to_string(), t(false, 50, 2_000_000_000)),
            // Sold tokens it never bought (received): ignored.
            ("B".to_string(), t(false, 10, 5_000_000_000)),
            // Bought and still holding: no realized gain or loss.
            ("C".to_string(), t(true, 10, 3_000_000_000)),
        ];
        let p = pnl(&trades);
        assert!((p.realized - 1.5).abs() < 1e-9, "{}", p.realized);
        assert!((p.bought - 4.0).abs() < 1e-9);
        assert!((p.sold - 7.0).abs() < 1e-9);
        assert_eq!(p.tokens, 3);
    }
}
