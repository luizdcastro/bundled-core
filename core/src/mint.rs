//! A token's mint account: who can still mint more of it or freeze holders' tokens, and how much exists now.

use crate::pump::Reader;

/// What a mint account says, read as it stands (SPL Token and Token-2022 share these first 82 bytes: a mint authority
/// as a `COption<Pubkey>`, the supply, decimals, whether it's initialized, a freeze authority as a `COption<Pubkey>`).
#[derive(Debug, Clone, PartialEq)]
pub struct MintState {
    /// Can mint more of the coin, whenever it likes; `None` once revoked, as launchpads do at launch.
    pub mint_authority: Option<String>,
    /// Can freeze any holder's tokens so they can't be sold or sent; `None` once revoked.
    pub freeze_authority: Option<String>,
    /// Raw units in existence now.
    pub supply: u64,
}

pub fn parse(data: &[u8]) -> Option<MintState> {
    if data.len() < 82 {
        return None;
    }
    let mut r = Reader::new(data);
    let mint_authority = authority(&mut r)?;
    let supply = r.u64()?;
    r.skip(2)?; // decimals, is_initialized
    let freeze_authority = authority(&mut r)?;
    Some(MintState { mint_authority, freeze_authority, supply })
}

fn authority(r: &mut Reader) -> Option<Option<String>> {
    let set = r.u32()? == 1;
    let key = r.pubkey()?;
    Some(set.then_some(key))
}
