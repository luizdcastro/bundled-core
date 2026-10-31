//! $GTA (EDeL…pump, 2026-10-22): read "0% bundled", insiders 0%, and a 98.7% crash "nobody we flagged" sold into. Its
//! creator bought 6.6% alone at launch and sold it at 55 seconds; at 153 seconds three transactions, signed by 3, 7 and
//! 6 wallets that were funded with the creator 24 seconds before launch, bought 79% of the supply for 85 SOL, the
//! whole curve, and the creator migrated it in the same block. `fixtures/late_bundle/gta.json`: every transaction that
//! succeeded on its bonding curve, from the create to the migration, as `getTransaction` returns them.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use bundled_core::analysis::{analyze, late_bundle_candidates, late_bundle_in, launch_second, Input, Report};
use bundled_core::trades::{trades, Trade};
use bundled_core::Tx;

const MINT: &str = "EDeL3pRZVGTjuF3qEoC8EEhsYy7qcWUDx2VQiL5hpump";
const CREATOR: &str = "6vxdVaedVhouyynFgpMS2tZrRrMV3Qdj2ewy8ta7ztaB";
const CURVE: &str = "64zNSzVhPQ66ANHrPQQs5issQ1kQ2oKcUyzCdmsXs5Eq";
const CREATE: &str = "3nUUkXefJH1jKYdQgF5phimbZW4Xt6zPeVCWbwob23ufw6d9VzEszypjKdmoQMXoAC3duEVWEawNQ9miUz1R4r2q";
const CREATED_AT: i64 = 1_792_690_639;
const SUPPLY: u64 = 1_000_000_000_000_000;

fn gta() -> Vec<Trade> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/late_bundle/gta.json");
    let txs: Vec<Tx> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    txs.iter().flat_map(|t| trades(t, MINT)).collect()
}

fn report(trades: &[Trade], late_bundles: bool) -> Report {
    // Who holds what after these trades: the bundle's wallets still hold all they bought.
    let mut held: HashMap<String, i128> = HashMap::new();
    for t in trades {
        *held.entry(t.wallet.clone()).or_default() += if t.is_buy { t.tokens as i128 } else { -(t.tokens as i128) };
    }
    let holders: Vec<(String, u64)> = held.into_iter().filter(|(_, a)| *a > 0).map(|(w, a)| (w, a as u64)).collect();
    let inp = Input {
        mint: MINT,
        creator: CREATOR,
        create_sig: CREATE,
        created_at: CREATED_AT,
        supply: SUPPLY,
        curve_and_pools: [CURVE.to_string()].into(),
        claim_vaults: HashSet::new(),
        trades,
        roles: [(CREATOR.to_string(), vec!["creator".to_string()])].into(),
        holders: &holders,
        known: HashMap::new(),
        bundled_pct: 0.0,
        proxima: false,
        proxima_later: false,
        router_txs: 0,
        proxima_after_secs: None,
        proxima_wallets: 0,
        offchain_creator: false,
        dev_launches_7d: 1,
        dev_bundled_7d: 0,
        dev_pct: 6.63,
        creator_record: None,
        crew: None,
        crew_wallets: HashMap::new(),
        funding: HashMap::new(),
        first_seen: HashMap::new(),
        block0: vec![],
        transfers: &[],
        streamflow_claims: vec![],
        stream_locks: vec![],
        lock_escrows: HashSet::new(),
        burned: 0,
        set_aside: None,
        declared_bundle: false,
        opening_bundle: false,
        liquidity_pulled: None,
        late_bundles,
        late_bundle_sigs: HashSet::new(),
        mint_check: None,
        launch_funded: HashMap::new(),
    };
    analyze(&inp)
}

#[test]
fn gta_bundle_at_153_seconds_is_a_late_bundle() {
    let r = report(&gta(), true);
    let b = r.summary.late_bundle.as_ref().expect("a late bundle");
    println!("{b:?}");
    assert_eq!((b.after_secs, b.wallets, b.txs, b.creator), (153, 16, 3, false));
    assert!((b.pct - 79.31).abs() < 0.05, "{b:?}");
    assert!((b.sol - 85.0).abs() < 0.1, "{b:?}");
    // Its wallets are insiders: the bundle's role, and why.
    let bundled: Vec<_> = r.wallets.iter().filter(|w| w.reasons.iter().any(|x| x.code == "late_bundle")).collect();
    assert_eq!(bundled.len(), 16);
    assert!(bundled.iter().all(|w| w.roles.iter().any(|r| r == "bundle")));
    assert!((r.summary.insiders_hold_pct - 79.31).abs() < 0.05, "{:?}", r.summary.insiders_hold_pct);
    // A red flag, the risk counts it as bundled, and the timeline tells it after the creator sold.
    assert_eq!(r.signals.iter().find(|s| s.code == "late_bundle").map(|s| s.weight), Some(40));
    assert!(r.risk == "high" || r.risk == "extreme", "{}", r.risk);
    let stages: Vec<_> = r.timeline.iter().map(|s| s.stage).collect();
    assert_eq!(stages, ["launch", "first_second", "first_exit", "late_bundle", "now"]);
}

#[test]
fn gta_before_its_bundle_has_none() {
    // The creator's buy, the snipers' and their sells: each wallet bought in its own transaction.
    let early: Vec<Trade> = gta().into_iter().filter(|t| t.time < CREATED_AT + 150).collect();
    assert!(!early.is_empty());
    let r = report(&early, true);
    assert!(r.summary.late_bundle.is_none());
    assert!(!r.signals.iter().any(|s| s.code == "late_bundle"));
}

#[test]
fn late_bundles_are_read_only_where_a_transaction_buyers_signed_it() {
    // EVM: one signer, and a contract buying for others is a presale or a router.
    let r = report(&gta(), false);
    assert!(r.summary.late_bundle.is_none());
}

/// $KFINE (Qq5c…pump, 2026-10-23): its create event's clock read 05:03:36, the chain timed its whole launch block (the
/// create and its 4-transaction Jito bundle, 74% of the supply) 05:03:37. The late check took that block for a late
/// bundle and stored its 5 transactions: "74% bundled at launch, and 5 wallets bought 74% more together 1 seconds after
/// it", 148% bundled, in a verdict and then a receipt on X. `fixtures/late_bundle/kfine.json`: its transactions from
/// 05:03:36 to 05:03:38 as the history call returns them.
#[test]
fn kfine_launch_bundle_is_never_a_late_one() {
    const MINT: &str = "Qq5cVuXnpYR1tXbwA7HDpdJW4bG6DAJoWA9wc1gpump";
    const CREATE: &str = "5EmLHCn8bVByWAZnK2RZLcDJkqL1YxiGSazWSYt2qwRX6rWxhyBDsmnsnJXt9oWr1dTZwhYHayCrtMbg9n1gnP6T";
    const CREATOR: &str = "ChGH6jaUcVxYhj7Mj3XwXDFzLXno2iNXUkU2MPVYV9z7";
    const CREATED_AT: i64 = 1_792_731_816;
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/late_bundle/kfine.json");
    let txs: Vec<Tx> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let trades: Vec<Trade> = txs.iter().flat_map(|t| trades(t, MINT)).collect();
    let slot_of: HashMap<String, u64> = txs.iter().map(|t| (t.signature().to_string(), t.slot.unwrap())).collect();
    // What the late check kept: the launch's own bundle, the create among it.
    let stored: HashSet<String> = txs.iter().filter(|t| t.slot == Some(460_316_926)).map(|t| t.signature().to_string()).collect();
    assert_eq!(stored.len(), 5);
    let none = HashSet::new();
    // Counted from the event's clock, as before: the launch bundle again, 74%.
    let before = late_bundle_in(&trades, CREATED_AT, CREATED_AT, CREATOR, SUPPLY, &none, &stored, &none, 0.0).map(|(b, _)| b);
    assert!(before.as_ref().is_some_and(|b| (b.pct - 74.24).abs() < 0.05 && b.after_secs == 1), "{before:?}");
    // The launch's second is the create's, as the chain times it; nothing in it is late.
    let second = launch_second(&trades, CREATE, CREATED_AT);
    assert_eq!(second, CREATED_AT + 1);
    assert!(late_bundle_in(&trades, CREATED_AT, second, CREATOR, SUPPLY, &none, &stored, &none, 0.0).is_none());
    assert!(late_bundle_candidates(&trades, &slot_of, second, SUPPLY, 5).iter().all(|s| trades.iter().any(|t| &t.sig == s && t.time > second)));
    // Through the report too: the transactions stored from before, the create's second never counts.
    let inp = Input { create_sig: CREATE, created_at: CREATED_AT, late_bundle_sigs: stored, ..kfine_input(&trades) };
    assert!(analyze(&inp).summary.late_bundle.is_none());
}

fn kfine_input(trades: &[Trade]) -> Input<'_> {
    Input {
        mint: "Qq5cVuXnpYR1tXbwA7HDpdJW4bG6DAJoWA9wc1gpump",
        creator: "ChGH6jaUcVxYhj7Mj3XwXDFzLXno2iNXUkU2MPVYV9z7",
        create_sig: "",
        created_at: 0,
        supply: SUPPLY,
        curve_and_pools: HashSet::new(),
        claim_vaults: HashSet::new(),
        trades,
        roles: HashMap::new(),
        holders: &[],
        known: HashMap::new(),
        bundled_pct: 74.24,
        proxima: false,
        proxima_later: false,
        router_txs: 0,
        proxima_after_secs: None,
        proxima_wallets: 0,
        offchain_creator: false,
        dev_launches_7d: 1,
        dev_bundled_7d: 1,
        dev_pct: 0.14,
        creator_record: None,
        crew: None,
        crew_wallets: HashMap::new(),
        funding: HashMap::new(),
        first_seen: HashMap::new(),
        block0: vec![],
        transfers: &[],
        streamflow_claims: vec![],
        stream_locks: vec![],
        lock_escrows: HashSet::new(),
        burned: 0,
        set_aside: None,
        declared_bundle: false,
        opening_bundle: false,
        liquidity_pulled: None,
        late_bundles: true,
        late_bundle_sigs: HashSet::new(),
        mint_check: None,
        launch_funded: HashMap::new(),
    }
}
