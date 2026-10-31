//! Checks our numbers against the two isitbundled reports captured on
//! 2026-10-13 (`original/api-samples/curl/analyze-stream-report-*.json`).

use std::path::PathBuf;
use bundled_core::{analyze_bundle, count_once, parse_launch, pump, signer_buys, signer_sells, Move, Tx};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn load_tx(sig: &str) -> Tx {
    let raw = std::fs::read_to_string(fixtures().join(format!("tx/{sig}.json"))).unwrap();
    let rpc: serde_json::Value = serde_json::from_str(&raw).unwrap();
    serde_json::from_value(rpc["result"].clone()).unwrap()
}

fn bundle_sigs(create_sig: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(fixtures().join(format!("jito/{create_sig}.json"))).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    serde_json::from_value(v[0]["txSignatures"].clone()).unwrap()
}

struct Expect {
    create_sig: &'static str,
    mint: &'static str,
    creator: &'static str,
    bundle_pct: f64,
    creator_pct: f64,
    bundle_wallets: usize,
    proxima: bool,
}

fn check(e: Expect) {
    let txs: Vec<Tx> = bundle_sigs(e.create_sig).iter().map(|s| load_tx(s)).collect();
    let launch = parse_launch(&txs[0]).expect("first bundle tx is the create");
    assert_eq!(launch.mint, e.mint);
    assert_eq!(launch.creator, e.creator);
    assert_eq!(launch.supply, 1_000_000_000_000_000);
    assert_eq!(launch.create_sig, e.create_sig);

    let buys: Vec<_> = txs.iter().flat_map(|t| signer_buys(t, &launch.mint)).collect();
    let a = analyze_bundle(&launch.creator, launch.supply, &buys);
    assert!(
        (a.total_pct - e.bundle_pct).abs() < 0.01,
        "bundle {} vs {}",
        a.total_pct,
        e.bundle_pct
    );
    assert!(
        (a.creator_pct - e.creator_pct).abs() < 0.01,
        "creator {} vs {}",
        a.creator_pct,
        e.creator_pct
    );
    assert_eq!(a.wallets.len(), e.bundle_wallets);
    assert_eq!(bundled_core::bundle::uses_proxima(&txs), e.proxima);
}

#[test]
fn proxima_sample_matches_isitbundled() {
    check(Expect {
        create_sig: "2Kwkdw6xFPpP68eYPbYt4tGvPbdkGPf2qP5ZJGGSmMo6PrEU544ebu74XHX5uLwZeA9SFSTizChEs5fHj5sesj4N",
        mint: "ESzZ5k3GDeeFJZxdsaqHUn9MKD6uMG1HYbBHHhv5pump",
        creator: "4xyTayB1AtR9onwdwcTf6ZF85JdVEiQrasgKEEfaRPcx",
        bundle_pct: 51.9999,
        creator_pct: 2.0,
        bundle_wallets: 4,
        // Proxima's router shows up in later trades, not in the create bundle.
        proxima: false,
    });
}

#[test]
fn sumo_sample_matches_isitbundled() {
    // The bundle buys out the curve and migrates to PumpSwap in the same
    // bundle. The pool's 20.69% must not count as a buy.
    check(Expect {
        create_sig: "2o9CB3BmTtFLrHEU81K8zrESTLiqZh1W8Vo3auFgumXQ2RTi39R6YgGrcczNhD8EbQda6vbhutpr3vKtVzkUYq3p",
        mint: "9QENqoT6kX45g9oXnDXtog1e7xrjqAPTjiWLopJJpump",
        creator: "CoeakbLhZZaZiExchLvNFFi92reBjYpSuiF9WaE1DW3s",
        bundle_pct: 90.2304,
        creator_pct: 0.9796,
        bundle_wallets: 11,
        proxima: false,
    });
}

#[test]
fn reads_events_from_inner_instructions_when_logs_are_truncated() {
    let tx = load_tx("2o9CB3BmTtFLrHEU81K8zrESTLiqZh1W8Vo3auFgumXQ2RTi39R6YgGrcczNhD8EbQda6vbhutpr3vKtVzkUYq3p");
    assert!(tx.logs().iter().any(|l| l.contains("truncated")));
    assert!(pump::events(&tx).len() >= 2);
    assert_eq!(parse_launch(&tx).unwrap().name, "Adix");
}

#[test]
fn finds_where_the_creators_buy_landed() {
    // The token account the dump check reads later, in the create transaction.
    let tx = load_tx("2Kwkdw6xFPpP68eYPbYt4tGvPbdkGPf2qP5ZJGGSmMo6PrEU544ebu74XHX5uLwZeA9SFSTizChEs5fHj5sesj4N");
    let mint = "ESzZ5k3GDeeFJZxdsaqHUn9MKD6uMG1HYbBHHhv5pump";
    assert_eq!(
        tx.token_account("4xyTayB1AtR9onwdwcTf6ZF85JdVEiQrasgKEEfaRPcx", mint),
        Some("33p3nR3ywAe5sAhzJUD2MD8QYSCHTXmfVPWxGaPVKn7y")
    );
    // Sumo's create loads it from a lookup table.
    let tx = load_tx("2o9CB3BmTtFLrHEU81K8zrESTLiqZh1W8Vo3auFgumXQ2RTi39R6YgGrcczNhD8EbQda6vbhutpr3vKtVzkUYq3p");
    let mint = "9QENqoT6kX45g9oXnDXtog1e7xrjqAPTjiWLopJJpump";
    assert_eq!(
        tx.token_account("CoeakbLhZZaZiExchLvNFFi92reBjYpSuiF9WaE1DW3s", mint),
        Some("C27jZXaSqtJsBMQSByaDFbwfjUHyFXgx9kxSqEe8aoz8")
    );
    // A wallet that bought nothing holds none.
    assert_eq!(tx.token_account("4xyTayB1AtR9onwdwcTf6ZF85JdVEiQrasgKEEfaRPcx", mint), None);
}

#[test]
fn a_plain_buy_is_not_a_launch() {
    let tx = load_tx("2k3ovN6pq1NMNSE7Za74PpXAR88JVhA6yLMmo6PNkRcpAe28KnbxayKuEkBmgchRqDBhzhtptQx5RqLL4Yjh69Pk");
    assert!(parse_launch(&tx).is_none());
}

#[test]
fn reads_a_2024_create_without_creator_or_time() {
    // pump.fun's create event named no creator and no time until 2025: the wallet that sent it is the creator, and the
    // block's time the launch's. Before this, every such coin read as "Not a launch" (GYKm…pump, 2026-10-21).
    let tx = load_tx("b6vk1sX2dJVps7LDojY3Lzh1GxexhpH5hBuM8Zpzp8wL5aunmSJaMNBZuBULx6uCtjibuJwiV3omb7qnM6rZxop");
    let l = parse_launch(&tx).expect("a 2024 pump.fun create is a launch");
    assert_eq!(l.mint, "GYKmdfcUmZVrqfcH1g579BGjuzSRijj3LBuwv79rpump");
    assert_eq!(l.creator, l.user);
    assert_eq!(l.user, tx.signers()[0]);
    assert_eq!(l.created_at, tx.block_time.unwrap());
    assert_eq!(l.supply, 1_000_000_000_000_000);
    assert!(!l.mayhem);
}

const GX1J: &str = "GX1j6VyStDxP2k6L7pHk3PkeSf96qxCgut6VMxN7pump";

#[test]
fn parses_version_1_transactions() {
    // Most pump.fun traffic uses the v1 transaction format since October 2026.
    // This one is a sell: the balance drops, so it isn't a buy.
    let tx = load_tx("2y48SGboxPyC3YbYUwkwgsvPAio5YCBUiVK2sqCKU9XWLE3XgCEFaSVG36s3mNLx4mpmpViHnvZueuoo3kj7caob");
    let seller = "2H6DwrqAsETL1PjrT5KAX3MAELx71FuDkKVZj18PaeJf";
    assert!(tx.succeeded());
    assert!(tx.signers().contains(&seller));
    assert_eq!(tx.token_deltas(GX1J)[seller], -5_507_504_411_574);
    assert!(signer_buys(&tx, GX1J).is_empty());
}

#[test]
fn failed_transactions_never_count_as_buys() {
    let tx = load_tx("3TcJMj51av7UrEBjG5ngqW7URQUciUVs4pkBB45MCNfpoJNH7mcBCgi9BXV9iZDLgBPmWGwbKAU5xENzNdGHexjf");
    assert!(!tx.succeeded());
    assert!(signer_buys(&tx, GX1J).is_empty());
}

#[test]
fn decodes_curve_trades_with_market_cap() {
    use bundled_core::trades::{candles, trades};
    let mint = "ESzZ5k3GDeeFJZxdsaqHUn9MKD6uMG1HYbBHHhv5pump";
    let create = load_tx("2Kwkdw6xFPpP68eYPbYt4tGvPbdkGPf2qP5ZJGGSmMo6PrEU544ebu74XHX5uLwZeA9SFSTizChEs5fHj5sesj4N");
    let t = trades(&create, mint);
    assert_eq!(t.len(), 1);
    assert!(t[0].is_buy);
    assert_eq!(t[0].wallet, "4xyTayB1AtR9onwdwcTf6ZF85JdVEiQrasgKEEfaRPcx");
    assert_eq!(t[0].tokens, 20_000_000_000_000);
    assert_eq!(t[0].lamports, 569_800_570);
    // 30.5698 SOL of virtual reserves against 1.053B virtual tokens, 1B supply.
    let cap = t[0].market_cap(1_000_000_000_000_000);
    assert!((cap - 29.0311).abs() < 0.001, "{cap}");

    // The whole bundle lands in one 5-minute candle that only goes up.
    let all: Vec<_> =
        bundle_sigs("2Kwkdw6xFPpP68eYPbYt4tGvPbdkGPf2qP5ZJGGSmMo6PrEU544ebu74XHX5uLwZeA9SFSTizChEs5fHj5sesj4N")
            .iter()
            .flat_map(|s| trades(&load_tx(s), mint))
            .collect();
    assert_eq!(all.len(), 5);
    let c = candles(&all, 1_000_000_000_000_000, all[0].time, 300);
    assert_eq!(c.len(), 1);
    assert!(c[0].4 > c[0].1, "close above open");
    assert!((c[0].5 - all.iter().map(|t| t.sol()).sum::<f64>()).abs() < 1e-9);
}

#[test]
fn decodes_pumpswap_trades_after_migration() {
    use bundled_core::trades::trades;
    let mint = "7VuPxGPmSSifqJLxRnqFD23Lr5WFJitxMqBG3V8jpump";
    let buy = load_tx("3KMNZCahCEHgMRjQk7fjCgnsixRMBZurrYmGNJznTei97GayzRDkT2MVFwEGyHp17GzJMoQaYZYQkCu99SZhc2Cj");
    let t = trades(&buy, mint);
    assert_eq!(t.len(), 1);
    assert!(t[0].is_buy);
    assert_eq!(t[0].tokens, 281_150_519_079);
    // Pool reserves after the trade match the pool's token balances, plus the 17.58 SOL of virtual reserves this
    // boosted pool prices with: the sell below then comes out of the swap formula to the lamport.
    assert_eq!(t[0].virtual_tokens, 128_717_344_008_606);
    assert_eq!(t[0].virtual_sol, 120_720_010_923 + 17_580_134_157);

    let sell = load_tx("5P9FHRPZ4tyvdCi9PVmTySMMWk17q1T1ATM64nPFYjVCZWWPaTiz3U3VvXtFAk39JiKqnmfvpMsCfyiCKxHenzTA");
    let t = trades(&sell, mint);
    assert_eq!(t.len(), 1);
    assert!(!t[0].is_buy);
    assert_eq!(t[0].virtual_tokens, 128_998_494_527_685);
    assert_eq!(t[0].virtual_sol, 120_417_986_047 + 17_580_134_157);
    let cap = t[0].market_cap(1_000_000_000_000_000);
    assert!(cap > 1060.0 && cap < 1080.0, "{cap}");
}

/// $BUNDLED (Fm46…pump): 9k6F…kB1 bought 11.8% and approved 7QHq…YXc in the same transaction, and 7QHq…YXc sold it
/// from 9k6F…kB1's account the next block. pump.fun's event names 7QHq…YXc; the sell is 9k6F…kB1's. Captured 2026-10-24.
#[test]
fn a_delegated_sell_is_the_holders() {
    use bundled_core::trades::{trades, transfers};
    let mint = "Fm46kSM8KW18abKcuTR8RB16jrhQFXKpUMME5azKpump";
    let buyer = "9k6FrGQC8oECUKp8d3rB7JvBiTtb6nsReUB5CGWzWkB1";
    let buy = trades(&load_tx("2YN6pYLEaWNjS9s1omcsrhBqmAi4Mu56jCze3PdPHsaDUqAUPVSFd9mjv62YewQt8NxiK94MLib6qQETqQYyMrnp"), mint);
    assert_eq!(buy.len(), 1);
    assert!(buy[0].is_buy);
    assert_eq!(buy[0].wallet, buyer);
    let tx = load_tx("2bwxLoPMVzrZ3DnUXrPLmTUnGpPsPdsGWozEFNBJGWb2zcpfWXEc9zn32pFGqxCNRrKRiTAzA3AQrMHgHdzVytMo");
    let sell = trades(&tx, mint);
    assert_eq!(sell.len(), 1);
    assert!(!sell[0].is_buy);
    assert_eq!(sell[0].wallet, buyer);
    assert_eq!(sell[0].tokens, buy[0].tokens);
    assert!(transfers(&tx, mint).is_empty());
}

/// $1 (Trump $1 Coin): its pump.fun curve and PumpSwap pool are quoted in TRUMP, and it's the quote of other coins'
/// pools. Captured 2026-10-16.
const ONE: &str = "2bCCHQadi4QeHWPKG3w13siCyWWQPgkFtniouTqVpump";
const TRUMP: &str = "6p6xgHyF7AeE6TZkSmFsko444wqoP15icUSqi2jfGiPN";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

#[test]
fn reads_a_curve_quoted_in_another_token() {
    use bundled_core::trades::{price_quotes, trades};
    // $1's create: the creator's bundle buys 14.5% for 276.3 TRUMP. The SOL fields are 0; the quote fields carry it.
    let mut t = trades(&load_tx("5n3XfPSFvhCQxbWHaUupqMCC46UPKmym3F2TCXiDZwYHR6UpnzHo3kYwWPNtAAsQziipm6siY1YWUWC2jRyPbWQC"), ONE);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].tokens, 144_932_534_473_410);
    let q = t[0].quote.clone().expect("quoted in TRUMP");
    assert_eq!((q.mint.as_str(), q.amount, q.reserves), (TRUMP, 276_323_227, 2_045_743_716));
    // Not valued yet: nothing in SOL, so the chart can't take it for a price of 0.
    assert_eq!((t[0].lamports, t[0].virtual_sol), (0, 0));

    // At 0.0177 SOL a TRUMP: 4.89 SOL paid, and a launch cap of 2,204 TRUMP, 39 SOL.
    price_quotes(&mut t, |m| (m == TRUMP).then_some((6, 0.0177)));
    assert_eq!(t[0].lamports, 4_890_921_118);
    let cap = t[0].market_cap(1_000_000_000_000_000);
    assert!((cap - 2_204.305 * 0.0177).abs() < 0.01, "{cap}");
    // Valued once: a later price leaves it as it is.
    price_quotes(&mut t, |_| Some((6, 1.0)));
    assert_eq!(t[0].lamports, 4_890_921_118);

    // A USDC curve (6 decimals) and a TSLAx one (8 decimals).
    let t = trades(&load_tx("B1fgfcfRQDSKRiVBXvtdPCnWDspUUVLjjkut8jhcWaSYvhB4aMYEd1dsbSesrg7A8n6qaCgxcgTdYMLwEKRhAVg"), "CzhNKJVjDXiBGw8YfEWVgfVVfaTueGX3UWhn5fcTpump");
    let q = t[0].quote.as_ref().expect("quoted in USDC");
    assert_eq!((q.mint.as_str(), q.amount, q.reserves), (USDC, 9_899_008, 4_301_899_008));
    let t = trades(&load_tx("2LmPoL7RktrgEq4cT188xovMCYrtDrFSLPm7c9xGRh52BmPrMZoJR8yVxEzjhPVb8GTZJxiJvtNFLeoR8apy8y7U"), "9uyX1yg4utveNF8jweEuwKjQty5m88oSHDcjBPiBpump");
    assert_eq!(t[0].quote.as_ref().map(|q| q.mint.as_str()), Some("XsDoVfqeBukxuZHWhdvWHBhgEHjGNst4MLodqsJHzoB"));
}

#[test]
fn reads_pumpswap_pools_quoted_in_another_token() {
    use bundled_core::trades::trades;
    // $1/TRUMP is a boosted pool: it prices as if it held 1,035.9 TRUMP more than it does.
    let buy = trades(&load_tx("gEAFYqhi3jPfNPmPpnkke9K2PvXz46zeFcr7xTQUxwNmiBdDUfs5ZNVYkymWkZC1rTrx1TxQm3AUugtsEHK8E8n"), ONE);
    assert_eq!(buy.len(), 1);
    assert!(buy[0].is_buy);
    let q = buy[0].quote.as_ref().expect("quoted in TRUMP");
    assert_eq!(q.mint, TRUMP);
    assert_eq!(buy[0].pool.as_deref(), Some("AQN7UkKYSnaZns61dVW2Q6hQmm7WiFSAyjBon6FVwhGf"));
    let sell = trades(&load_tx("34HC1NZZQfFFWUocM4QcvfUi58TP9ruUtBAfcKBdHNPF7B1cokDAG5M2NrnvchG3ivLkn1KwL8ocZbypVjPiy5q6"), ONE);
    assert!(!sell[0].is_buy);
    assert_eq!(sell[0].quote.as_ref().map(|q| q.mint.as_str()), Some(TRUMP));
}

#[test]
fn a_pool_trades_only_its_base_token() {
    use bundled_core::trades::trades;
    // One route through three pools: 66LP/SOL, then $1/66LP, then $1/TRUMP.
    let route = load_tx("3CmRsBozSPLoiG15iJqhfxhRLeKNGBxAsHpn3sVPRvS6o2qo2tY6Gw1iBtAb9r9Xgs6a7HrjDmzykJTofY1nJvBT");
    let one = trades(&route, ONE);
    let quotes: Vec<_> = one.iter().map(|t| t.quote.as_ref().map(|q| &q.mint[..4])).collect();
    assert_eq!(quotes, [Some("66LP"), Some("6p6x")]);
    // 66LP is the base of the first pool only: in $1/66LP it's what was paid, not what was traded.
    let lp = trades(&route, "66LPmREAYBqPeyidL7LMW7RaJjKbfADbuUMsqatkpump");
    assert_eq!(lp.len(), 1);
    assert_eq!(lp[0].pool.as_deref(), Some("E9pd39bsqnQ62FVB4o7HMm8RZ8Hw2zG5wNsZDVgfBcJJ"));
    assert!(lp[0].quote.is_none(), "quoted in SOL");
}

#[test]
fn a_boosted_sol_pool_prices_with_its_virtual_reserves() {
    use bundled_core::trades::trades;
    // The pool holds 7,734.7 SOL and prices as if it held 17.58 more.
    let t = trades(&load_tx("MYfWoVR3q4Tm4aeK6jq8bQGzVZNUtfp37nkGfrkCF62bPkkwNqoRngtvUURnqHfsiAgNAKSZNLZsN9RrUX5Xtkt"), "BNj1neW1sZYxHpYX2dPm6o1VwJFEpbxy3X6tjAqZpump");
    assert_eq!(t.len(), 1);
    assert!(t[0].quote.is_none());
    assert_eq!(t[0].lamports, 390_690);
    assert_eq!(t[0].virtual_sol, 7_752_318_237_276);
}

#[test]
fn prices_trades_on_other_venues_in_the_token_paid() {
    use bundled_core::trades::{price_quotes, trades};
    const USD1: &str = "USD1ttGY1N17NEEHLmELoaybftRBUSErhqYiQzvEmuB";
    const BONK_COIN: &str = "44Sw1p1CowmCrryE9BWq7UxS3NqQYu2y3NGQjpGfcomu";
    // A Meteora DBC coin bought with 3.77 USDC: no SOL changed hands, so it had read as no trade at all. Since
    // 2026-10-16 it's read from the curve's swap event, priced by the curve in USDC.
    let mut t = trades(&load_tx("3VjXJeSJFzfYpdJCESh86QK8zNj3tKmi9wxj4uDHgLBkosfxa1XHdK46Z9RhbkE3etAb8sh5HnUkgRq5h2d25YUT"), "FKH4dfCBa79Sn6Vv67NhQPasK97Y9NJ9qimz2KmGmoon");
    assert_eq!(t.len(), 1);
    assert!(!t[0].generic && t[0].is_buy);
    assert_eq!(t[0].tokens, 106_431_199_122);
    let q = t[0].quote.clone().expect("paid in USDC");
    assert_eq!((q.mint.as_str(), q.amount), (USDC, 3_767_881));
    price_quotes(&mut t, |m| (m == USDC).then_some((6, 0.01)));
    assert_eq!(t[0].lamports, 37_678_810);

    // A Bonk coin sold for 286.6 USD1.
    let t = trades(&load_tx("2hnmP2ao18k8Noa4ZfAZFm7RDp6XizpEoXaupYFxHfXuBNhU75WsT5HzyDF8tugyqWhYsS7yHbn6YkFVVZJ1Nh3J"), BONK_COIN);
    assert_eq!(t.len(), 1);
    assert!(!t[0].is_buy);
    assert_eq!(t[0].quote.as_ref().map(|q| (q.mint.as_str(), q.amount)), Some((USD1, 286_564_990)));
    // One bought with 1.06 USD1 had read as a 0.002 SOL buy: the rent of the token account it opened.
    let t = trades(&load_tx("5BoGMYq73eSU36LMkewdsmhbSrDUkmSuxj7v7EeEWbJaCQAebXdiXvFnyxdNbeMgAJpPRNW5xbzXAHcNoYAoxKxx"), BONK_COIN);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].quote.as_ref().map(|q| (q.mint.as_str(), q.amount)), Some((USD1, 1_063_888)));
    // Bought with 5 SOL through a route that swapped it into USD1 first: since LaunchLab trades are read from the
    // curve's own event, it's the 524.96 USD1 the curve took, as every other trade of this coin.
    let t = trades(&load_tx("2W3CYY3GiFewS6KFxXDyhjGwkSibxFeQorBtnG5u7YLX3aDZtZ8c4RbMDdAR8BYz9vk5ygvtk2fw7PLuA6cnWuD5"), BONK_COIN);
    assert_eq!(t.len(), 1);
    assert!(!t[0].generic);
    assert_eq!(t[0].quote.as_ref().map(|q| (q.mint.as_str(), q.amount)), Some((USD1, 524_964_003)));
}

#[test]
fn a_stablecoin_is_never_priced_in_what_it_bought() {
    use bundled_core::trades::trades;
    // The USDC that bought the Meteora coin: a wallet's PnL counts the coin, not its dollars, so no trade comes of it.
    let tx = load_tx("3VjXJeSJFzfYpdJCESh86QK8zNj3tKmi9wxj4uDHgLBkosfxa1XHdK46Z9RhbkE3etAb8sh5HnUkgRq5h2d25YUT");
    assert!(trades(&tx, USDC).iter().all(|t| t.quote.is_none()));
}

/// A DBC launch made on Bags, which co-signs its creates. Captured 2026-10-16.
#[test]
fn tags_a_bags_launch() {
    let tx = load_tx("2pteD1PHCozmyijbVCucCRX8QzqcHG9kpAm2ZfCCgxmLWwQFiRCA7h19tHofV9giCxJwEhM9NUycFs5muWbRLwX");
    let l = bundled_core::parse_create(&tx).expect("DBC create");
    assert_eq!(l.venue, "Meteora DBC");
    assert_eq!(l.platform.as_deref(), Some("Bags"));
    assert_eq!(l.mint, "H7Ge27Y2VxMJzwZ4o4qqsQuJnw2sMHticiTiMQoFBAGS");
}

/// A DBC launch made with `initialize_virtual_pool_with_token2022_transfer_hook`, through a launcher's program:
/// $HAMMER (FLXu…1ydk, 2026-09-28) was sent to the Telegram bot and read "isn't a token launch". Captured 2026-10-24.
#[test]
fn parses_a_dbc_transfer_hook_launch() {
    let tx = load_tx("4ShjYMibM9ufSUQrPRaXWJdDUL2wNDmXDVroe8fpPE7JPSJvs7Qv2ya9DCA9DKi92DKs6DoBkYwQWRyQeUaKo98Y");
    let l = bundled_core::parse_create(&tx).expect("DBC create");
    assert_eq!(l.venue, "Meteora DBC");
    assert_eq!(l.mint, "FLXuCG8J27ZW3nFe9nSEvApNqjZHFi4yEoDojcud1ydk");
    assert_eq!((l.name.as_str(), l.symbol.as_str()), ("HAMMER", "HAMMER"));
    assert_eq!(l.creator, "AnA4CnD29tArRRbzdpgzEDepjZJEHPigHqcu4Co8THUE");
    assert_eq!(l.bonding_curve, "6j6GdJdVohWMbUjumnN1w3KeBwQvaDzbob1b6hP3nwsu");
    assert_eq!(bundled_core::dbc::config_of(&tx, &l.mint).as_deref(), Some("2iqaXwGJV7UYoq87BRW7ZZ6fCvfwsWqo9EeyyJgnYmSa"));
}

/// A buy on that transfer-hook curve (`swap2_with_transfer_hook`), whose only swap event is `EvtSwap2WithTransferHook`:
/// 0.0198 SOL in (the vault's 19,798,020 lamports and a 1,980 referral fee), 3,141,466,158,586 raw $HAMMER out.
#[test]
fn dbc_transfer_hook_swaps_are_priced_by_the_curve() {
    use bundled_core::trades::trades;
    let buy = trades(&load_tx("MpGtQyEwdSgaZD4MEEmgmZQTaFth1R3S2erSHU6fb1DCZFEbEGZ2VqJNZpo6E2qec3ap3wqYBDptovc9q8Jd2pq"), "FLXuCG8J27ZW3nFe9nSEvApNqjZHFi4yEoDojcud1ydk");
    assert_eq!(buy.len(), 1);
    assert!(buy[0].is_buy && !buy[0].generic && buy[0].quote.is_none());
    assert_eq!(buy[0].wallet, "237UwM2EwyivqzSQeVSZiSjNVwMLJkr6rgkW21S5ziUh");
    assert_eq!((buy[0].lamports, buy[0].tokens), (19_800_000, 3_141_466_158_586));
}

/// $HAMMER's mint, read 2026-10-24: its creator kept the mint authority (a DBC config can leave it to the creator) and
/// had minted 52.6M since launch, on 500M; nobody can freeze.
#[test]
fn reads_a_mint_with_its_authority_kept() {
    use base64::Engine;
    let raw = std::fs::read_to_string(fixtures().join("accounts/FLXuCG8J27ZW3nFe9nSEvApNqjZHFi4yEoDojcud1ydk.b64")).unwrap();
    let data = base64::engine::general_purpose::STANDARD.decode(raw.trim()).unwrap();
    let m = bundled_core::mint::parse(&data).unwrap();
    assert_eq!(m.mint_authority.as_deref(), Some("AnA4CnD29tArRRbzdpgzEDepjZJEHPigHqcu4Co8THUE"));
    assert_eq!(m.freeze_authority, None);
    assert_eq!(m.supply, 552_629_913_521_191);
}

/// SOL a wallet sent: CRYq…LV5 paid 3.115 SOL to the wallet that later signed $HAMMER's create (82 minutes before it),
/// and 0.5 SOL to a new wallet that later bought 9.6% (35 minutes before).
#[test]
fn reads_sol_a_wallet_sent() {
    use bundled_core::funding::outgoing;
    let from = "CRYqtDi9Ki5eKMYqQNUXBgonwVD7cf9XTUXT6FiajLV5";
    let paid = |sig: &str| outgoing(from, &[load_tx(sig)]).into_iter().map(|o| (o.from, o.lamports)).collect::<Vec<_>>();
    assert_eq!(
        paid("4cJ7k6CGHzb7ZH924WTQ1aK5o6pw2iFZ31Jm7CA4YSeejsFwqnh6f5KAtq4DTx9uP2wNcbcCyyd4KG83W1svG8ap"),
        vec![("85UjJAUfFaAFt5YPaG5hGHn4VVdDoC4oEetZoMCyqNyT".to_string(), 3_115_000_000)]
    );
    assert_eq!(
        paid("bUazxHQ4Wrt9Wh6dnYQkDyFDKFzAi7TeewodGXob4c5wQriX4pRxG9v8Xr9KHoXeA5Q4hv8tGFzb47QwCEPTVDF"),
        vec![("8tJHbsNmFyEJsposfYNJAmsYugVjcbQvBrezseicrcG9".to_string(), 500_000_000)]
    );
    // The receiver's side: no SOL sent.
    assert!(outgoing("8tJHbsNmFyEJsposfYNJAmsYugVjcbQvBrezseicrcG9", &[load_tx("bUazxHQ4Wrt9Wh6dnYQkDyFDKFzAi7TeewodGXob4c5wQriX4pRxG9v8Xr9KHoXeA5Q4hv8tGFzb47QwCEPTVDF")]).is_empty());
}

/// A Privacy Cash withdrawal (2026-10-24): relayer W97m…FaqH signs and pays the fee, the pool 4AV2…vfGh pays out 2.04
/// SOL to 4SeN…x4VdL. The funder is the pool, which links nobody; a plain transfer is still its signer's.
#[test]
fn a_privacy_pool_payout_is_the_pools() {
    use bundled_core::{funding::incoming, labels};
    let pool = "4AV2Qzp3N4c9RfzyEbNZs2wqWfW4EwKnnxFAZCndvfGh";
    let tx = load_tx("37x2QZ66csK8TKQm4jzCE9kRTVw4HzvnTx9btS9SHgEEAk3EBfr4UBuF5ry5ovdoAMGF39ct7TsQ1DYKMnxmfo29");
    let got = incoming("4SeNq5AdLas8JQSYKAoJrC9JiNWpgHGAo7XHnYYx4VdL", &[tx]);
    assert_eq!(got.iter().map(|i| (i.from.as_str(), i.lamports)).collect::<Vec<_>>(), vec![(pool, 2_036_824_999)]);
    assert!(labels::funds_everyone(pool));
    assert!(!labels::funds_everyone("W97m2N12NdSgWu3bppYCPHdXNwz15SDUZNoD5jNFaqH"));
    let plain = load_tx("bUazxHQ4Wrt9Wh6dnYQkDyFDKFzAi7TeewodGXob4c5wQriX4pRxG9v8Xr9KHoXeA5Q4hv8tGFzb47QwCEPTVDF");
    let got = incoming("8tJHbsNmFyEJsposfYNJAmsYugVjcbQvBrezseicrcG9", &[plain]);
    assert_eq!(got[0].from, "CRYqtDi9Ki5eKMYqQNUXBgonwVD7cf9XTUXT6FiajLV5");
}

#[test]
fn parses_a_meteora_dbc_launch() {
    let tx = load_tx("uXVrQqzt5VXeuimz1oKjAwyYqVAgcPtwLG5kFWPQzikrFohrns4THXqk6WhAi8z3ceMqL99NpfBtZRD82cT82oW");
    assert!(parse_launch(&tx).is_none(), "not a pump.fun create");
    let l = bundled_core::parse_create(&tx).expect("DBC create");
    assert_eq!(l.venue, "Meteora DBC");
    assert_eq!(l.platform, None);
    assert_eq!(l.mint, "D4Qim7uWzFtVLXBnhLM2MKYs7jYD4rqP8kNXFeazT1rG");
    assert_eq!(l.creator, "D5aNnoXryBwuzLvPe5vtMRUodhwLe1oQqtkuL1KnbLAd");
    assert_eq!(l.bonding_curve, "EtqUX1j29mcR7JbTq3cfFhyBJorAoYqTxFtWaqW2Eo8h");
    assert_eq!((l.name.as_str(), l.symbol.as_str()), ("SydneyCat", "SYDNEYCAT"));
    assert_eq!(l.supply, 1_000_000_000_000_000);
}

#[test]
fn recognises_uxento_by_its_fee_wallet() {
    use bundled_core::addr::UXENTO_FEE_WALLET;
    let uxento = load_tx("3xxQwRrszDgkRXBaPdRyPnffjgfEWkf6EQz13GsfevJWfGyJWagvVpPTFJt4QbJuhxLB6biHzLwwG3msqo2zL5kQ");
    assert!(parse_launch(&uxento).is_some(), "a pump.fun create");
    assert!(uxento.mentions(UXENTO_FEE_WALLET));
    let sumo = load_tx("2o9CB3BmTtFLrHEU81K8zrESTLiqZh1W8Vo3auFgumXQ2RTi39R6YgGrcczNhD8EbQda6vbhutpr3vKtVzkUYq3p");
    assert!(!sumo.mentions(UXENTO_FEE_WALLET));
}

#[test]
fn bonk_creates_parse() {
    use bundled_core::parse_create;
    let tx = load_tx("k7KnY5NK9bkDugGvXjSQQRD8Mn5ZEtiXohi5DnJe6uQoUyo2Mt2b61hucRpTgxa7ThHFv3aZbA28CGMdqUByKho");
    let l = parse_create(&tx).expect("a Bonk create");
    assert_eq!(l.venue, "Bonk");
    assert_eq!(l.mint, "Hd9Jzr7Lyz3KvmE5qfFwfe3GdD2zKEiBjyrFw4qoSFgu");
    assert_eq!(l.name, "Love Your Meme");
    assert_eq!(l.symbol, "LYM");
    assert_eq!(l.supply, 1_000_000_000_000_000);
    // The creator bought in its own create: about 9.6% of the supply.
    let buys = signer_buys(&tx, &l.mint);
    let a = analyze_bundle(&l.creator, l.supply, &buys);
    assert!((a.total_pct - 9.62).abs() < 0.01, "{}", a.total_pct);

    let tx = load_tx("4qunqtJivsJf3yCaM9Fe2rGmbnG4ayYq6ihYCeHUT3sbB9zeXctLUDL9RTwvh6Jefrr5F3C9m25GyVapH7emdGJM");
    let l = parse_create(&tx).expect("a second Bonk create");
    assert_eq!(l.mint, "DU2wuQ4d9v12RaYpTuNL8CzpSqudQhLgNXnja6wJ7wxT");
    assert_eq!(l.symbol, "E");
}


/// A Sumo launch (ADIX, on the original's Sumo list): its create loads accounts
/// from Sumo's shared lookup table, the fingerprint the server checks.
#[test]
fn sumo_create_names_its_lookup_tables() {
    let tx = load_tx("2o9CB3BmTtFLrHEU81K8zrESTLiqZh1W8Vo3auFgumXQ2RTi39R6YgGrcczNhD8EbQda6vbhutpr3vKtVzkUYq3p");
    let tables: Vec<&str> = tx.lookup_tables().collect();
    assert!(tables.contains(&"BgRe3UrGYLVbtnug2BGgKVEF8MQwXQ6cfzVLGyDBzRxU"), "{tables:?}");
    assert_eq!(bundled_core::parse_create(&tx).map(|l| l.mint).as_deref(), Some("9QENqoT6kX45g9oXnDXtog1e7xrjqAPTjiWLopJJpump"));
}

/// Super Inu (SI): a Raydium LaunchLab curve quoted in NVDAx (8 decimals), captured 2026-10-16. Its first trades were
/// priced by what wallets paid, and read as dust: the creator's 3.64 NVDAx as 0.08 SOL of rent.
const SI: &str = "DEW9dSN6QpWyNthphCpMmAbZP1Q4cEKR9xQXAri98WDP";
const NVDAX: &str = "Xsc9qvGR1efVDFGLrVsmkzv3qi45LTBjeUKSPmx9qEh";
const SI_SUPPLY: u64 = 997_997_180_097_072;

#[test]
fn launchlab_trades_are_priced_by_the_curve_in_its_quote_token() {
    use bundled_core::trades::{price_quotes, trades};
    // The create, with the creator's buy of 20% of the supply.
    let mut t = trades(&load_tx("5X4SGFFUFZCrxTeabiQFJGKGp1QjkEhH25w1XrXJryJ2GASWctDipDnFX7eHidMhHyjxWJc3x6TpvPJLrATdDSe6"), SI);
    assert_eq!(t.len(), 1);
    assert!(t[0].is_buy && !t[0].generic);
    assert_eq!(t[0].wallet, "9pkJqJLtefuna3GLtKZyF5DBWwEGUdxZUSGCmf5jZhA9");
    assert_eq!(t[0].tokens, 198_279_170_389_796);
    let q = t[0].quote.as_ref().unwrap();
    assert_eq!((q.mint.as_str(), q.amount, q.reserves), (NVDAX, 364_218_805, 1_567_271_550 + 359_666_069));
    assert_eq!(t[0].virtual_tokens, 1_073_025_605_602_079 - 200_281_990_292_724);
    // At 1 SOL per NVDAx: 19.27 NVDAx of reserves against 872.7M tokens, 998M supply.
    price_quotes(t.iter_mut(), |m| (m == NVDAX).then_some((8, 1.0)));
    let cap = t[0].market_cap(SI_SUPPLY);
    assert!((cap - 22.04).abs() < 0.05, "{cap}");
    assert!((t[0].sol() - 3.642).abs() < 0.001);

    // Three wallets bought in one transaction: each trade goes to its own wallet, priced up the curve in order.
    let t = trades(&load_tx("5jUWKqzrzE42fEKeRuqv9vdygSVsxEKufAcmcGLvFVKDqNYhYS67RGjFfHhn9KGB4voaiKbkvczd2NAzcthVMeWY"), SI);
    let got: Vec<(&str, u64)> = t.iter().map(|t| (t.wallet.as_str(), t.tokens)).collect();
    assert_eq!(
        got,
        [
            ("tQi75x9GeqsDeFdPVdwn6fwfiNLog53STe56Wjt4MVj", 46_816_113_466_500),
            ("8pNSgAe9JPi7fSodDSAXmjY9tbkPZQ5TpdR6LddhJtse", 35_662_046_687_273),
            ("7K1ygpyD3EXtyfnQ1X8Vsk5tn2PR7Whah96dCZoR7uZ", 34_904_902_564_177),
        ]
    );
    let reserves: Vec<u64> = t.iter().map(|t| t.quote.as_ref().unwrap().reserves).collect();
    assert!(reserves.windows(2).all(|w| w[0] < w[1]));

    // A sell: tokens in, NVDAx out, priced lower than before it.
    let t = trades(&load_tx("3kMzVafe45ZcyQyAnNVvBGiH8gcNdfBzHFnKcHyAkVubGx3bfxCLn7rUxnMSRuBs1M42Hti6ZJCwvgB9azXG69Mc"), SI);
    assert_eq!(t.len(), 1);
    assert!(!t[0].is_buy);
    assert_eq!(t[0].wallet, "EQUiJsxnuQVj11s6JFcXowgJLMW32UZimpNgkrgSy25N");
    assert_eq!(t[0].quote.as_ref().unwrap().amount, 81_263_113);
    assert_eq!(t[0].virtual_tokens, 1_073_025_605_602_079 - 658_107_452_359_043);
}

#[test]
fn a_launchlab_buy_bought_with_sol_swapped_in_is_priced_by_the_curve() {
    use bundled_core::trades::trades;
    // The wallet swapped 0.53 SOL into NVDAx and bought with 0.275 NVDAx in one transaction.
    let t = trades(&load_tx("4gx3gQRtHB4mWgV9EMp5PzUsbqPP5iaed3kc85pzFBb19Cs2Lvpp9v3fDuaVZpKnydaYM5aiZ1ox7sbQGF3ucawq"), SI);
    assert_eq!(t.len(), 1);
    assert!(t[0].is_buy && !t[0].generic);
    assert_eq!(t[0].quote.as_ref().unwrap().amount, 27_536_904);
    assert_eq!(t[0].quote.as_ref().unwrap().reserves, 1_567_271_550 + 689_805_194);
}

#[test]
fn a_launchlab_curve_quoted_in_sol_is_priced_by_its_reserves() {
    use bundled_core::trades::trades;
    // GRAIL: three wallets in one transaction, on a curve quoted in SOL. Captured 2026-10-16.
    let t = trades(
        &load_tx("2f1kck5QyKhbKzzVSFRpA77qvZ49CqL8QhQsekoDF9td8n4aZNBXnEJNGhzM6A47HFEuH5csh1TAM8reZ7tc3wei"),
        "FYFDLqtBd4ZAiDfbx7XxXndcjJuBEZCz5XKAqQwaHC8W",
    );
    assert_eq!(t.len(), 3);
    assert!(t.iter().all(|t| !t.generic && t.quote.is_none()));
    assert_eq!(t[0].lamports, 2_361_186_915);
    assert_eq!(t[0].virtual_sol, 30_000_852_951 + 2_622_706_606);
    assert_eq!(t[0].virtual_tokens, 1_073_025_605_596_382 - 86_263_773_249_137);
    // A token with a 3% transfer fee: each wallet got a little less than the curve sent, and keeps its own amount.
    assert_eq!(t[0].wallet, "3uS2ZDLL952nzkifsmzFK4Jgc8EH4HVmxr5T97orF3UG");
    assert_eq!(t[0].tokens, 73_675_860_023_703);
}

/// RMT: a Meteora DBC curve quoted in SOL, captured 2026-10-16.
const RMT: &str = "2C6wg2BfrVEYLvBoX8caShdkp125bYe3ksLPwfQnKS2y";

#[test]
fn dbc_trades_are_priced_by_the_curve() {
    use bundled_core::trades::trades;
    let buy = trades(&load_tx("5KHkH9YdwF3yTEpvZRqRwUdcma9omBdSBzHYZGCBJAkpb3HGgWBeveRbUAj9ctpFncYEEXoBCTH56JHp7YK14LSt"), RMT);
    assert_eq!(buy.len(), 1);
    assert!(buy[0].is_buy && !buy[0].generic && buy[0].quote.is_none());
    assert_eq!(buy[0].wallet, "G3wm1Vy7y8V9Bdb542fev9LBFDKkigwJdKww9gmie8J2");
    assert_eq!((buy[0].lamports, buy[0].tokens), (7_174_273, 417_770_718));
    let sell = trades(&load_tx("4zFao5kW26muMu7mBrXj6v5UHpnZMnZ2cBSftY22DUdjprbqk3wVhy7Zj4Yeg6MNbDjHwiAwYS64bAYQ8RTK3YLf"), RMT);
    assert_eq!(sell.len(), 1);
    assert!(!sell[0].is_buy && !sell[0].generic);
    assert_eq!((sell[0].lamports, sell[0].tokens), (7_032_353, 417_770_718));
    // The curve's price after each, about 0.0170 lamports a raw unit, as the trades' own execution prices say.
    for t in [&buy[0], &sell[0]] {
        let price = t.virtual_sol as f64 / t.virtual_tokens as f64;
        assert!((price - 0.0170).abs() < 0.0001, "{price}");
    }
}

/// $ANSЕMINU (EGmv…2Q7U, 2026-10-16): a Meteora DBC config whose curve sold 0.05% of a fixed 1B supply and moved 0.05%
/// to its DAMM v2 pool, and kept the rest for the wallet that made it, 8TPAC…zxLc. It took 999M tokens out of the
/// curve's vault 13m46s after launch and sold them into the pool within 30 seconds.
const ANSEM_MINT: &str = "EGmvDmLqXsMCwcFy2PzP7LhrEVaFpnaByKmRuPec2Q7U";
const ANSEM_OWNER: &str = "8TPACXaKotSZ7WXktfmKDRhgoypyGXNzo1ctr2YBzxLc";

#[test]
fn dbc_config_keeps_the_leftover_for_its_owner() {
    use base64::Engine;
    let tx = load_tx("3YkKrGpLHSTbqKWBCPabUHhJFrANfVu2ymqBEwCUumeC4Pegi8rkbZJtMBZqVRq2ceocsdCRGRbijk5F44z6z6n4");
    let config = bundled_core::dbc::config_of(&tx, ANSEM_MINT).expect("the create names its config");
    assert_eq!(config, "TRK1nRhn4kjuUp3odvcLttV92nT1sWFBvWXvVGHfDSZ");
    let raw = std::fs::read_to_string(fixtures().join(format!("accounts/{config}.b64"))).unwrap();
    let data = base64::engine::general_purpose::STANDARD.decode(raw.trim()).unwrap();
    let c = bundled_core::dbc::Config::parse(&data).expect("a DBC config");
    assert_eq!((c.fee_claimer.as_str(), c.leftover_receiver.as_str()), (ANSEM_OWNER, ANSEM_OWNER));
    assert!(c.fixed_supply);
    assert_eq!((c.curve_sells, c.to_pool, c.vesting), (542_745_539_314, 500_773_625_664, 0));
    assert_eq!((c.supply_before, c.supply_after), (1_000_000_000_000_000, 1_000_000_000_000_000));
    assert_eq!(c.set_aside(), 1_000_000_000_000_000 - 542_745_539_314 - 500_773_625_664);
    let (kept, sells) = c.shares().unwrap();
    assert!((kept - 99.896).abs() < 0.001 && (sells - 0.0543).abs() < 0.0001, "{kept} {sells}");
    // Without a fixed supply, nothing is left over: what the curve doesn't sell is burned.
    assert_eq!(bundled_core::dbc::Config { fixed_supply: false, ..c }.set_aside(), 0);
}

#[test]
fn a_dbc_leftover_withdrawal_is_supply_taken_not_bought() {
    use bundled_core::{addr::DBC_POOL_AUTHORITY, trades};
    let tx = load_tx("4pHjDAXJAUFKo3pLZfroCLoA4SCiG53jtN3KWMhZi74VdY6FEf2RUSxuXiM1V64AfrL7PSJbVffzpnJ3B5qvJcKK");
    assert!(trades::trades(&tx, ANSEM_MINT).is_empty(), "not a trade");
    let t = trades::transfers_without_trades(&tx, ANSEM_MINT);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].from.as_str(), t[0].to.as_str()), (DBC_POOL_AUTHORITY, ANSEM_OWNER));
    assert_eq!(t[0].raw, 998_958_802_924_436 - 2_322_089_344);
    // Any other coin's history skips it.
    assert!(trades::transfers_without_trades(&tx, "So11111111111111111111111111111111111111112").iter().all(|t| t.from != DBC_POOL_AUTHORITY));
    // The migration and the fee claim pay no leftover.
    for sig in [
        "4UYfRLFZJ1ZdT7Kr5AqpDxDtcEUgGPkTZKRrEz4u32Lrq73QoDpiPWzgvFjHV5J2V3wrL2t3FpqZ3JEPWASEzW9A",
        "2Uu93SFh8Rp1JnH7snBCfVuBnDRF6bedYsZBBUnNVBMjJHHL61LbobMn3o2Kf2m2FqgwDDjwjMEuZu3kocMCuujc",
    ] {
        let tx = load_tx(sig);
        assert!(bundled_core::dbc::leftover_withdrawals(&tx, ANSEM_MINT).is_empty(), "{sig}");
    }
}

#[test]
fn a_migrated_dbc_coin_is_priced_by_its_damm_v2_pool() {
    use base64::Engine;
    let account = |a: &str| {
        let raw = std::fs::read_to_string(fixtures().join(format!("accounts/{a}.b64"))).unwrap();
        base64::engine::general_purpose::STANDARD.decode(raw.trim()).unwrap()
    };
    // Its curve: moved to the pool 5m22s after launch; DexScreener still prices the coin at the curve's last price.
    assert_eq!(bundled_core::dbc::curve_migrated(&account("4h36KSNw2QbokJ8vELdq4NZ3QHVWhj2aCgNiuuLVPTm6")), Some(true));
    // Its pool, emptied by the dump: 0.0014 SOL for the whole supply (GeckoTerminal: $0.14).
    let (a, b, price) = bundled_core::dbc::damm_v2_price(&account("J599ddtph9AfyYJjdTMPKzgREPesFj94LcqVKAhCJPkB")).unwrap();
    assert_eq!((a.as_str(), b.as_str()), (ANSEM_MINT, "So11111111111111111111111111111111111111112"));
    let cap_sol = price * 1e15 / 1e9;
    assert!((cap_sol - 0.00143).abs() < 0.0001, "{cap_sol}");
    // A curve account isn't a pool.
    assert!(bundled_core::dbc::damm_v2_price(&account("4h36KSNw2QbokJ8vELdq4NZ3QHVWhj2aCgNiuuLVPTm6")).is_none());
}

/// $BUNDLED's creator AmC98… locked its 6.6% launch buy in a Streamflow contract on 2026-10-15 20:01 UTC
/// (Q5CwK…): contract FF7ky…, escrow 9QTt4…. It unlocks all at once on 2026-11-14 23:00 UTC and nobody can cancel it.
/// Read 2026-10-21.
const BUNDLED_MINT: &str = "Fm46kSM8KW18abKcuTR8RB16jrhQFXKpUMME5azKpump";

#[test]
fn a_streamflow_lock_names_its_contract_and_the_contract_its_terms() {
    use base64::Engine;
    use bundled_core::streamflow;
    let account = |a: &str| {
        let raw = std::fs::read_to_string(fixtures().join(format!("accounts/{a}.b64"))).unwrap();
        base64::engine::general_purpose::STANDARD.decode(raw.trim()).unwrap()
    };
    let escrow = "9QTt4kcwEUm87yCQH6aS8wCqpfYahhnYtZVrL59yEhrx";
    let creator = "AmC98qT8x4tc4YKLX9dbvbAu2wMM9NH3bgodVLfBwP8k";
    let tx = load_tx("Q5CwKcRRbTodLtXDSjnWCorG1mbXWcMwxjLTRNMg2YJV8GDJmJknTBU9nS2vP8GwWSDNAbkqzp8vZZyKYoY6Zdo");
    let contract = streamflow::contract_of(&tx, BUNDLED_MINT, escrow).expect("the create names its contract");
    assert_eq!(contract, "FF7kys1q1VPeLKSqy13tsyQfS5Hn1RbzLKgQqzsUVhuJ");
    // Not a holder of the coin in that transaction: no contract.
    assert_eq!(streamflow::contract_of(&tx, BUNDLED_MINT, "5SEpbdjFK5FxwTvfsGMXVQTD2v4M2c5tyRTxhdsPkgDw"), None);
    let t = streamflow::decode(&contract, &account(&contract), BUNDLED_MINT, escrow).expect("a Streamflow contract");
    assert_eq!((t.sender.as_str(), t.recipient.as_str()), (creator, creator));
    // What the escrow holds: the deposit less Streamflow's and the partner's fees (296,801,705,478 + 32,977,967,275).
    assert_eq!(t.deposited, 65_955_934_550_769);
    assert_eq!(t.deposited + 296_801_705_478 + 32_977_967_275, 66_285_714_223_523 - 1, "the creator kept 1 raw unit");
    assert_eq!((t.withdrawn, t.cliff_amount, t.amount_per_period, t.period), (0, t.deposited, t.deposited, 1));
    assert_eq!((t.start, t.cliff, t.end), (1_794_697_200, 1_794_697_200, Some(1_794_697_200)));
    assert!(!t.cancelable_by_sender && !t.cancelable_by_recipient && !t.transferable_by_sender);
    assert!(!t.pausable && !t.can_update_rate && !t.closed && t.canceled_at.is_none());
    assert!(!t.can_take_back(creator));
    // Another coin's or another escrow's contract is not this one.
    assert_eq!(streamflow::decode(&contract, &account(&contract), ANSEM_MINT, escrow), None);
    assert_eq!(streamflow::decode(&contract, &account(&contract), BUNDLED_MINT, creator), None);

    // A team's vesting of 2wqw8… (read 2026-10-21): 20% at the start on 2026-08-18 12:37 UTC, then 114,285,714,286 a week
    // for 21 weeks, to 2027-01-12. Its sender can cancel it, and a crank's withdrawal (3hBY5…) names it too.
    let (mint, escrow) = ("2wqw81F24mxBsQTAvKFAufzmzPZVnq29CJqftekforgE", "8QXCYZR6Ez8Rxq9vbz4EBxmgmxySev3kX37rMAaU9gRG");
    let tx = load_tx("3hBY5Cm8q7qA2ngMVYx4hGNPgL9N7vKUH7qvyx8hD7RBiEAhmACgKUX2CxpbM8fsPLEzGrabFJSpf6xqaJYHRjn8");
    let contract = streamflow::contract_of(&tx, mint, escrow).expect("the withdrawal names its contract");
    assert_eq!(contract, "BiAp5mJvYpFLs94R9JxCzX81YQGeXcMvdXiAeCf2hknG");
    let t = streamflow::decode(&contract, &account(&contract), mint, escrow).expect("a Streamflow contract");
    assert_eq!(t.sender, "7MveZGqMymryCKwFazbf5Ewx72ZxLFP9k8KjhKZt8FbM");
    assert_eq!(t.recipient, "CF8L4x6nfgq61TP1A9kGdpHkGmzDBRBfqxccD5fL7t3q");
    assert_eq!((t.deposited, t.withdrawn, t.cliff_amount), (3_000_000_000_000, 1_628_571_428_574, 600_000_000_000));
    assert_eq!((t.period, t.amount_per_period), (604_800, 114_285_714_286));
    assert_eq!((t.start, t.cliff), (1_787_056_661, 1_787_056_661));
    // The end it keeps is the one its schedule gives: 21 weeks from the cliff pay out the rest.
    let periods = (t.deposited - t.cliff_amount).div_ceil(t.amount_per_period);
    assert_eq!(periods, 21);
    assert_eq!(t.end, Some(t.cliff + (periods * t.period) as i64));
    assert!(t.cancelable_by_sender && !t.cancelable_by_recipient && t.transferable_by_sender);
    assert!(t.can_take_back(&t.sender));
}

/// A Bonk launch whose creator bought through a route that swapped SOL into BONK, the curve's quote, first (2026-10-16):
/// the new coin is the create's base mint (F4hf…bonk), not BONK, which outweighed it in the transaction's balances and
/// had its own address stored as this launch.
#[test]
fn a_launchlab_create_bought_through_its_quote_is_the_base_mint() {
    let l = bundled_core::parse_create(&load_tx("34syWGQhxSde1BZPpiSWzAmdXnvj1ENVw9Hn6Mn5s4PQQqFaMkHcZ5R96xZTmkSiX8AjM5SSMEbLq8KmNxMvAdCA"))
        .expect("a LaunchLab create");
    assert_eq!(l.mint, "F4hfCEHJfq9HkrriUaGrgQ3Dg1xBSQWADht2ftRxbonk");
    assert_eq!((l.venue.as_str(), l.symbol.as_str()), ("Bonk", "LOULOU"));
    assert!(l.supply > 0);
}

/// Gw9f…moon (Meteora DBC, 2026-10-21): its creator bought 61.6% in the create, then sent all of it to a second wallet in
/// a transaction both signed, in the first second. That was read as a second 61.6% buy: "123% bought in the first
/// second".
#[test]
fn a_transfer_between_signers_is_not_a_buy() {
    const MINT: &str = "Gw9fgZZn5gyaWD4CvvPmznU5mKfZJGqAJSdu1Wr6moon";
    let transfer = load_tx("27MhCybGKk3dAi3JS6cyV5zhN8WG9gg2Cci6HeDsQ88gMmzTPxcLdVhM7juiq1PQL8CVpGyeFqB88phEbCtjHWAf");
    assert_eq!(transfer.token_deltas(MINT)["A7LYDCwfs2RiafmmtGjuW5vhHaotSc4HF6XZUHEvuWPQ"], 616_089_527_716_622);
    assert!(signer_buys(&transfer, MINT).is_empty());
    // The create's own buy still counts in full.
    let create = load_tx("3VPMMzY631TcooaBVzM49u31Xxa2dgCpPKoYuYcPKoPZDjZesDT6QDnpUeufUN5K4LbUVdNXF3rLdLZas6CXWWsV");
    let buys = signer_buys(&create, MINT);
    assert_eq!(buys.len(), 1);
    assert_eq!((buys[0].wallet.as_str(), buys[0].raw), ("DYtCZ1LhvhyWmQjpqmiFuX14QBXZ2At85cub3HS26V3J", 616_089_527_716_622));
}

/// A first second's moves as `scanner::first_second_from` makes them, each transaction's buys counted for `group`, and
/// what each group bought before any was counted once.
fn first_second_flows(file: &str, mint: &str, group: impl Fn(&str) -> usize) -> (Vec<Move>, [u128; 3]) {
    let raw = std::fs::read_to_string(fixtures().join(file)).unwrap();
    let txs: Vec<Tx> = serde_json::from_str(&raw).unwrap();
    let mut held = std::collections::HashMap::new();
    let mut flows = vec![];
    let mut gross = [0u128; 3];
    for t in &txs {
        for s in signer_sells(t, mint) {
            let mut left = s.raw as u128;
            for g in 0..3 {
                let h: &mut u128 = held.entry((s.wallet.clone(), g)).or_default();
                let back = (*h).min(left);
                *h -= back;
                left -= back;
                if back > 0 {
                    flows.push(Move::Sell(g, back));
                }
            }
        }
        for b in signer_buys(t, mint) {
            let g = group(t.signature());
            *held.entry((b.wallet, g)).or_default() += b.raw as u128;
            gross[g] += b.raw as u128;
            flows.push(Move::Buy(g, b.raw as u128));
        }
    }
    (flows, gross)
}

/// $LOTTO (FsHA…pump, 2026-10-19): its first second as the history call returns it. Its creator's Jito bundle (6ZU77…,
/// 4LKQ…) bought 60% and sold all of it back two blocks later (3iP4…); then three other bundles bought 54.6% of the same
/// tokens. Every buy added up read "115% bundled".
#[test]
fn a_first_second_counts_each_token_once() {
    let (flows, gross) = first_second_flows("tx/lotto-first-second.json", "FsHAhmhwNqcSPzpNh9HUhYSmpkG9SBK9jH9aBymMpump", |sig| match &sig[..6] {
        "4bGUXR" | "4LKQEu" => 0,
        "5inMZW" | "gbyiqU" | "5aEB7u" => 1,
        _ => 2,
    });
    let share = |r: u128| r as f64 / 1e13;
    assert!((share(gross[0]) - 60.0).abs() < 0.01 && (share(gross[1]) - 54.58).abs() < 0.01, "{gross:?}");
    let counted = count_once(&flows, 3, Some(2));
    // The creator's bundle keeps its 60%: a bundle that dumps is still told. The other bundles bought back what it sold.
    assert!((share(counted[0]) - 60.0).abs() < 0.01, "{counted:?}");
    assert_eq!(counted[1], 0);
    assert!(share(counted.iter().sum()) <= 100.0);
}

/// $OpenBot (MzUx…pump, 2026-10-28): its first second as the history call returns it. No creator's bundle (Jito has
/// none; its create bought nothing). A sniper (7Jhd…) bought 25%, then sold it back in a bundle of its own (4srC…) right
/// before another bundle's two wallets (44B9…, 4shuC…) bought 24.3%. Counted for the sniper, the bundle read 0% and the
/// coin "not bundled".
#[test]
fn a_bundle_buying_what_plain_buys_sold_back_keeps_its_share() {
    let (flows, gross) = first_second_flows("tx/openbot-first-second.json", "MzUxrgP4k6huTD415pA9bKzBA8BvCgyu15TfbEnpump", |sig| match &sig[..6] {
        "5Ae3Qy" => 0,
        "44B9un" | "4shuC8" => 1,
        _ => 2,
    });
    let share = |r: u128| r as f64 / 1e13;
    assert_eq!(gross[0], 0);
    assert!((share(gross[1]) - 24.26).abs() < 0.01, "{gross:?}");
    let before = count_once(&flows, 3, None);
    assert_eq!(before[1], 0, "{before:?}");
    let counted = count_once(&flows, 3, Some(2));
    // The bundle's whole buy is its own, taken off the plain buys: the first second's total doesn't change.
    assert_eq!(counted[1], gross[1], "{counted:?}");
    assert_eq!(counted.iter().sum::<u128>(), before.iter().sum::<u128>());
    assert!(share(counted[2]) < share(before[2]));
}
