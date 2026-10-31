//! Meteora Dynamic Bonding Curve launches.

use crate::addr::{BAGS_WALLET, DBC_POOL_AUTHORITY, DBC_PROGRAM};
use crate::pump::{Launch, Reader};
use crate::trades::Transfer;
use crate::tx::Tx;

/// `initialize_virtual_pool_with_token2022`, `..._with_spl_token` and `..._with_token2022_transfer_hook` (a Token-2022
/// mint whose transfers also run a program the launcher picks). All three take the same arguments and the same first six accounts.
const CREATE_TOKEN2022: [u8; 8] = [0xa9, 0x76, 0x33, 0x4e, 0x91, 0x6e, 0xdc, 0x9b];
const CREATE_SPL: [u8; 8] = [0x8c, 0x55, 0xd7, 0xb0, 0x66, 0x36, 0x68, 0x4f];
const CREATE_TRANSFER_HOOK: [u8; 8] = [0xb6, 0x0d, 0xe9, 0xb1, 0x2a, 0x91, 0x87, 0x02];
const CREATES: [[u8; 8]; 3] = [CREATE_TOKEN2022, CREATE_SPL, CREATE_TRANSFER_HOOK];
/// `withdraw_leftover`. Accounts: 0 pool authority, 1 config, 2 pool, 3 the receiver's token account, 4 base vault,
/// 5 base mint, 6 leftover receiver.
const WITHDRAW_LEFTOVER: [u8; 8] = [0x14, 0xc6, 0xca, 0xed, 0xeb, 0xf3, 0xb7, 0x42];

/// Returns the launch if `tx` creates a DBC pool. Accounts: 0 config,
/// 1 pool authority, 2 creator, 3 base mint, 4 quote mint, 5 pool. The
/// instruction data starts with the name, symbol and metadata URI.
pub fn parse_launch(tx: &Tx) -> Option<Launch> {
    if !tx.succeeded() {
        return None;
    }
    let (data, accounts) = tx
        .program_instructions(DBC_PROGRAM)
        .into_iter()
        .find(|(d, _)| d.len() > 8 && CREATES.iter().any(|c| d[..8] == *c))?;
    let mut r = Reader::new(&data[8..]);
    let name = r.string()?;
    let symbol = r.string()?;
    let uri = r.string()?;
    let creator = accounts.get(2)?.clone();
    let mint = accounts.get(3)?.clone();
    let pool = accounts.get(5)?.clone();
    // Everything is minted in the create transaction, so its balances add up to the supply.
    let supply: u64 = tx
        .meta
        .as_ref()?
        .post_token_balances
        .iter()
        .filter(|b| b.mint == mint)
        .filter_map(|b| b.ui_token_amount.amount.parse::<u64>().ok())
        .sum();
    Some(Launch {
        venue: "Meteora DBC".to_string(),
        // Bags co-signs every launch made on it. Other launches can list its wallet without its signature.
        platform: tx.signers().contains(&BAGS_WALLET).then(|| "Bags".to_string()),
        mayhem: false,
        mint,
        name,
        symbol,
        uri,
        bonding_curve: pool,
        user: tx.signers().first()?.to_string(),
        creator,
        created_at: tx.block_time?,
        supply,
        create_sig: tx.signature().to_string(),
        create_slot: tx.slot,
    })
}

/// The config `mint` was launched under, from its create transaction: the account that holds the curve's settings,
/// among them who gets the supply the curve doesn't sell.
pub fn config_of(tx: &Tx, mint: &str) -> Option<String> {
    tx.program_instructions(DBC_PROGRAM)
        .into_iter()
        .find(|(d, a)| d.len() > 8 && CREATES.iter().any(|c| d[..8] == *c) && a.get(3).map(String::as_str) == Some(mint))
        .and_then(|(_, a)| a.first().cloned())
}

/// A DBC config's supply settings, from its account (`PoolConfig`, a fixed layout: MeteoraAg/dynamic-bonding-curve,
/// programs/dynamic-bonding-curve/src/state/config.rs). A config is set once and never changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Claims the trading fees: the wallet that made the config.
    pub fee_claimer: String,
    /// Gets what's left in the curve's vault once it has moved to a pool (`withdraw_leftover`).
    pub leftover_receiver: String,
    /// The supply is minted whole at launch, rather than only what the curve sells.
    pub fixed_supply: bool,
    /// What the curve sells to buyers, and what moves to the pool with the curve's SOL, in raw units.
    pub curve_sells: u64,
    pub to_pool: u64,
    /// What vests to the creator after migration.
    pub vesting: u64,
    /// The supply minted at launch, and after migration's burn (fixed supply only).
    pub supply_before: u64,
    pub supply_after: u64,
}

impl Config {
    pub fn parse(data: &[u8]) -> Option<Config> {
        let at = |o: usize| Reader::new(data.get(o..)?).u64();
        let key = |o: usize| Reader::new(data.get(o..)?).pubkey();
        // The locked vesting: amount per period, cliff, frequency, number of periods, cliff unlock (+ padding).
        let vesting = at(296)?.checked_mul(at(320)?)?.checked_add(at(328)?)?;
        Some(Config {
            fee_claimer: key(40)?,
            leftover_receiver: key(72)?,
            fixed_supply: *data.get(244)? == 1,
            curve_sells: at(256)?,
            to_pool: at(272)?,
            vesting,
            supply_before: at(344)?,
            supply_after: at(352)?,
        })
    }

    /// What the leftover receiver gets once the curve fills, in raw units: everything minted beyond what the curve
    /// sells, what moves to the pool and what vests, less what migration burns (`supply_before - supply_after` at
    /// most), which comes to the supply after migration less those three. A config without a fixed supply mints only
    /// a little beyond what it sells, and burns all of that: nothing.
    pub fn set_aside(&self) -> u64 {
        if !self.fixed_supply {
            return 0;
        }
        self.supply_after.saturating_sub(self.curve_sells.saturating_add(self.to_pool).saturating_add(self.vesting))
    }

    /// `set_aside` and what the curve sells, as shares of the supply after migration. None without a fixed supply,
    /// where the shares depend on how the curve was drawn.
    pub fn shares(&self) -> Option<(f64, f64)> {
        (self.fixed_supply && self.supply_after > 0).then(|| {
            (crate::pct(self.set_aside() as u128, self.supply_after), crate::pct(self.curve_sells as u128, self.supply_after))
        })
    }
}

/// The supply a `withdraw_leftover` in `tx` paid out of the curve's vault, as a transfer from the vault to the
/// receiver: tokens taken without buying them. Empty when `tx` has none for `mint`.
pub fn leftover_withdrawals(tx: &Tx, mint: &str) -> Vec<Transfer> {
    if !tx.succeeded() || !tx.mentions(DBC_PROGRAM) {
        return vec![];
    }
    let paid = tx.token_deltas(mint).get(DBC_POOL_AUTHORITY).copied().unwrap_or(0);
    tx.program_instructions(DBC_PROGRAM)
        .into_iter()
        .filter(|(d, a)| d.len() >= 8 && d[..8] == WITHDRAW_LEFTOVER && a.get(5).map(String::as_str) == Some(mint))
        .filter_map(|(_, a)| a.get(6).cloned())
        .take(1)
        .filter(|_| paid < 0)
        .map(|to| Transfer {
            sig: tx.signature().to_string(),
            time: tx.block_time.unwrap_or(0),
            from: DBC_POOL_AUTHORITY.to_string(),
            to,
            raw: paid.unsigned_abs() as u64,
        })
        .collect()
}

/// Whether a DBC curve has moved to its pool, from the curve's account (`PoolState`): `is_migrated`, after the 8-byte
/// discriminator, the volatility tracker (64), config, creator, mint and both vaults (32 each), the reserves and fees
/// (six u64), the price (u128), the activation point (u64) and the pool type (u8). None for an account too short.
pub fn curve_migrated(data: &[u8]) -> Option<bool> {
    (data.len() >= 416).then(|| data[305] == 1)
}

/// A Meteora DAMM v2 pool account's size, and where its token A starts (after the 8-byte discriminator and its fees).
pub const DAMM_V2_POOL_SIZE: usize = 1112;
pub const DAMM_V2_TOKEN_A: usize = 168;

/// A Meteora DAMM v2 pool (`Pool`, from its IDL): token A, token B, and the price of A in B, raw units, from its
/// square-root price (Q64.64), which sits after both mints, three more addresses, padding, liquidity, fees and the
/// price range.
pub fn damm_v2_price(data: &[u8]) -> Option<(String, String, f64)> {
    if data.len() != DAMM_V2_POOL_SIZE {
        return None;
    }
    let mut r = Reader::new(&data[DAMM_V2_TOKEN_A..]);
    let (a, b) = (r.pubkey()?, r.pubkey()?);
    let sqrt = Reader::new(&data[DAMM_V2_TOKEN_A + 288..]).u128()? as f64 / 2f64.powi(64);
    Some((a, b, sqrt * sqrt))
}
