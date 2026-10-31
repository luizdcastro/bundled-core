//! Pure detection logic: no network, no database. Everything here is tested
//! against real transactions in `fixtures/`.

pub mod addr;
pub mod b58;
pub mod analysis;
pub mod bundle;
pub mod crash;
pub mod dbc;
pub mod funding;
pub mod labels;
pub mod launchlab;
pub mod mint;
pub mod pnl;
pub mod pump;
pub mod streamflow;
pub mod trades;
pub mod tx;

pub use bundle::{analyze_bundle, count_once, signer_buys, signer_sells, BundleAnalysis, Buy, Move};
pub use pump::{parse_launch, Launch};
pub use tx::Tx;

/// The launch, if `tx` creates a token on a launchpad we know.
pub fn parse_create(tx: &Tx) -> Option<Launch> {
    pump::parse_launch(tx)
        .or_else(|| dbc::parse_launch(tx))
        .or_else(|| launchlab::parse_launch(tx))
}

/// Share of `supply` as a percentage, the unit every API field uses.
pub fn pct(raw: u128, supply: u64) -> f64 {
    if supply == 0 {
        return 0.0;
    }
    raw as f64 / supply as f64 * 100.0
}
