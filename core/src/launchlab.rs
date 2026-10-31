//! Raydium LaunchLab launches, the program behind Bonk (LetsBonk) and others.
//!
//! A create (`initialize`, `initialize_v2`, ...) emits a `PoolCreateEvent`: the
//! pool, its creator and config, then the token's decimals, name, symbol and
//! metadata URI. The mint isn't in the event, but the create mints the whole
//! supply, so it's the token whose balances in the transaction add up to it.

use crate::addr::{BONK_PLATFORM, LAUNCHLAB_PROGRAM, WRAPPED_SOL};
use crate::pump::{events_of, Launch, Reader};
use crate::tx::Tx;

/// `PoolCreateEvent`.
const POOL_CREATE_EVENT: [u8; 8] = [0x97, 0xd7, 0xe2, 0x09, 0x76, 0xa1, 0x73, 0xae];

/// Returns the launch if `tx` creates a LaunchLab pool.
pub fn parse_launch(tx: &Tx) -> Option<Launch> {
    if !tx.succeeded() || !tx.mentions(LAUNCHLAB_PROGRAM) {
        return None;
    }
    let event = events_of(tx, LAUNCHLAB_PROGRAM)
        .into_iter()
        .find(|e| e.len() > 8 && e[..8] == POOL_CREATE_EVENT)?;
    let mut r = Reader::new(&event[8..]);
    let pool = r.pubkey()?;
    let creator = r.pubkey()?;
    let _config = r.pubkey()?;
    let _decimals = r.u8()?;
    let name = r.string()?;
    let symbol = r.string()?;
    let uri = r.string()?;
    // The new token: the create's base mint, the account after its pool (pool state, base mint, quote mint). The coin
    // with the most tokens in the transaction's balances used to stand in for it, until a creator bought through a
    // route that swapped SOL into BONK, the curve's quote, first: BONK outweighed the new coin, and its own address was
    // stored as a Bonk launch ($LOULOU, 2026-10-16). That rule stays for a create whose accounts don't show the pool.
    let balances = &tx.meta.as_ref()?.post_token_balances;
    let held = |m: &str| -> u64 {
        balances
            .iter()
            .filter(|b| b.mint == m)
            .filter_map(|b| b.ui_token_amount.amount.parse::<u64>().ok())
            .sum()
    };
    // The buy names the pool too, followed by a token account, not a mint: only a mint counts.
    let mints = tx.mints();
    let base = tx
        .program_instructions(LAUNCHLAB_PROGRAM)
        .into_iter()
        .find_map(|(_, accounts)| {
            let at = accounts.iter().position(|a| *a == pool)?;
            accounts.get(at + 1).filter(|m| mints.contains(&m.as_str())).cloned()
        });
    let (mint, supply) = match base {
        Some(m) => {
            let supply = held(&m);
            (m, supply)
        }
        None => tx
            .mints()
            .into_iter()
            .filter(|m| *m != WRAPPED_SOL)
            .map(|m| (m.to_string(), held(m)))
            .max_by_key(|(_, total)| *total)?,
    };
    let venue = if tx.mentions(BONK_PLATFORM) { "Bonk" } else { "LaunchLab" };
    Some(Launch {
        venue: venue.to_string(),
        platform: None,
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
