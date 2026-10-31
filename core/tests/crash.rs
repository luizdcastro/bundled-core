//! $CODEX (DMhs…pump, 2026-10-22): "0% bundled", risk low, and a crash nobody we flagged sold into. Its creator's launch
//! buy, then every trade from 1792679000 to 1792680400 as our parser reads them from chain (`fixtures/crash/codex.json`).

use std::path::PathBuf;
use bundled_core::crash::{crash, find};
use bundled_core::trades::Trade;

const SUPPLY: u64 = 1_000_000_000_000_000;
const CREATOR: &str = "Gkf5Wng71XPgsNsH7wjLFcA5PoZCzz5fHUAouMbtP8xM";

fn codex() -> Vec<Trade> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/crash/codex.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn codex_crash_is_found_from_its_high() {
    let w = find(&codex(), SUPPLY).expect("a crash");
    println!("{w:?}");
    assert!(w.from_cap > 1_000.0 && w.to_cap < 60.0, "{w:?}");
    assert!(w.end - w.start <= 600, "fell in under 10 minutes: {w:?}");
}

#[test]
fn codex_crash_was_sold_by_the_crowd_not_its_insiders() {
    let c = crash(&codex(), SUPPLY, |w| w == CREATOR, |_| false).expect("a crash");
    println!("{c:?}");
    assert!(c.drop_pct > 94.0, "{c:?}");
    // 414 wallets sold 73% of the supply in all; 54 of them bought as much back in it, and net the rest let go of 55%.
    assert!(c.sellers > 350 && c.sold_pct > 50.0 && c.sold_pct < 60.0, "{c:?}");
    assert_eq!(c.insider_sellers, 0);
}

#[test]
fn codex_reads_high_risk_while_it_stays_down() {
    let c = crash(&codex(), SUPPLY, |w| w == CREATOR, |_| false).expect("a crash");
    assert!(c.still_down(), "{c:?}");
    assert_eq!(c.risk_floor(), Some("high"));
}

/// CNfz…ZD3 (2026-10-22): its crash's 14 minutes and its launch's first page. It read "422
/// wallets sold 164% of the supply": wallets bought 112% of the supply in those minutes and sold 164%, the same tokens
/// round and round. Net, 239 wallets let go of 67%.
#[test]
fn cnfz_crash_counts_what_sellers_let_go_of_not_every_sell() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/crash/cnfz.json");
    let trades: Vec<Trade> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let c = crash(&trades, SUPPLY, |_| false, |_| false).expect("a crash");
    println!("{c:?}");
    assert_eq!((c.start, c.end), (1_792_330_928, 1_792_331_790));
    assert!(c.drop_pct > 93.0, "{c:?}");
    assert_eq!(c.sellers, 239);
    assert!((c.sold_pct - 67.2).abs() < 0.1, "{c:?}");
}

/// $TRUMPOCEAN (G4wm…pump, 2026-10-23): its first 100 trades and its crash's 15 minutes, from its report. Its page read
/// "210 wallets sold 105% of the supply": 6rify… bought 0.28% and sold 65% it had been sent, so the tokens other wallets
/// bought in the crash and passed on counted in two wallets' nets. Its sells less its buys, what went back into the
/// pool, were 92%.
#[test]
fn trumpocean_crash_never_reads_over_what_wallets_let_go_of() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/crash/trumpocean.json");
    let trades: Vec<Trade> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let c = crash(&trades, SUPPLY, |_| false, |w| w.starts_with("6rify")).expect("a crash");
    println!("{c:?}");
    assert_eq!((c.start, c.end, c.sellers), (1_792_682_891, 1_792_683_779, 210));
    assert!((c.sold_pct - 91.6).abs() < 0.1, "{c:?}");
    // A group is held to the same: 6rify… alone let go of 65%.
    assert!((c.sniper_sold_pct - 65.4).abs() < 0.1, "{c:?}");
}
