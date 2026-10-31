//! Streamflow vesting contracts: which account holds a deposit's terms, and what those terms are.
//!
//! A contract's terms live in its metadata account, owned by the Streamflow program. Its layout is `streamLayout` in
//! @streamflow/stream 13.4.0 (`solana/layout.ts`), and the SDK's own offsets agree with it (`STREAM_STRUCT_OFFSET_*`:
//! sender 49, recipient 113, mint 177, closed 671). Every instruction that moves a contract's tokens names that account
//! just before the contract's escrow token account: `create` and `create_v2` at 3 and 4, `create_unchecked` and its
//! v2 at 2 and 3 (3 and 4 with a payer), `withdraw` at 3 and 4. The escrow token account is owned by itself, so the
//! escrow a deposit names (an owner) is that account. A price-based ("aligned") contract is one of these too, made
//! through a proxy program that is its sender and can change its pace.

use crate::pump::Reader;
use crate::tx::Tx;
use serde::{Deserialize, Serialize};

/// The Streamflow vesting program.
pub const PROGRAM: &str = "strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m";

/// The layout's bytes up to `closed`, the last field read.
const MIN_LEN: usize = 672;
/// Past this, the SDK reads an end time as no end (`MAX_SAFE_UNIX_TIME_VALUE`).
const MAX_TIME: u64 = 8_640_000_000_000;

/// A vesting contract's terms, as its metadata account holds them now. Amounts are raw token units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockTerms {
    /// The metadata account.
    pub contract: String,
    /// Who made it, and who it pays.
    pub sender: String,
    pub recipient: String,
    /// What it holds for the recipient, fees taken, and what it has paid out so far.
    pub deposited: u64,
    pub withdrawn: u64,
    /// `cliff_amount` unlocks at `cliff` (no earlier than `start`), then `amount_per_period` each `period` seconds
    /// until `end`, when all of it has.
    pub start: i64,
    pub cliff: i64,
    pub cliff_amount: u64,
    pub period: u64,
    pub amount_per_period: u64,
    /// None when the contract says it never ends.
    pub end: Option<i64>,
    pub cancelable_by_sender: bool,
    pub cancelable_by_recipient: bool,
    /// The sender can hand the payout to another wallet.
    pub transferable_by_sender: bool,
    pub pausable: bool,
    /// Its pace can change after it starts: by its sender, or by the price for an aligned contract.
    pub can_update_rate: bool,
    pub canceled_at: Option<i64>,
    pub closed: bool,
}

impl LockTerms {
    /// Whether `wallet`, which put the tokens in, can get them back before the schedule pays them: it can cancel the
    /// contract (as its sender, or as the recipient when it pays itself), or it can raise the pace of one that pays
    /// itself. A contract already cancelled or closed holds nothing to take back.
    pub fn can_take_back(&self, wallet: &str) -> bool {
        if self.canceled_at.is_some() || self.closed {
            return false;
        }
        let pays_itself = self.recipient == wallet;
        self.cancelable_by_sender
            || (self.cancelable_by_recipient && pays_itself)
            || (self.can_update_rate && pays_itself && self.sender == wallet)
    }
}

/// The terms in the data of Streamflow metadata account `contract`, if it is the contract for `mint` whose escrow is
/// `escrow`: anything else (another layout, another contract, a closed account) gives None, never a guess.
pub fn decode(contract: &str, data: &[u8], mint: &str, escrow: &str) -> Option<LockTerms> {
    if data.len() < MIN_LEN {
        return None;
    }
    let u64_at = |o: usize| Reader::new(data.get(o..)?).u64();
    let time_at = |o: usize| u64_at(o).and_then(|t| i64::try_from(t).ok());
    let key = |o: usize| Reader::new(data.get(o..)?).pubkey();
    let flag = |o: usize| data.get(o).map(|b| *b != 0);
    if key(177)? != mint || key(209)? != escrow {
        return None;
    }
    let end = u64_at(33)?;
    let canceled_at = time_at(25)?;
    Some(LockTerms {
        contract: contract.to_string(),
        sender: key(49)?,
        recipient: key(113)?,
        deposited: u64_at(417)?,
        withdrawn: u64_at(17)?,
        start: time_at(409)?,
        cliff: time_at(441)?,
        cliff_amount: u64_at(449)?,
        period: u64_at(425)?,
        amount_per_period: u64_at(433)?,
        end: (end <= MAX_TIME).then_some(end as i64),
        cancelable_by_sender: flag(457)?,
        cancelable_by_recipient: flag(458)?,
        transferable_by_sender: flag(460)?,
        pausable: flag(539)?,
        can_update_rate: flag(540)?,
        canceled_at: (canceled_at > 0).then_some(canceled_at),
        closed: flag(671)?,
    })
}

/// The metadata account of the contract whose escrow is `escrow` (an owner of `mint`), from a Streamflow instruction
/// in `tx` that names it: the account just before the escrow's token account.
pub fn contract_of(tx: &Tx, mint: &str, escrow: &str) -> Option<String> {
    let account = tx.token_account(escrow, mint)?;
    tx.program_instructions(PROGRAM).into_iter().find_map(|(_, accounts)| {
        let i = accounts.iter().position(|a| a == account)?;
        accounts.get(i.checked_sub(1)?).cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms() -> LockTerms {
        LockTerms {
            contract: "c".into(),
            sender: "dev".into(),
            recipient: "dev".into(),
            deposited: 100,
            withdrawn: 0,
            start: 10,
            cliff: 10,
            cliff_amount: 0,
            period: 1,
            amount_per_period: 1,
            end: Some(110),
            cancelable_by_sender: false,
            cancelable_by_recipient: false,
            transferable_by_sender: false,
            pausable: false,
            can_update_rate: false,
            canceled_at: None,
            closed: false,
        }
    }

    #[test]
    fn who_can_take_a_lock_back() {
        assert!(!terms().can_take_back("dev"), "fixed terms");
        assert!(LockTerms { cancelable_by_sender: true, ..terms() }.can_take_back("dev"));
        // The recipient cancelling gets what it is owed, not the rest: only a contract paying itself comes back whole.
        assert!(LockTerms { cancelable_by_recipient: true, ..terms() }.can_take_back("dev"));
        assert!(!LockTerms { cancelable_by_recipient: true, recipient: "other".into(), ..terms() }.can_take_back("dev"));
        // A sender paying itself can speed it up; a pace set by a price (an aligned contract's proxy) can't be.
        assert!(LockTerms { can_update_rate: true, ..terms() }.can_take_back("dev"));
        assert!(!LockTerms { can_update_rate: true, sender: "proxy".into(), ..terms() }.can_take_back("dev"));
        // Cancelled already: nothing is left in it.
        assert!(!LockTerms { cancelable_by_sender: true, canceled_at: Some(50), ..terms() }.can_take_back("dev"));
    }

    #[test]
    fn another_layout_or_contract_reads_as_nothing() {
        assert_eq!(decode("c", &[0u8; 100], "M", "E"), None);
        assert_eq!(decode("c", &[0u8; 1104], "M", "E"), None);
    }
}
