//! Bonding-curve trades and the market-cap chart built from them.
//!
//! Every pump.fun trade emits a `TradeEvent` carrying the curve's virtual
//! reserves after the trade, which gives the price at that moment without
//! any outside price feed.
//!
//! Curves and pools can trade against any token, not only SOL (USDC, a
//! tokenized stock, another coin). Such trades keep their amounts in that
//! token (`Trade::quote`) until `price_quotes` values them in SOL, so every
//! chart and total stays in one unit.

use crate::addr::{PUMPSWAP_PROGRAM, WRAPPED_SOL};
use crate::pump::{events, events_of, Reader};
use crate::tx::Tx;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const TRADE_EVENT: [u8; 8] = [0xbd, 0xdb, 0x7f, 0xd3, 0x4e, 0xe6, 0x61, 0xee];
const AMM_BUY_EVENT: [u8; 8] = [0x67, 0xf4, 0x52, 0x1f, 0x2c, 0xf5, 0x77, 0x77];
const AMM_SELL_EVENT: [u8; 8] = [0x3e, 0x2f, 0x37, 0x0a, 0xa5, 0x03, 0xdc, 0x2a];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub sig: String,
    pub time: i64,
    pub wallet: String,
    pub is_buy: bool,
    pub lamports: u64,
    pub tokens: u64,
    pub virtual_sol: u64,
    pub virtual_tokens: u64,
    /// The PumpSwap pool for trades after migration; `None` on the bonding curve.
    pub pool: Option<String>,
    /// Priced from what the trader paid rather than pool reserves (venues
    /// without a decoder). Fees and rent skew small ones, so the chart prefers
    /// reserve-priced trades.
    #[serde(default)]
    pub generic: bool,
    /// The token the trade was paid in when it isn't SOL. `lamports` and
    /// `virtual_sol` then hold its SOL value, or 0 until `price_quotes` sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<QuoteLeg>,
}

/// A trade's side in a quote token other than SOL, in that token's base units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteLeg {
    pub mint: String,
    /// What the trader paid or got.
    pub amount: u64,
    /// The quote side of the reserves that price the trade, virtual reserves included.
    pub reserves: u64,
    /// SOL per whole token the trade was valued at, once it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sol_per_token: Option<f64>,
}

/// Whether a quote mint pump.fun names is SOL: the default key on curves that predate quotes, wrapped SOL in pools.
fn is_sol(mint: &str) -> bool {
    mint == "11111111111111111111111111111111" || mint == WRAPPED_SOL
}

/// The quote mints of trades not yet valued in SOL.
pub fn unpriced_quotes<'a>(trades: impl IntoIterator<Item = &'a Trade>) -> Vec<String> {
    let mut out: Vec<String> = trades
        .into_iter()
        .filter_map(|t| t.quote.as_ref())
        .filter(|q| q.sol_per_token.is_none())
        .map(|q| q.mint.clone())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Values trades paid in another token in SOL. `rate` gives a quote mint's decimals and SOL per whole token, or
/// `None` when it has no price, and those trades stay at 0. Trades already valued keep their value.
pub fn price_quotes<'a>(trades: impl IntoIterator<Item = &'a mut Trade>, rate: impl Fn(&str) -> Option<(u8, f64)>) {
    for t in trades {
        let Some(q) = t.quote.as_mut().filter(|q| q.sol_per_token.is_none()) else { continue };
        let Some((decimals, sol)) = rate(&q.mint).filter(|(_, sol)| *sol > 0.0 && sol.is_finite()) else { continue };
        let lamports = |raw: u64| (raw as f64 / 10f64.powi(decimals as i32) * sol * 1e9).round() as u64;
        t.lamports = lamports(q.amount);
        t.virtual_sol = lamports(q.reserves);
        q.sol_per_token = Some(sol);
    }
}

impl Trade {
    pub fn sol(&self) -> f64 {
        self.lamports as f64 / 1e9
    }

    /// Market cap in SOL right after this trade.
    pub fn market_cap(&self, supply: u64) -> f64 {
        market_cap(self.virtual_sol, self.virtual_tokens, supply)
    }
}

/// Price × supply, with pump.fun's 9-decimal SOL and 6-decimal tokens.
pub fn market_cap(virtual_sol: u64, virtual_tokens: u64, supply: u64) -> f64 {
    if virtual_tokens == 0 {
        return 0.0;
    }
    (virtual_sol as f64 / 1e9) / (virtual_tokens as f64 / 1e6) * (supply as f64 / 1e6)
}

/// Trades of `mint` in `tx`: bonding-curve trades, then PumpSwap trades
/// after the token migrated.
pub fn trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    // A DBC curve's leftover paid out to its receiver isn't bought: read as a trade, the receiver's new token account's
    // rent would price 99% of the supply at a fraction of a SOL.
    if !tx.succeeded() || !crate::dbc::leftover_withdrawals(tx, mint).is_empty() {
        return vec![];
    }
    let mut out = curve_trades(tx, mint);
    out.extend(amm_trades(tx, mint));
    delegated_sellers(tx, mint, &mut out);
    if out.is_empty() {
        out = launchlab_trades(tx, mint);
    }
    if out.is_empty() {
        out = dbc_trades(tx, mint);
    }
    if out.is_empty() {
        out = generic_trades(tx, mint);
    }
    out
}

/// A sell is the holder's whose tokens went, not the signer's. pump.fun's and PumpSwap's events name the signer, and a
/// wallet can approve another to sell for it: $BUNDLED (Fm46…pump) had 11.8% of its supply bought by 9k6F…kB1, which
/// approved 7QHq…YXc in the same transaction, and 7QHq…YXc sold it from 9k6F…kB1's account the next block. Read so,
/// the seller had never bought and the buyer's tokens left without a trade or a transfer (a transaction with a trade
/// has none). When the named wallet's balance didn't drop, the sell goes to the one wallet whose balance dropped by
/// what was sold.
fn delegated_sellers(tx: &Tx, mint: &str, trades: &mut [Trade]) {
    if trades.iter().all(|t| t.is_buy) {
        return;
    }
    let deltas = tx.token_deltas(mint);
    let mut taken: Vec<String> = trades.iter().map(|t| t.wallet.clone()).collect();
    for t in trades.iter_mut().filter(|t| !t.is_buy) {
        if deltas.get(&t.wallet).is_some_and(|d| *d < 0) {
            continue;
        }
        let holders: Vec<&String> = deltas
            .iter()
            .filter(|(w, d)| **d < 0 && d.unsigned_abs() == t.tokens as u128 && !taken.contains(w))
            .map(|(w, _)| w)
            .collect();
        if let [holder] = holders.as_slice() {
            t.wallet = (*holder).clone();
            taken.push(t.wallet.clone());
        }
    }
}

/// A swap on a bonding curve as its program's event tells it: the pool, the side, the coin and quote amounts, and the
/// price after it as quote reserves over base reserves (raw units).
struct CurveSwap {
    pool: String,
    is_buy: bool,
    base: u64,
    quote: u64,
    reserves_quote: u64,
    reserves_base: u64,
}

/// Trades on a Raydium LaunchLab curve (Bonk's too), from its `TradeEvent`: the pool, then total_base_sell,
/// virtual_base, virtual_quote, real_base_before, real_quote_before, real_base_after, real_quote_after, amount_in,
/// amount_out, protocol_fee, platform_fee, creator_fee and share_fee (u64 each), and trade_direction (0 buy, 1 sell).
/// Priced by the curve's reserves after the trade, (virtual_quote + real_quote) / (virtual_base - real_base), as
/// pump.fun's are. Before this, they were priced by what each wallet paid, which goes wrong when the curve is quoted in
/// another token (Super Inu's in NVDAx: the creator's 3.64 NVDAx read as 0.08 SOL of rent) or one wallet pays for a
/// bundle.
fn launchlab_trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    use crate::addr::{LAUNCHLAB_AUTHORITY, LAUNCHLAB_PROGRAM};
    if !tx.mentions(LAUNCHLAB_PROGRAM) {
        return vec![];
    }
    let swaps: Vec<CurveSwap> = events_of(tx, LAUNCHLAB_PROGRAM)
        .into_iter()
        .filter(|e| e.len() > 8 + 32 + 13 * 8 && e[..8] == TRADE_EVENT)
        .filter_map(|e| {
            let mut r = Reader::new(&e[8..]);
            let pool = r.pubkey()?;
            let _total_base_sell = r.u64()?;
            let (virtual_base, virtual_quote) = (r.u64()?, r.u64()?);
            r.skip(16)?; // real base and quote before
            let (real_base, real_quote) = (r.u64()?, r.u64()?);
            let (amount_in, amount_out) = (r.u64()?, r.u64()?);
            r.skip(32)?; // fees
            let is_buy = r.u8()? == 0;
            let (base, quote) = if is_buy { (amount_out, amount_in) } else { (amount_in, amount_out) };
            Some(CurveSwap {
                pool,
                is_buy,
                base,
                quote,
                reserves_quote: virtual_quote.checked_add(real_quote)?,
                reserves_base: virtual_base.checked_sub(real_base).filter(|b| *b > 0)?,
            })
        })
        .collect();
    curve_swap_trades(tx, mint, LAUNCHLAB_AUTHORITY, swaps)
}

/// Meteora DBC's `EvtSwap` (and `EvtSwap2`, which newer versions emit beside it). A transfer-hook coin's swaps emit
/// `EvtSwap2WithTransferHook` alone, laid out as `EvtSwap2` up to the curve's price.
const DBC_SWAP_EVENT: [u8; 8] = [0x1b, 0x3c, 0x15, 0xd5, 0x8a, 0xaa, 0xbb, 0x93];
const DBC_SWAP2_EVENT: [u8; 8] = [0xbd, 0x42, 0x33, 0xa8, 0x26, 0x50, 0x75, 0x99];
const DBC_SWAP2_HOOK_EVENT: [u8; 8] = [0x86, 0x3b, 0xa8, 0x78, 0x5e, 0x33, 0x72, 0xe7];
/// A DBC price, quote per base in raw units, is carried as reserves of this many base units, so a curve's price fits
/// in a `u64` from a few dollars of market cap to billions.
const DBC_PRICE_SCALE: f64 = 1e17;

/// Trades on a Meteora DBC curve (Bags and others), from its swap event: the pool and config, trade_direction (1 buy,
/// 0 sell), has_referral, then for `EvtSwap` the parameters (amount_in, minimum_amount_out) and the result
/// (actual_input_amount, output_amount, next_sqrt_price as a u128, fees); for `EvtSwap2` the parameters (amount_0,
/// amount_1, swap_mode) and the result (included_fee_input_amount, excluded_fee_input_amount, amount_left,
/// output_amount, next_sqrt_price, fees). Priced by the curve's price after the swap, (next_sqrt_price / 2^64)^2. Before
/// this, DBC trades were priced by what each wallet paid, and a sell that paid out elsewhere read as a crash.
fn dbc_trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    use crate::addr::{DBC_POOL_AUTHORITY, DBC_PROGRAM};
    if !tx.mentions(DBC_PROGRAM) {
        return vec![];
    }
    let events = events_of(tx, DBC_PROGRAM);
    // Both events describe the same swap: read `EvtSwap` when it's there, else `EvtSwap2` (or its transfer-hook twin).
    let v1 = events.iter().any(|e| e.len() >= 8 && e[..8] == DBC_SWAP_EVENT);
    let swaps: Vec<CurveSwap> = events
        .iter()
        .filter(|e| e.len() >= 8 && if v1 { e[..8] == DBC_SWAP_EVENT } else { e[..8] == DBC_SWAP2_EVENT || e[..8] == DBC_SWAP2_HOOK_EVENT })
        .filter_map(|e| {
            let mut r = Reader::new(&e[8..]);
            let pool = r.pubkey()?;
            r.skip(32)?; // config
            let is_buy = r.u8()? == 1;
            r.skip(1)?; // has_referral
            let (input, output) = if v1 {
                let amount_in = r.u64()?;
                r.skip(8 + 8)?; // minimum out, actual input
                (amount_in, r.u64()?)
            } else {
                r.skip(8 + 8 + 1)?; // parameters
                let included_fee_input = r.u64()?;
                r.skip(8 + 8)?; // excluded fee input, amount left
                (included_fee_input, r.u64()?)
            };
            let sqrt = r.u128()?;
            let price = (sqrt as f64 / 2f64.powi(64)).powi(2);
            let reserves_quote = (price * DBC_PRICE_SCALE).round();
            if !(reserves_quote >= 1.0 && reserves_quote < u64::MAX as f64) {
                return None;
            }
            let (base, quote) = if is_buy { (output, input) } else { (input, output) };
            Some(CurveSwap { pool, is_buy, base, quote, reserves_quote: reserves_quote as u64, reserves_base: DBC_PRICE_SCALE as u64 })
        })
        .collect();
    curve_swap_trades(tx, mint, DBC_POOL_AUTHORITY, swaps)
}

/// `mint`'s trades from a curve's swap events. The quote token is the one the curve's vault (owned by `authority`)
/// took in or paid out besides the coin; only this coin's pool counts, the one whose swaps add up to what the vault's
/// balance of it did; and the trader is the wallet whose balance of `mint` moved by the swap's amount, a signer first
/// (a token with a transfer fee arrives a little short). Empty when any of that can't be told, and the transaction
/// falls back to `generic_trades`.
fn curve_swap_trades(tx: &Tx, mint: &str, authority: &str, swaps: Vec<CurveSwap>) -> Vec<Trade> {
    if swaps.is_empty() {
        return vec![];
    }
    // `owner_token_deltas` leaves wrapped SOL out, and most curves are quoted in it.
    let mut vault = tx.owner_token_deltas(authority);
    if let Some(&d) = tx.token_deltas(WRAPPED_SOL).get(authority).filter(|d| **d != 0) {
        vault.insert(WRAPPED_SOL.to_string(), d);
    }
    let Some(&vault_base) = vault.get(mint) else { return vec![] };
    // One quote token moving through the vault besides the coin: more would be a transaction trading several coins.
    let quotes: Vec<&String> = vault.keys().filter(|m| *m != mint).collect();
    let [quote] = quotes.as_slice() else { return vec![] };
    let mut pools: Vec<&str> = swaps.iter().map(|e| e.pool.as_str()).collect();
    pools.sort_unstable();
    pools.dedup();
    let net = |pool: &str| -> i128 {
        swaps.iter().filter(|e| e.pool == pool).map(|e| if e.is_buy { -(e.base as i128) } else { e.base as i128 }).sum()
    };
    let Some(pool) = pools.iter().copied().find(|p| pools.len() == 1 || net(p) == vault_base).map(str::to_string) else {
        return vec![];
    };
    let signers = tx.signers();
    let mut wallets: Vec<(String, i128)> =
        tx.token_deltas(mint).into_iter().filter(|(o, d)| *d != 0 && o != authority).collect();
    let quote = (!is_sol(quote.as_str())).then(|| quote.to_string());
    swaps
        .iter()
        .filter(|e| e.pool == pool)
        .filter_map(|e| {
            // The trader: a signer if one moved the right way, else anyone who did, nearest the trade's amount.
            let side = |d: &i128| if e.is_buy { *d > 0 } else { *d < 0 };
            let gap = |d: &i128| (d.unsigned_abs() as i128 - e.base as i128).abs();
            let pick = wallets
                .iter()
                .enumerate()
                .filter(|(_, (_, d))| side(d))
                .min_by_key(|(_, (o, d))| (!signers.contains(&o.as_str()), gap(d)))
                .map(|(i, _)| i)?;
            let (wallet, d) = wallets.remove(pick);
            let (lamports, virtual_sol, leg) = match &quote {
                None => (e.quote, e.reserves_quote, None),
                Some(m) => (
                    0,
                    0,
                    Some(QuoteLeg { mint: m.clone(), amount: e.quote, reserves: e.reserves_quote, sol_per_token: None }),
                ),
            };
            Some(Trade {
                sig: tx.signature().to_string(),
                time: tx.block_time.unwrap_or(0),
                wallet,
                is_buy: e.is_buy,
                lamports,
                tokens: d.unsigned_abs() as u64,
                virtual_sol,
                virtual_tokens: e.reserves_base,
                pool: None,
                generic: false,
                quote: leg,
            })
        })
        .collect()
}

/// Trades on venues without a dedicated decoder (Meteora DBC, Bonk, routers): a
/// signer whose balance of `mint` changed, priced by the SOL it paid or got
/// (native plus wrapped, with the fee added back for the fee payer), or, when
/// SOL barely moved, by the one other token it paid or got (USDC, USD1, another
/// coin). Prices include fees, so they're execution prices rather than pool
/// prices.
fn generic_trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    let signers = tx.signers();
    let fee_payer = signers.first().copied();
    let fee = tx.meta.as_ref().map_or(0, |m| m.fee) as i64;
    let sol: std::collections::HashMap<&str, i64> = tx.sol_deltas().into_iter().collect();
    let wsol = tx.token_deltas(crate::addr::WRAPPED_SOL);
    tx.token_deltas(mint)
        .into_iter()
        .filter(|(owner, d)| *d != 0 && signers.contains(&owner.as_str()))
        .filter_map(|(owner, d)| {
            let mut change =
                sol.get(owner.as_str()).copied().unwrap_or(0) + wsol.get(&owner).copied().unwrap_or(0) as i64;
            if Some(owner.as_str()) == fee_payer {
                change += fee;
            }
            let (is_buy, lamports) = if d > 0 { (true, -change) } else { (false, change) };
            let tokens = d.unsigned_abs() as u64;
            // Fees and account rent aside, no SOL changed hands: priced in the token that did. Never for a dollar
            // stablecoin itself, which is what people pay with (a wallet's PnL leaves it as it was).
            if lamports < MIN_GENERIC_PRICE_LAMPORTS as i64 && !crate::addr::is_stablecoin(mint) {
                if let Some((quote, amount)) = paid_in_token(tx, &owner, mint, d) {
                    return Some(Trade {
                        sig: tx.signature().to_string(),
                        time: tx.block_time.unwrap_or(0),
                        wallet: owner,
                        is_buy,
                        lamports: 0,
                        tokens,
                        virtual_sol: 0,
                        virtual_tokens: tokens,
                        pool: None,
                        generic: true,
                        quote: Some(QuoteLeg { mint: quote, amount, reserves: amount, sol_per_token: None }),
                    });
                }
            }
            (lamports > 0).then(|| {
                Trade {
                    sig: tx.signature().to_string(),
                    time: tx.block_time.unwrap_or(0),
                    wallet: owner,
                    is_buy,
                    lamports: lamports as u64,
                    tokens,
                    virtual_sol: lamports as u64,
                    virtual_tokens: tokens,
                    pool: None,
                    generic: true,
                    quote: None,
                }
            })
        })
        .collect()
}

/// The one token other than `mint` (and SOL) that `owner` moved the other way, and how much: what it paid with or got.
/// `None` when there's none, or more than one to choose from.
fn paid_in_token(tx: &Tx, owner: &str, mint: &str, delta: i128) -> Option<(String, u64)> {
    let mut other = tx
        .owner_token_deltas(owner)
        .into_iter()
        .filter(|(m, d)| m != mint && d.signum() == -delta.signum());
    let (m, d) = other.next()?;
    other.next().is_none().then(|| (m, d.unsigned_abs() as u64))
}

fn curve_trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    events(tx)
        .into_iter()
        .filter(|e| e[..8] == TRADE_EVENT)
        .filter_map(|e| {
            let mut r = Reader::new(&e[8..]);
            let event_mint = r.pubkey()?;
            let lamports = r.u64()?;
            let tokens = r.u64()?;
            let is_buy = r.u8()? == 1;
            let wallet = r.pubkey()?;
            let time = r.i64()?;
            let virtual_sol = r.u64()?;
            let virtual_tokens = r.u64()?;
            if event_mint != mint {
                return None;
            }
            // A curve quoted in another token leaves the SOL fields at 0 and names the token at the end.
            let quote = (virtual_sol == 0)
                .then(|| curve_quote(&mut r))
                .flatten()
                .filter(|(m, ..)| !is_sol(m))
                .map(|(mint, amount, reserves)| QuoteLeg { mint, amount, reserves, sol_per_token: None });
            Some(Trade {
                sig: tx.signature().to_string(),
                time: tx.block_time.unwrap_or(time),
                wallet,
                is_buy,
                lamports,
                tokens,
                virtual_sol,
                virtual_tokens,
                pool: None,
                generic: false,
                quote,
            })
        })
        .collect()
}

/// The quote mint, amount and virtual quote reserves at the end of a `TradeEvent`, read on from its virtual token
/// reserves (pump.fun's IDL: github.com/pump-fun/pump-public-docs, idl/pump.json). `None` for older, shorter events.
fn curve_quote(r: &mut Reader) -> Option<(String, u64, u64)> {
    r.skip(16)?; // real SOL and token reserves
    r.skip(32 + 16)?; // fee recipient, fee basis points, fee
    r.skip(32 + 16)?; // creator, creator fee basis points, creator fee
    r.skip(1 + 32)?; // track_volume, unclaimed and claimed tokens, current SOL volume, last update
    r.string()?; // ix_name
    r.skip(1 + 32)?; // mayhem_mode, cashback and buyback basis points and amounts
    let shareholders = r.u32()? as usize;
    r.skip(shareholders.checked_mul(34)?)?; // pubkey and u16 basis points each
    Some((r.pubkey()?, r.u64()?, r.u64()?))
}

/// PumpSwap buys and sells of `mint`. The events carry the pool's reserves
/// before the trade; the reserves after it follow from the amounts.
fn amm_trades(tx: &Tx, mint: &str) -> Vec<Trade> {
    events_of(tx, PUMPSWAP_PROGRAM)
        .into_iter()
        .filter_map(|e| {
            let is_buy = match &e[..8] {
                d if d == AMM_BUY_EVENT => true,
                d if d == AMM_SELL_EVENT => false,
                _ => return None,
            };
            let mut r = Reader::new(&e[8..]);
            let time = r.i64()?;
            let mut f = [0u64; 13];
            for v in f.iter_mut() {
                *v = r.u64()?;
            }
            let pool = r.pubkey()?;
            let wallet = r.pubkey()?;
            let user_base = r.pubkey();
            let user_quote = r.pubkey();
            // Only pools that trade this token as their base: a routed tx can touch several pools, and a token can be
            // the quote of another coin's pool.
            match user_base.as_deref().and_then(|a| tx.mint_of(a)) {
                Some(base) if base != mint => return None,
                Some(_) => {}
                None if !tx.owns_token_account(&pool, mint) => return None,
                None => {}
            }
            // What the pool trades it against: the other token it holds, else what the trader paid from.
            let others: Vec<&str> = tx.mints_held_by(&pool).into_iter().filter(|m| *m != mint).collect();
            let quote_mint = match others.as_slice() {
                [q] => Some(q.to_string()),
                _ => user_quote.as_deref().and_then(|a| tx.mint_of(a)).map(str::to_string),
            };
            let virtual_quote = amm_virtual_quote(&mut r, is_buy).unwrap_or(0);
            // f[0] tokens moved; f[4], f[5] pool token and quote reserves before;
            // f[11] quote into or out of the pool; f[12] quote the user paid or got.
            let (tokens, pool_tokens, pool_quote, pool_quote_change, user_quote_amount) = (f[0], f[4], f[5], f[11], f[12]);
            let (virtual_tokens, quote_reserves) = if is_buy {
                (pool_tokens.checked_sub(tokens)?, pool_quote.checked_add(pool_quote_change)?)
            } else {
                (pool_tokens.checked_add(tokens)?, pool_quote.checked_sub(pool_quote_change)?)
            };
            let quote_reserves = u64::try_from(quote_reserves as i128 + virtual_quote).ok()?;
            let quote = quote_mint.filter(|m| !is_sol(m)).map(|mint| QuoteLeg {
                mint,
                amount: user_quote_amount,
                reserves: quote_reserves,
                sol_per_token: None,
            });
            let (lamports, virtual_sol) = if quote.is_some() { (0, 0) } else { (user_quote_amount, quote_reserves) };
            Some(Trade {
                sig: tx.signature().to_string(),
                time: tx.block_time.unwrap_or(time),
                wallet,
                is_buy,
                lamports,
                tokens,
                virtual_sol,
                virtual_tokens,
                pool: Some(pool),
                generic: false,
                quote,
            })
        })
        .collect()
}

/// A PumpSwap event's virtual quote reserves, read on from the user's quote token account (pump.fun's IDL,
/// idl/pump_amm.json). A boosted pool prices as if it held this much more quote, which can be negative; it's 0
/// elsewhere, and `None` for older events that end sooner.
fn amm_virtual_quote(r: &mut Reader, is_buy: bool) -> Option<i128> {
    r.skip(96 + 16)?; // protocol fee recipient and its token account, coin creator, its fee basis points and fee
    if is_buy {
        r.skip(1 + 24 + 8 + 8)?; // track_volume, unclaimed and claimed tokens, current SOL volume, last update, min out
        r.string()?; // ix_name
    }
    r.skip(32)?; // cashback and buyback basis points and amounts
    r.i128()
}

/// `[time, open, high, low, close, volume]`, market cap and volume in SOL:
/// the shape the original's chart used.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Candle(pub i64, pub f64, pub f64, pub f64, pub f64, pub f64);

/// Execution-priced trades smaller than this don't set the price (0.01 SOL).
const MIN_GENERIC_PRICE_LAMPORTS: u64 = 10_000_000;
/// Execution-priced trades this big (0.1 SOL) set a candle's price when it has any: a small one's price carries its
/// fixed costs (a bot's fee, a tip, account rent), and a 0.027 SOL buy read 2.9 times the market (Super Inu, 18:28).
const RELIABLE_GENERIC_PRICE_LAMPORTS: u64 = 100_000_000;

/// An execution price this many times above or below the trades on both sides of it is a stray print, not the market.
/// Both sides have to agree, so a real move, which agrees with the trades after it, never trips it; 5 left Super Inu's
/// 4.9-times "buy" paid in an obscure coin as its 6-hour peak (2026-10-16).
const STRAY: f64 = 3.0;
/// The trades either side a trade is judged against: enough that two or three bad fills in a row can't outvote the
/// market (Super Inu's good 1 SOL buy sat between two near zero, 2026-10-16), few enough to follow a real move.
const NEIGHBOURS: usize = 4;

/// Which of `trades` (oldest first) are stray prints (see `strays`).
fn stray_prints(trades: &[Trade], supply: u64) -> Vec<bool> {
    let mut out = vec![false; trades.len()];
    for (i, _, _) in strays(trades, supply) {
        out[i] = true;
    }
    out
}

/// The stray prints in `trades` (oldest first): each one's index, market cap, and the median of its neighbours'.
///
/// Only execution-priced trades are judged, against the `NEIGHBOURS` execution-priced trades before it and those after
/// it. One is stray when its price is more than `STRAY` times off both sides', the same way: the market was at one
/// price before it and at the same price after it. That's SOL from elsewhere in the transaction, a wallet that didn't
/// pay for its own tokens, a fill in a pool holding $20, or a trade paid in a coin whose price is off (Super Inu's peak
/// 700 times the market; its 32.8 SOL "buy" paid in an obscure coin, at 5 times every trade around it).
///
/// A real move agrees with the trades on one side of it, whatever its size, so it stays: a creator dumping the supply
/// is followed by trades at the crashed price (RMT, SAMO, TACP, 2026-10-16). A trade with fewer than two trades on
/// either side (the first and last of a chart) isn't judged: one side alone can't tell a stray print from a move, and a
/// dump at the very end must stay. Reserve-priced trades are exact and are never left out. Public for the developer
/// tool that lists them.
#[doc(hidden)]
pub fn strays(trades: &[Trade], supply: u64) -> Vec<(usize, f64, f64)> {
    let idx: Vec<usize> = (0..trades.len())
        .filter(|&i| trades[i].generic && trades[i].virtual_sol > 0 && trades[i].lamports >= MIN_GENERIC_PRICE_LAMPORTS)
        .collect();
    let caps: Vec<f64> = idx.iter().map(|&i| trades[i].market_cap(supply)).collect();
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let n = v.len();
        if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
    };
    let mut out = vec![];
    for (k, &i) in idx.iter().enumerate() {
        let sides: Vec<f64> = [k.saturating_sub(NEIGHBOURS)..k, k + 1..(k + 1 + NEIGHBOURS).min(idx.len())]
            .into_iter()
            .filter(|s| s.len() >= 2)
            .map(|s| median(s.map(|j| caps[j]).collect()))
            .collect();
        if sides.len() < 2 || sides.iter().any(|&m| m <= 0.0) {
            continue;
        }
        let c = caps[k];
        if sides.iter().all(|&m| c > m * STRAY) || sides.iter().all(|&m| c < m / STRAY) {
            out.push((i, c, median(sides)));
        }
    }
    out
}

/// OHLC candles of market cap. Gaps without trades are filled with flat
/// candles at the last close, so the line doesn't jump. Stray prints (see `stray_prints`) don't set the price.
pub fn candles(trades: &[Trade], supply: u64, start: i64, interval: i64) -> Vec<Candle> {
    candles_keeping_strays(trades, supply, start, interval, false)
}

/// `candles`, with stray prints kept when `keep_strays`: for the developer tool that compares the two.
#[doc(hidden)]
pub fn candles_keeping_strays(trades: &[Trade], supply: u64, start: i64, interval: i64, keep_strays: bool) -> Vec<Candle> {
    let stray = if keep_strays { vec![false; trades.len()] } else { stray_prints(trades, supply) };
    let mut by_bucket: BTreeMap<i64, Vec<(&Trade, bool)>> = BTreeMap::new();
    for (t, s) in trades.iter().zip(stray) {
        let bucket = start + (t.time - start).max(0) / interval * interval;
        by_bucket.entry(bucket).or_default().push((t, s));
    }
    let Some(&last) = by_bucket.keys().next_back() else {
        return vec![];
    };
    let mut out = vec![];
    let mut prev_close: Option<f64> = None;
    let mut bucket = start;
    while bucket <= last {
        match by_bucket.get(&bucket) {
            Some(ts) => {
                // Reserve-priced trades when there are any; otherwise execution
                // prices, leaving out dust where fees and rent dominate. A trade
                // in another token that has no SOL value yet can't set the price.
                let priced: Vec<&Trade> = ts.iter().map(|(t, _)| *t).filter(|t| !t.generic && t.virtual_sol > 0).collect();
                // `usable` sets the close; `wicks`, the high and low. Execution-priced, a small trade can close the
                // candle (after a dump, the small sells at the crashed price are the market) but only reaches the
                // wicks when the candle has nothing bigger.
                let (usable, wicks): (Vec<&Trade>, Vec<&Trade>) = if priced.is_empty() {
                    let paid: Vec<&Trade> =
                        ts.iter().filter(|(t, stray)| !stray && t.lamports >= MIN_GENERIC_PRICE_LAMPORTS).map(|(t, _)| *t).collect();
                    let reliable: Vec<&Trade> = paid.iter().copied().filter(|t| t.lamports >= RELIABLE_GENERIC_PRICE_LAMPORTS).collect();
                    let wicks = if reliable.is_empty() { paid.clone() } else { reliable };
                    (paid, wicks)
                } else {
                    (priced.clone(), priced)
                };
                let caps: Vec<f64> = usable.iter().map(|t| t.market_cap(supply)).collect();
                let volume = ts.iter().map(|(t, _)| t.sol()).sum();
                if caps.is_empty() {
                    if let Some(c) = prev_close {
                        out.push(Candle(bucket, c, c, c, c, volume));
                    }
                    bucket += interval;
                    continue;
                }
                let open = prev_close.unwrap_or(caps[0]);
                let close = *caps.last().unwrap();
                let wick: Vec<f64> = wicks.iter().map(|t| t.market_cap(supply)).collect();
                let high = wick.iter().copied().fold(open.max(close), f64::max);
                let low = wick.iter().copied().fold(open.min(close), f64::min);
                out.push(Candle(bucket, open, high, low, close, volume));
                prev_close = Some(close);
            }
            None => {
                if let Some(c) = prev_close {
                    out.push(Candle(bucket, c, c, c, c, 0.0));
                }
            }
        }
        bucket += interval;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkWallet {
    pub address: String,
    pub role: String,
    pub sol: f64,
}

/// Trades by flagged wallets in one candle and direction.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub t: i64,
    pub side: &'static str,
    pub trades: usize,
    pub sol: f64,
    pub creator_sol: f64,
    pub wallets: Vec<MarkWallet>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowSummary {
    pub insider_in: f64,
    pub insider_out: f64,
    pub creator_in: f64,
    pub creator_out: f64,
}

/// Buy and sell marks for wallets in `roles` (wallet → its main role).
pub fn marks(
    trades: &[Trade],
    roles: &HashMap<String, String>,
    creator: &str,
    start: i64,
    interval: i64,
) -> (Vec<Mark>, FlowSummary) {
    let mut grouped: BTreeMap<(i64, bool), Vec<&Trade>> = BTreeMap::new();
    let mut summary = FlowSummary::default();
    for t in trades.iter().filter(|t| roles.contains_key(&t.wallet)) {
        let bucket = start + (t.time - start).max(0) / interval * interval;
        grouped.entry((bucket, t.is_buy)).or_default().push(t);
        let (inn, out) = if t.wallet == creator {
            (&mut summary.creator_in, &mut summary.creator_out)
        } else {
            (&mut summary.insider_in, &mut summary.insider_out)
        };
        if t.is_buy {
            *inn += t.sol();
        } else {
            *out += t.sol();
        }
    }
    let marks = grouped
        .into_iter()
        .map(|((t, is_buy), ts)| {
            let mut per: BTreeMap<&str, f64> = BTreeMap::new();
            for tr in &ts {
                *per.entry(&tr.wallet).or_default() += tr.sol();
            }
            let mut wallets: Vec<MarkWallet> = per
                .into_iter()
                .map(|(a, sol)| MarkWallet {
                    address: a.to_string(),
                    role: roles[a].clone(),
                    sol,
                })
                .collect();
            wallets.sort_by(|a, b| b.sol.total_cmp(&a.sol));
            Mark {
                t,
                side: if is_buy { "buy" } else { "sell" },
                trades: ts.len(),
                sol: ts.iter().map(|tr| tr.sol()).sum(),
                creator_sol: ts.iter().filter(|tr| tr.wallet == creator).map(|tr| tr.sol()).sum(),
                wallets,
            }
        })
        .collect();
    (marks, summary)
}

/// A direct token transfer: someone sent `mint` to a wallet without a trade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    pub sig: String,
    pub time: i64,
    pub from: String,
    pub to: String,
    pub raw: u64,
}

/// Transfers of `mint` in `tx`, when it isn't a trade: one signer's balance
/// drops and other wallets' balances rise by what it sent.
pub fn transfers(tx: &Tx, mint: &str) -> Vec<Transfer> {
    if !trades(tx, mint).is_empty() {
        return vec![];
    }
    transfers_without_trades(tx, mint)
}

/// `transfers` for a `tx` already known to hold no trade of `mint`: a history read decodes each transaction's trades
/// once and asks for transfers only when there are none, instead of decoding them twice. A DBC curve's leftover paid
/// out is one too, from the curve's vault (`dbc::leftover_withdrawals`).
pub fn transfers_without_trades(tx: &Tx, mint: &str) -> Vec<Transfer> {
    if !tx.succeeded() {
        return vec![];
    }
    let leftover = crate::dbc::leftover_withdrawals(tx, mint);
    if !leftover.is_empty() {
        return leftover;
    }
    let signers = tx.signers();
    let deltas = tx.token_deltas(mint);
    let senders: Vec<&String> = deltas
        .iter()
        .filter(|(o, d)| **d < 0 && signers.contains(&o.as_str()))
        .map(|(o, _)| o)
        .collect();
    let [from] = senders.as_slice() else { return vec![] };
    deltas
        .iter()
        .filter(|(o, d)| **d > 0 && o != from)
        .map(|(to, d)| Transfer {
            sig: tx.signature().to_string(),
            time: tx.block_time.unwrap_or(0),
            from: (*from).clone(),
            to: to.clone(),
            raw: *d as u64,
        })
        .collect()
}
