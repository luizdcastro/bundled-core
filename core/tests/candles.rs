//! Candles from execution-priced trades: stray prints don't draw peaks, real moves stay.

use bundled_core::trades::{candles, Trade};

const SUPPLY: u64 = 1_000_000_000_000_000;

/// A trade at `cap` SOL of market cap: 1M tokens of reserves against `cap / 1000` SOL, with 1B supply.
fn trade(time: i64, cap: f64, sol: f64, generic: bool) -> Trade {
    Trade {
        sig: format!("s{time}-{cap}"),
        time,
        wallet: "w".into(),
        is_buy: true,
        lamports: (sol * 1e9) as u64,
        tokens: 1,
        virtual_sol: (cap / 1000.0 * 1e9) as u64,
        virtual_tokens: 1_000_000_000_000,
        pool: None,
        generic,
        quote: None,
    }
}

#[test]
fn a_stray_print_sets_no_price_but_keeps_its_volume() {
    // One fill 700 times the market in a thin pool, between ordinary trades.
    let caps = [12_000.0, 12_100.0, 12_050.0, 8_500_000.0, 12_200.0, 12_150.0, 12_300.0];
    let t: Vec<Trade> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| trade(i as i64, c, 1.0, true))
        .collect();
    let c = candles(&t, SUPPLY, 0, 60);
    assert_eq!(c.len(), 1);
    assert!(c[0].2 < 12_400.0, "high {}", c[0].2);
    assert!((c[0].5 - 7.0).abs() < 1e-9, "volume {}", c[0].5);
}

#[test]
fn a_stray_print_low_is_left_out_too() {
    let caps = [12_000.0, 12_100.0, 15.0, 12_200.0, 12_150.0];
    let t: Vec<Trade> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| trade(i as i64, c, 1.0, true))
        .collect();
    assert!(candles(&t, SUPPLY, 0, 60)[0].3 > 11_000.0);
}

#[test]
fn a_real_move_stays() {
    // A 10x run that holds: every trade after the jump agrees with it.
    let caps = [100.0, 110.0, 105.0, 1_000.0, 1_050.0, 1_100.0, 1_080.0];
    let t: Vec<Trade> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| trade(i as i64 * 60, c, 1.0, true))
        .collect();
    let c = candles(&t, SUPPLY, 0, 60);
    assert!((c[3].4 - 1_000.0).abs() < 1.0, "close {}", c[3].4);
    assert!(c.iter().map(|c| c.2).fold(0.0, f64::max) >= 1_100.0 - 1.0);
}

#[test]
fn reserve_priced_trades_are_never_left_out() {
    // A launch's sniper spike on the curve is real, and reserves price it exactly.
    let caps = [30.0, 31.0, 400.0, 32.0, 33.0];
    let t: Vec<Trade> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| trade(i as i64, c, 1.0, false))
        .collect();
    assert!((candles(&t, SUPPLY, 0, 60)[0].2 - 400.0).abs() < 1.0);
}

#[test]
fn too_few_neighbours_to_judge() {
    // Two trades can't tell a stray print from a move.
    let t = vec![trade(0, 100.0, 1.0, true), trade(1, 5_000.0, 1.0, true)];
    assert!((candles(&t, SUPPLY, 0, 60)[0].2 - 5_000.0).abs() < 1.0);
}

/// A trade at `cap` that sells or buys `tokens`.
fn sized(time: i64, cap: f64, tokens: u64, is_buy: bool) -> Trade {
    Trade {
        tokens,
        is_buy,
        ..trade(time, cap, 0.5, true)
    }
}

#[test]
fn a_dump_of_most_of_the_supply_stays() {
    // The creator sells 80,000 times what anyone else traded, for a fraction of a SOL, at the end: a real crash.
    let t = vec![
        sized(0, 6_800.0, 10_000, true),
        sized(1, 6_900.0, 10_000, true),
        sized(2, 6_850.0, 10_000, false),
        sized(3, 11.0, 800_000_000, false),
        sized(4, 0.04, 800_000_000, false),
    ];
    let c = candles(&t, SUPPLY, 0, 60);
    assert!(c[0].4 < 1.0, "close {}", c[0].4);
}

#[test]
fn a_trade_far_off_both_sides_is_stray_whatever_its_size() {
    // A big sell priced 14 times every trade before and after it: SOL from elsewhere in the transaction.
    let t = vec![
        sized(0, 5_200.0, 10_000, true),
        sized(1, 5_250.0, 10_000, true),
        sized(2, 71_000.0, 900_000, false),
        sized(3, 5_300.0, 10_000, true),
        sized(4, 5_220.0, 10_000, false),
    ];
    assert!(candles(&t, SUPPLY, 0, 60)[0].2 < 6_000.0);
}

#[test]
fn a_good_trade_between_bad_fills_stays_and_the_bad_ones_go() {
    // Super Inu, 3 hours in: a 1 SOL buy at the market, right after two buys priced near zero.
    let caps = [15_000.0, 15_100.0, 424.0, 430.0, 17_000.0, 15_200.0, 15_300.0, 15_250.0];
    let t: Vec<Trade> = caps
        .iter()
        .enumerate()
        .map(|(i, &c)| trade(i as i64 * 60, c, 1.0, true))
        .collect();
    let c = candles(&t, SUPPLY, 0, 60);
    assert!(c.iter().all(|c| c.3 > 10_000.0), "{c:?}");
    assert!((c[4].4 - 17_000.0).abs() < 1.0, "close {}", c[4].4);
}

#[test]
fn a_big_buy_paid_in_a_mispriced_coin_is_stray() {
    // Super Inu at 17:15: a "32.8 SOL" buy of 440B tokens paid in an obscure coin, at 5 times every trade around it.
    let mut t: Vec<Trade> = [1.66, 1.69, 1.64, 1.65]
        .iter()
        .enumerate()
        .map(|(i, &c)| sized(i as i64, c * 1e6, 10_000, true))
        .collect();
    t.push(sized(4, 8.79e6, 440_000, true));
    t.extend(
        [1.55, 1.65, 1.67, 1.62]
            .iter()
            .enumerate()
            .map(|(i, &c)| sized(5 + i as i64, c * 1e6, 10_000, false)),
    );
    assert!(candles(&t, SUPPLY, 0, 60)[0].2 < 2e6);
}

#[test]
fn small_trades_set_the_price_only_when_a_candle_has_nothing_bigger() {
    // A 0.027 SOL buy whose fees make it read 2.9 times the market, beside bigger trades at the market.
    let t = vec![
        trade(0, 1_800.0, 7.0, true),
        trade(1, 5_260.0, 0.027, true),
        trade(2, 1_820.0, 12.6, true),
    ];
    assert!(candles(&t, SUPPLY, 0, 60)[0].2 < 1_900.0);
    // With only small trades, they still draw the candle.
    let t = vec![trade(0, 1_800.0, 0.02, true), trade(1, 1_850.0, 0.03, true)];
    assert!((candles(&t, SUPPLY, 0, 60)[0].4 - 1_850.0).abs() < 1.0);
}

#[test]
fn small_sells_after_a_dump_close_the_candle() {
    // A creator dumps for 0.52 SOL at an average 42; small sells right after, in the same minute, show the price is 0.
    let t = vec![
        trade(0, 30_000.0, 0.4, true),
        trade(10, 42.0, 0.52, false),
        trade(20, 0.16, 0.032, false),
    ];
    let c = candles(&t, SUPPLY, 0, 60);
    assert!((c[0].4 - 0.16).abs() < 0.01, "close {}", c[0].4);
    assert!(c[0].3 <= c[0].4 && c[0].2 >= c[0].1);
}
