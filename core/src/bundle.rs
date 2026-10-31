//! Who bought at launch, and how much.

use crate::addr::PROXIMA_ROUTER;
use crate::pct;
use crate::tx::Tx;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Buy {
    pub wallet: String,
    pub raw: u64,
    pub sig: String,
}

/// Tokens of `mint` bought in `tx`: positive balance changes for wallets that
/// signed it. The bonding curve, pools and migration vaults never sign, so
/// they drop out, and it doesn't matter which program or router ran the trade
/// or whether the logs were truncated.
///
/// What another signer lost in it isn't bought: tokens moved between wallets
/// that both signed are a transfer. A Meteora DBC creator sent its 61.6% to a
/// second wallet in the launch's first second (27Mh…, Gw9f…moon, 2026-10-21),
/// read as a second 61.6% buy: "123% bought in the first second". The signers'
/// gains count less their losses, shared in proportion; a transfer nets to none.
pub fn signer_buys(tx: &Tx, mint: &str) -> Vec<Buy> {
    if !tx.succeeded() {
        return vec![];
    }
    let signers = tx.signers();
    let deltas: Vec<(String, i128)> =
        tx.token_deltas(mint).into_iter().filter(|(owner, _)| signers.contains(&owner.as_str())).collect();
    let gained: i128 = deltas.iter().filter(|(_, d)| *d > 0).map(|(_, d)| d).sum();
    let lost: i128 = deltas.iter().filter(|(_, d)| *d < 0).map(|(_, d)| -d).sum();
    let bought = (gained - lost).max(0);
    deltas
        .into_iter()
        .filter(|(_, d)| *d > 0)
        .map(|(wallet, d)| Buy {
            wallet,
            raw: if lost == 0 { d as u64 } else { (d * bought / gained) as u64 },
            sig: tx.signature().to_string(),
        })
        .filter(|b| b.raw > 0)
        .collect()
}

/// Tokens of `mint` sold in `tx` by wallets that signed it: their losses less what the other signers gained, shared in
/// proportion, as `signer_buys` counts gains (a transfer between signers is neither). `raw` is what was sold.
pub fn signer_sells(tx: &Tx, mint: &str) -> Vec<Buy> {
    if !tx.succeeded() {
        return vec![];
    }
    let signers = tx.signers();
    let deltas: Vec<(String, i128)> =
        tx.token_deltas(mint).into_iter().filter(|(owner, _)| signers.contains(&owner.as_str())).collect();
    let gained: i128 = deltas.iter().filter(|(_, d)| *d > 0).map(|(_, d)| d).sum();
    let lost: i128 = deltas.iter().filter(|(_, d)| *d < 0).map(|(_, d)| -d).sum();
    let sold = (lost - gained).max(0);
    deltas
        .into_iter()
        .filter(|(_, d)| *d < 0)
        .map(|(wallet, d)| Buy {
            wallet,
            raw: if gained == 0 { (-d) as u64 } else { (-d * sold / lost) as u64 },
            sig: tx.signature().to_string(),
        })
        .filter(|b| b.raw > 0)
        .collect()
}

/// A move in a launch's tokens, in chain order: a buy counted for a group, or tokens sold back to the market by a wallet
/// whose buy was counted for a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Move {
    Buy(usize, u128),
    Sell(usize, u128),
}

/// What each of `groups` bought, counting each token once. A buy counts only beyond what was sold back before it and
/// not yet bought again: what one wallet sold back and the next one bought are the same tokens. $LOTTO (FsHA…pump,
/// 2026-10-19): its creator's bundle bought 60% and sold all of it back two blocks later, and three other bundles bought
/// 55%; added up, "115% bundled". $KRAUL (6ER2…wwub): a bundle wallet sold its 35% back and two wallets bought 34%,
/// "103%". Every buy is still told, the first in full: a bundle that dumps keeps its share. Counted this way the groups
/// never add up to more than was held at once, so never more than the supply.
///
/// `plain` is the group of buys that aren't a bundle. What only it sold back and a bundle bought again is the bundle's,
/// taken off the plain buys: $OpenBot (MzUx…pump, 2026-10-28) read "not bundled" after a sniper sold its 25% back and
/// another bundle's two wallets bought 24% of it in the same second. What a bundle sold back is bought again first.
pub fn count_once(flows: &[Move], groups: usize, plain: Option<usize>) -> Vec<u128> {
    // Sold back and not yet bought again: by a bundle, and by the plain buys.
    let (mut kept, mut loose): (u128, u128) = (0, 0);
    let mut out = vec![0u128; groups];
    for f in flows {
        match *f {
            Move::Sell(g, raw) if Some(g) == plain => loose += raw,
            Move::Sell(_, raw) => kept += raw,
            Move::Buy(g, raw) => {
                let again = raw.min(kept);
                kept -= again;
                let rest = raw - again;
                let taken = rest.min(loose);
                loose -= taken;
                // Of what the plain buys sold back, only what they were counted for moves: the rest was counted before.
                let moved = match plain {
                    Some(p) if p != g => {
                        let m = taken.min(out[p]);
                        out[p] -= m;
                        m
                    }
                    _ => 0,
                };
                out[g] += rest - taken + moved;
            }
        }
    }
    out
}

/// True if any of `txs` went through Proxima's router.
pub fn uses_proxima(txs: &[Tx]) -> bool {
    txs.iter().any(|t| t.mentions(PROXIMA_ROUTER))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleAnalysis {
    /// Wallets that bought, creator excluded, largest first.
    pub wallets: Vec<String>,
    pub creator_raw: u128,
    /// Everything bought, the creator's buy included.
    pub total_raw: u128,
    pub creator_pct: f64,
    pub total_pct: f64,
}

/// Sums `buys` per wallet and splits out the creator.
pub fn analyze_bundle(creator: &str, supply: u64, buys: &[Buy]) -> BundleAnalysis {
    let mut per: BTreeMap<&str, u128> = BTreeMap::new();
    for b in buys {
        *per.entry(b.wallet.as_str()).or_default() += b.raw as u128;
    }
    let creator_raw = per.remove(creator).unwrap_or(0);
    let mut others: Vec<(&str, u128)> = per.into_iter().collect();
    others.sort_by(|a, b| b.1.cmp(&a.1));
    let total_raw = creator_raw + others.iter().map(|(_, r)| r).sum::<u128>();
    BundleAnalysis {
        wallets: others.into_iter().map(|(w, _)| w.to_string()).collect(),
        creator_raw,
        total_raw,
        creator_pct: pct(creator_raw, supply),
        total_pct: pct(total_raw, supply),
    }
}
