//! pump.fun create detection and event decoding.
//!
//! pump.fun emits Anchor events twice: as `Program data:` log lines and as a
//! self-invoked inner instruction (event CPI). Logs get truncated on busy
//! bundle transactions, so the inner instruction is read first.

use crate::addr::{PUMP_MINT_AUTHORITY, PUMP_PROGRAM, PUMP_TOTAL_SUPPLY};
use crate::tx::Tx;
use base64::Engine;

/// Anchor's `EVENT_IX_TAG`, prefixed to every event-CPI instruction.
const EVENT_IX_TAG: [u8; 8] = [0xe4, 0x45, 0xa5, 0x2e, 0x51, 0xcb, 0x9a, 0x1d];
const CREATE_EVENT: [u8; 8] = [0x1b, 0x72, 0xa9, 0x4d, 0xde, 0xeb, 0x63, 0x76];

#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// Launchpad: "pump.fun", "Meteora DBC", "Bonk" or "LaunchLab".
    pub venue: String,
    /// The site the launch was made on, when it isn't the launchpad itself: "Bags" (on Meteora DBC).
    pub platform: Option<String>,
    /// pump.fun's "mayhem" launch mode.
    pub mayhem: bool,
    pub mint: String,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub bonding_curve: String,
    /// The wallet that sent the create transaction.
    pub user: String,
    /// The creator recorded on the curve. Differs from `user` for "hidden creator" launches.
    pub creator: String,
    pub created_at: i64,
    pub supply: u64,
    pub create_sig: String,
    pub create_slot: Option<u64>,
}

/// pump.fun events in `tx`, each starting with its 8-byte discriminator.
pub fn events(tx: &Tx) -> Vec<Vec<u8>> {
    events_of(tx, PUMP_PROGRAM)
}

/// Anchor events emitted by `program` in `tx`, from event-CPI inner
/// instructions, or from `Program data:` logs when there are none.
pub fn events_of(tx: &Tx, program: &str) -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = tx
        .inner_instruction_data(program)
        .into_iter()
        .filter(|d| d.len() > 16 && d[..8] == EVENT_IX_TAG)
        .map(|d| d[8..].to_vec())
        .collect();
    if out.is_empty() {
        let b64 = base64::engine::general_purpose::STANDARD;
        out = tx
            .logs()
            .iter()
            .filter_map(|l| l.strip_prefix("Program data: "))
            .filter_map(|d| b64.decode(d).ok())
            .filter(|d| d.len() > 8)
            .collect();
    }
    out
}

/// Returns the launch if `tx` is a successful pump.fun create.
pub fn parse_launch(tx: &Tx) -> Option<Launch> {
    if !tx.succeeded() || !tx.mentions(PUMP_MINT_AUTHORITY) {
        return None;
    }
    let ev = events(tx).into_iter().find(|e| e[..8] == CREATE_EVENT)?;
    let mut r = Reader::new(&ev[8..]);
    let name = r.string()?;
    let symbol = r.string()?;
    let uri = r.string()?;
    let mint = r.pubkey()?;
    let bonding_curve = r.pubkey()?;
    let user = r.pubkey()?;
    // The creator and the time were added in 2025. Before, the wallet that sent the create was its creator, and the
    // block's time is the launch's (below): without them every 2024 coin read as no launch at all.
    let creator = r.pubkey().unwrap_or_else(|| user.clone());
    let created_at = r.i64().unwrap_or(0);
    // Reserves and supply were added in later program versions.
    let supply = (|| {
        r.u64()?; // virtual token reserves
        r.u64()?; // virtual sol reserves
        r.u64()?; // real token reserves
        r.u64()
    })()
    .unwrap_or(PUMP_TOTAL_SUPPLY);
    // Then the token program and, in newer versions, the "mayhem" launch mode.
    let mayhem = r.pubkey().and_then(|_| r.u8()).is_some_and(|b| b == 1);
    Some(Launch {
        venue: "pump.fun".to_string(),
        platform: None,
        mayhem,
        mint,
        name,
        symbol,
        uri,
        bonding_curve,
        user,
        creator,
        created_at: tx.block_time.unwrap_or(created_at),
        supply,
        create_sig: tx.signature().to_string(),
        create_slot: tx.slot,
    })
}

/// Borsh reader for the few types pump.fun events use.
pub struct Reader<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, o: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.o..self.o + n)?;
        self.o += n;
        Some(s)
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }

    pub fn i128(&mut self) -> Option<i128> {
        Some(i128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }

    /// Steps over `n` bytes the caller doesn't need.
    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub fn string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        Some(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    pub fn pubkey(&mut self) -> Option<String> {
        Some(bs58::encode(self.take(32)?).into_string())
    }
}
