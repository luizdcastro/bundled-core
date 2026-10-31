//! The full report: what every flagged wallet did with the token, who holds
//! it now, the red flags, and a risk level. Pure: callers fetch the trades,
//! holders and wallet history, and this turns them into findings.
//!
//! Thresholds here are first guesses, tuned against the original's samples;
//! each one is a named constant so it can be adjusted in one place.

use crate::crash::{self, Crash};
use crate::funding::{self, Funding};
use crate::labels;
use crate::bundle::{count_once, Move};
use crate::pct;
use crate::streamflow::LockTerms;
use crate::trades::{Trade, Transfer};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// A first buy within this many seconds of launch, outside the bundle, is a sniper.
pub const SNIPE_SECS: i64 = 10;
/// A fresh wallet buying back an insider's sell within this window is a hand-off.
pub const RELAY_SECS: i64 = 15;
/// Hand-off amount tolerance: strong match, and weaker "possible" match.
pub const RELAY_TOLERANCE: f64 = 0.02;
pub const POSSIBLE_RELAY_TOLERANCE: f64 = 0.05;
/// Weaker matches only count this close to the sell: busy tokens get look-alike
/// buys by chance.
pub const POSSIBLE_RELAY_SECS: i64 = 5;
/// Holders below this share of supply only received dust.
pub const DUST_PCT: f64 = 0.0001;
/// Consecutive early buys this regular (coefficient of variation) look scripted.
pub const REGULAR_CV: f64 = 0.15;
/// This many holders on one identical balance (dust aside) is a supply split up across wallets, not a crowd of buyers.
pub const SPLIT_MIN_WALLETS: usize = 20;
/// Insiders sending at least this share of supply out of their wallets without selling it is a red flag.
pub const MOVED_MIN_PCT: f64 = 5.0;
/// A DBC curve's leftover (supply its config keeps from buyers for one wallet) from this share of the supply up is told.
pub const LEFTOVER_MIN_PCT: f64 = 1.0;
/// The creator's record says it sells out fast when its launch buy was gone by the 2-minute check on at least this many
/// of its earlier launches, and on at least half of them.
pub const CREATOR_FAST_MIN: i64 = 2;
/// A launch buy above this share of the supply moves the price when it's sold (about 8% or more), as Filtered's "Dumped"
/// counts it: a creator that sells out fast, buying that much again, weighs on the risk.
pub const CREATOR_BIG_BUY_PCT: f64 = 4.0;
/// Snipers linked by how they were funded or paid are coordinated from this many wallets and this share of the supply
/// bought. What they bought weighs on the risk as a bundle's does, even after they've sold: a bundle by other means.
/// Snipers on their own, the bots every launch draws, don't count.
pub const COORDINATED_MIN_WALLETS: usize = 3;
pub const COORDINATED_MIN_PCT: f64 = 5.0;
/// The wallet reasons that link a sniper to others or to the launch: a shared funder, a flagged funder, a relay, an
/// exchange burst, or funding through hops in the hour before launch.
const COORDINATED_REASONS: [&str; 6] = ["same_funder", "funded_by_flagged", "relay_buy", "relay_split", "cex_burst", "throwaway_funded"];
/// A late bundle: transactions after the launch's first second in which several wallets buy at once. Only one party can
/// sign for all of them, so it's a bundle whenever it lands. It counts from this many wallets and this share of the
/// supply across those transactions. $GTA (EDeL…pump, 2026-10-22): its creator bought alone at launch and sold at 55
/// seconds; at 153 seconds three transactions signed by 16 wallets, funded with the creator 24 seconds before launch,
/// bought 79% of the supply, the whole curve, the creator migrated it in the same block, and a fourth signed by 5 more
/// bought 13% from the new pool. It read "0% bundled"; it reads 92%, 2 minutes after launch.
pub const LATE_BUNDLE_MIN_WALLETS: usize = 3;
pub const LATE_BUNDLE_MIN_PCT: f64 = 5.0;
/// Wallets the launch's side paid for are a red flag once they bought or hold this share of the supply, and weigh as a
/// big bundle does from the second.
const LAUNCH_FUNDED_MIN_PCT: f64 = 1.0;
const LAUNCH_FUNDED_BIG_PCT: f64 = 20.0;
/// A mint or freeze authority kept: the risk is at least high whatever else the coin shows.
const MINT_WEIGHT: u32 = 40;
/// Hand-offs count only where they beat chance. The same insider sells, moved this many seconds earlier or later, are
/// matched against the buys there; on a busy coin they match about as often, and the real matches are coincidence. On
/// KURP (2026-10-16) the rule matched 71% of insider sells, and 69% of the same sells at other moments.
const CHANCE_SHIFTS: [i64; 4] = [-180, -90, 90, 180];
/// A layer of hand-offs is kept only while chance explains under this share of its matches.
pub const RELAY_CHANCE_MAX: f64 = 0.5;
/// A crowd of new wallets: of the busiest traders and top holders checked, at least this many, wallets made after
/// launch (their first transaction ever came after it) doing at least this share of their trading. On 2026-10-16 KURP's
/// were 68% of its 200 busiest wallets' trading, a clean coin of its size (FTR) 0%.
pub const NEW_CROWD_MIN_WALLETS: usize = 50;
pub const NEW_CROWD_PCT: f64 = 40.0;
/// And at least this many new wallets, so one new whale isn't a crowd.
pub const NEW_CROWD_MIN_NEW: usize = 20;
/// Wallets bought with SOL from other wallets trading the coin: from this many up, a red flag, weighing most while they
/// hold this share of the supply. Out already, they were one operator's many wallets (a sniper's, on STREAM), not a
/// crowd waiting to sell.
pub const RECYCLED_MIN_WALLETS: usize = 3;
pub const RECYCLED_HOLD_PCT: f64 = 5.0;
/// A crew: at least this many of a launch's insiders were insiders together on the same earlier launch. On bundled
/// launches up to 2026-10-17, the creator was out within a minute 92% of the time with one, against 61% without.
pub const CREW_MIN_WALLETS: i64 = 3;
/// The roles that count toward a crew, on this launch and the earlier ones: insiders proper, as the site's "Insiders"
/// filter counts them. Hand-offs added almost no crews (3,180 bundled launches against 3,116) and include busy trading
/// bots; snipers, back-runs and first-second buyers are what every launch draws. A late bundle's wallets are a bundle's.
pub const CREW_ROLES: [&str; 7] = ["creator", "leftover", "kept", "bundle", "late_bundle", "proxima", "sumo"];

/// Roles that make a wallet an insider, most telling first.
const INSIDER_ROLES: [&str; 9] = ["creator", "leftover", "kept", "bundle", "funded", "proxima", "relay", "first_second", "transfer"];
/// Insiders proper: the creator, the wallet a DBC curve keeps its leftover for, a wallet that kept supply outside a
/// pool when it opened (\`SetAside::held_by\`), the creator's bundle, wallets the launch's own side paid for
/// (\`LaunchFunded\`) and Proxima.
const CORE_ROLES: [&str; 6] = ["creator", "leftover", "kept", "bundle", "funded", "proxima"];

/// A presale's contract and nothing else (`presale`, from the launch's scan): it bought at launch for the people who
/// paid into it, so neither it nor its payouts are an insider's.
pub fn only_presale(roles: &[String]) -> bool {
    !roles.is_empty() && roles.iter().all(|r| r == "presale")
}

/// One transaction from the launch's first second.
#[derive(Debug, Clone)]
pub struct Block0Tx {
    pub idx: Option<u64>,
    /// create, bundle (the creator's), coordinated (another multi-wallet bundle) or other.
    pub kind: String,
    pub wallets: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Known {
    /// Other launches this wallet was caught on.
    pub launches: i64,
    /// Of those, launches run through Proxima.
    pub proxima_launches: i64,
}

pub struct Input<'a> {
    pub mint: &'a str,
    pub creator: &'a str,
    /// The create transaction: what the chain times it at is the launch's second (`launch_second`).
    pub create_sig: &'a str,
    pub created_at: i64,
    pub supply: u64,
    /// Accounts that hold supply for the market, not for a person.
    pub curve_and_pools: HashSet<String>,
    /// Claim vaults of a token migration (migrate.fun) among the holders: supply
    /// kept for holders of the old token to claim. Also in `curve_and_pools`.
    pub claim_vaults: HashSet<String>,
    pub trades: &'a [Trade],
    /// Roles recorded at launch (creator, bundle, first_second, proxima).
    pub roles: HashMap<String, Vec<String>>,
    /// Current balances by owner, raw units.
    pub holders: &'a [(String, u64)],
    pub known: HashMap<String, Known>,
    pub bundled_pct: f64,
    pub proxima: bool,
    pub proxima_later: bool,
    pub router_txs: i64,
    /// Seconds from launch to the first Proxima router trade.
    pub proxima_after_secs: Option<i64>,
    pub proxima_wallets: i64,
    pub offchain_creator: bool,
    pub dev_launches_7d: i64,
    pub dev_bundled_7d: i64,
    /// The creator's launch buy, as a share of the supply (0 without one).
    pub dev_pct: f64,
    /// The creator's earlier launches and how fast it got out of them; None when it has none we watched.
    pub creator_record: Option<CreatorRecord>,
    /// This launch's insiders that were insiders together before (`db::crew`); None when no crew was found.
    pub crew: Option<Crew>,
    /// The crew's wallets, and on how many of the crew's earlier launches each one was: as more than their creator (as
    /// "caught" counts, so never more than `Known::launches`), and as only their creator.
    pub crew_wallets: HashMap<String, (i64, i64)>,
    /// Funding of the wallets we traced: insiders' before launch, and new wallets' before their first buy.
    pub funding: HashMap<String, Funding>,
    /// Each checked wallet's earliest transaction we found (unix seconds): before `created_at`, it existed before the
    /// launch. A busy coin's busiest traders and top holders only (`report::wallet_ages`); empty elsewhere.
    pub first_seen: HashMap<String, i64>,
    /// The first second's transactions in block order, when it was read.
    pub block0: Vec<Block0Tx>,
    /// Direct token transfers in the first hours.
    pub transfers: &'a [Transfer],
    /// Tokens claimed from Streamflow vesting: (wallet, raw amount).
    pub streamflow_claims: Vec<(String, u64)>,
    /// Tokens a wallet locked into a Streamflow contract. The escrow holds them; it is not another person.
    pub stream_locks: Vec<StreamLock>,
    /// Escrow accounts from those locks. They hold supply for the contract, so they are not holders.
    pub lock_escrows: HashSet<String>,
    /// Supply burned since launch, raw: the launch's supply less what the mint holds now. Nobody holds it.
    pub burned: u64,
    /// What a Meteora DBC curve's config keeps from buyers for one wallet; None elsewhere.
    pub set_aside: Option<SetAside>,
    /// The `bundle` wallets are ones the creator declared at launch, not a Jito bundle: Pons exempts the wallets a
    /// creator names from its snipe tax. False on Solana.
    pub declared_bundle: bool,
    /// The `bundle` wallets bought with the creator as a Uniswap v4 pool opened, not in a Jito bundle: in its opening
    /// transaction, or in its first second funded together (Pools.trade and hand launches). False on
    /// Solana and Pons.
    pub opening_bundle: bool,
    /// Liquidity taken out of a coin's launch pool after trading began (a Uniswap v4 launch); None elsewhere.
    pub liquidity_pulled: Option<LiquidityPulled>,
    /// Look for a late bundle (`late_bundle`). Solana: there one transaction's buyers all signed it.
    pub late_bundles: bool,
    /// Transactions after the first second that Jito landed in one buying bundle (`late_bundle_bundle`, read by the
    /// server's late check): each wallet signed its own, so only Jito's record ties them together.
    pub late_bundle_sigs: HashSet<String>,
    /// What the mint says now: None where it wasn't read (an EVM chain, or the read failed).
    pub mint_check: Option<MintCheck>,
    /// Wallets that bought with SOL from the launch's own side, or paid for the launch (\`LaunchFunded\`).
    pub launch_funded: HashMap<String, LaunchFunded>,
}

/// The coin's mint as it stands. A launchpad gives up both authorities at launch; a creator keeping the mint authority
/// can make more of the coin and sell it whenever it likes, and one keeping the freeze authority can stop any holder
/// from selling. $HAMMER (FLXu…1ydk, a Meteora DBC launch, 2026-09-28) kept its mint authority and had minted 9.5% of
/// its supply since.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MintCheck {
    /// A wallet other than the launchpad's own program can mint more, or freeze holders' tokens.
    pub can_mint: bool,
    pub can_freeze: bool,
    /// What was minted after the launch, as a share of the supply now.
    pub minted_pct: f64,
}

/// A wallet tied to the launch by SOL: it got SOL from the launch's own side before its first buy (\`by\` the creator,
/// the wallet that signed the create, or the wallet that funded either), or it is that funder and bought itself. Such a
/// wallet is the launch's, an insider, whenever it bought. $HAMMER: the wallet that paid for the create got 3.1 SOL from
/// CRYq…LV5 82 minutes before; CRYq…LV5 sent 0.5 SOL to each of 9 new wallets 33 to 35 minutes before; they bought 2 to
/// 22 hours after launch, spread out, and held 40% of the supply, read "insiders 0%".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchFunded {
    /// Who sent the SOL; for the funder itself, the launch's wallet it paid.
    pub by: String,
    pub time: i64,
    pub sol: f64,
    /// It funded the launch's wallet, rather than being funded by the launch's side.
    #[serde(default)]
    pub funded_launch: bool,
}

/// Wallets that bought together after the launch's first second, several in one transaction (`LATE_BUNDLE_MIN_WALLETS`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LateBundle {
    /// Seconds from launch to its first transaction.
    pub after_secs: i64,
    /// What it adds to the bundled share: what they bought in those transactions, each token once, so not what the
    /// launch's bundle wallets sold back before it (`late_bundle_in`). Every total counts this.
    pub pct: f64,
    /// What they bought in those transactions, as a share of the supply: what the red flag and the timeline tell.
    pub bought_pct: f64,
    pub wallets: usize,
    pub txs: usize,
    pub sol: f64,
    /// The creator was one of them.
    pub creator: bool,
}

/// Liquidity taken out of a coin's launch pool after trading began: the coin taken out (raw), the quote (whole ETH or
/// dollars of USDG) and whether its creator took it.
#[derive(Debug, Clone)]
pub struct LiquidityPulled {
    pub raw: u64,
    pub quote: f64,
    pub quote_is_usd: bool,
    pub by_creator: bool,
}

/// The creator's launches in the 7 days before this one whose launch buy was watched (`devcheck`), and on how many of
/// them those tokens were gone by the 2-minute check (`db::creator_record`).
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatorRecord {
    pub launches: i64,
    pub fast: i64,
}

/// This launch's insiders that were insiders together before: the earlier launches where `CREW_MIN_WALLETS` or more of
/// them were (`db::crew`), how many of this launch's insiders that takes in, the most of them on one launch, and of the
/// launches with a watched creator buy, on how many those tokens were gone by the 2-minute check. `recent`: the latest
/// of those launches, for the report to link.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub wallets: i64,
    pub launches: i64,
    pub most: i64,
    pub watched: i64,
    pub fast: i64,
    pub recent: Vec<CrewLaunch>,
}

/// One of a crew's earlier launches: the coin, and how many of this launch's insiders were insiders on it.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewLaunch {
    pub mint: String,
    pub symbol: String,
    pub wallets: i64,
}

/// Supply kept from buyers: a DBC config's leftover (`dbc::Config::set_aside`), the wallet it goes to once the curve
/// fills and how much (raw); or, for a coin that launched in a pool (Uniswap v4), the supply wallets held outside the
/// pool when trading began (`held_by`), with the largest holder as `wallet`.
#[derive(Debug, Clone)]
pub struct SetAside {
    pub wallet: String,
    pub raw: u64,
    /// The share of the supply the curve sells to buyers, when the config says.
    pub curve_sells_pct: Option<f64>,
    /// Who held it outside the pool, and how much (raw): empty for a DBC curve. They got it without buying it, as a
    /// leftover taken out of a curve is, so it counts with what they took: what they sell or send on of it shows.
    pub held_by: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reason {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "ref")]
    pub reference: Option<String>,
}

impl Reason {
    fn new(code: &'static str) -> Self {
        Self {
            code,
            value: None,
            reference: None,
        }
    }
    fn value(mut self, v: f64) -> Self {
        self.value = Some(v);
        self
    }
    fn reference(mut self, r: &str) -> Self {
        self.reference = Some(r.to_string());
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletStat {
    pub address: String,
    pub roles: Vec<String>,
    pub bought_pct: f64,
    pub sold_pct: f64,
    pub hold_pct: f64,
    pub sol_in: f64,
    pub sol_out: f64,
    pub buys: usize,
    pub sells: usize,
    /// Seconds after launch of the first buy.
    pub first_buy_after: Option<i64>,
    /// Supply it took out of a curve's vault without buying it (a DBC curve's leftover), as a share of the supply.
    #[serde(skip_serializing_if = "is_zero")]
    pub taken_pct: f64,
    pub other_launches: i64,
    pub reasons: Vec<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding: Option<Funding>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub owner: String,
    pub pct: f64,
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// What creator and bundle wallets hold now.
    pub insiders_hold_pct: f64,
    pub insiders_holding: usize,
    /// Share of what the creator and bundle bought that they've since sold.
    pub bundle_sold_pct: f64,
    /// Share of what the creator and bundle bought that left their wallets without a sale: sent on.
    pub bundle_moved_pct: f64,
    /// Share of what they bought that is still locked in a Streamflow contract, and that lock as a share of supply.
    pub bundle_locked_pct: f64,
    pub locked_pct: f64,
    /// Of `locked_pct`, what flagged wallets locked on terms that let them take it back before it pays out
    /// (`LockTerms::can_take_back`). The risk counts it as theirs still; the supply keeps it as locked.
    pub locked_revocable_pct: f64,
    /// The same as a share of supply, and the creator's own part of it.
    pub moved_pct: f64,
    pub dev_moved_pct: f64,
    /// The largest group of holders on one identical balance, dust aside: how many, each one's share, all of theirs.
    pub split_wallets: usize,
    pub split_each_pct: f64,
    pub split_pct: f64,
    /// Everything flagged: insiders, snipers, hand-offs.
    pub flagged_hold_pct: f64,
    pub sniper_hold_pct: f64,
    pub dev_hold_pct: f64,
    pub top10_pct: f64,
    pub curve_pool_pct: f64,
    pub holder_count: usize,
    pub dust_holders: usize,
    pub snipers: usize,
    pub relays: usize,
    pub possible_relays: usize,
    /// Wallets sent tokens straight from flagged wallets, and the most layers deep.
    pub transfers: usize,
    pub transfer_layers: usize,
    pub dust_recipients: usize,
    /// The creator's cluster: wallets linked to it by bundle, funding or hand-offs.
    pub cluster_wallets: usize,
    pub cluster_hold_pct: f64,
    /// A DBC curve's leftover: what its config keeps from buyers for one wallet, what that wallet has taken out of the
    /// curve's vault so far and sold, and what still waits there, as shares of the supply. All 0 elsewhere.
    pub set_aside_pct: f64,
    pub set_aside_taken_pct: f64,
    pub set_aside_sold_pct: f64,
    pub set_aside_waiting_pct: f64,
    /// The share of the supply that curve sells to buyers, when its config says.
    pub curve_sells_pct: Option<f64>,
    /// The creator's earlier launches, when we watched any (`Input::creator_record`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_record: Option<CreatorRecord>,
    /// This launch's insiders that were insiders together before (`Input::crew`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crew: Option<Crew>,
    /// Hand-off matches dropped because the same sells matched about as often at other moments.
    pub relays_by_chance: usize,
    /// Wallets made after launch among the wallets checked (`Input::first_seen`), when any were.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_wallets: Option<NewWallets>,
    /// A bundle bought after the first second (`LateBundle`), when there was one. Its wallets are insiders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_bundle: Option<LateBundle>,
    /// Wallets bought with SOL from another wallet trading this coin (`recycled`), how many of those trace back to an
    /// insider, what they hold now, and how many wallets funded after launch were traced.
    pub recycled_wallets: usize,
    pub recycled_from_insiders: usize,
    pub recycled_hold_pct: f64,
    pub recycled_traced: usize,
}

/// Wallets made after launch, among a busy coin's busiest traders and top holders.
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewWallets {
    /// Wallets checked, and how many of them were made after launch.
    pub checked: usize,
    pub new: usize,
    /// The new wallets' share of the checked wallets' trading (SOL in and out), and the checked wallets' share of all of it.
    pub volume_pct: f64,
    pub checked_volume_pct: f64,
    /// Supply the new wallets hold now.
    pub hold_pct: f64,
    /// SOL bought minus SOL sold, by the new wallets and by the older wallets checked.
    pub new_net_sol: f64,
    pub old_net_sol: f64,
}

impl NewWallets {
    /// Enough wallets checked, enough of them new, doing at least `pct` of the checked wallets' trading.
    pub fn crowd(&self, pct: f64) -> bool {
        self.checked >= NEW_CROWD_MIN_WALLETS && self.new >= NEW_CROWD_MIN_NEW && self.volume_pct >= pct
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplySplit {
    pub insiders: f64,
    pub other_suspicious: f64,
    pub snipers: f64,
    /// Wallets on the split's identical balance that aren't flagged otherwise.
    pub split: f64,
    pub everyone_else: f64,
    pub curve_pool: f64,
    /// A migration platform's claim vault (see `Migration`).
    pub claim_vault: f64,
    /// A DBC curve's leftover still waiting in its vault for the wallet its config keeps it for.
    pub set_aside: f64,
    /// Still sitting in a Streamflow escrow: locked, not held by a person.
    pub locked: f64,
    /// Burned since launch: no longer exists, so nobody holds it.
    pub burned: f64,
}

/// One deposit into a Streamflow contract. `escrow` is the account that received the tokens; `contract` is the account
/// that holds that escrow's terms, when the deposit named it (`streamflow::contract_of`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamLock {
    pub wallet: String,
    pub raw: u64,
    pub escrow: String,
    pub time: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    /// That contract's terms as read for this build: they change as it pays out or is cancelled, so they are never
    /// kept. None when they weren't read: nothing is known about when it pays out or who can cancel it.
    #[serde(skip)]
    pub terms: Option<LockTerms>,
}

/// The token was moved over from an old one through a migration platform: its
/// claim vault took `pct` of the supply for holders of the old token, and
/// `left_pct` is still waiting there. None of it is a bundle or a person.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Migration {
    pub platform: &'static str,
    pub pct: f64,
    pub left_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub code: &'static str,
    pub weight: u32,
    pub params: BTreeMap<&'static str, f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineStep {
    pub stage: &'static str,
    pub time: Option<i64>,
    pub facts: BTreeMap<&'static str, f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub wallets: Vec<WalletStat>,
    pub holders: Vec<Holder>,
    pub summary: Summary,
    pub supply: SupplySplit,
    pub signals: Vec<Signal>,
    pub risk: &'static str,
    /// The risk from insiders alone, before a crash's floor: what the scorecard and the X bot judge, as a crash anyone
    /// can see would otherwise grade our verdicts by their own outcome.
    pub insider_risk: &'static str,
    pub risk_score: f64,
    pub timeline: Vec<TimelineStep>,
    pub clusters: Vec<Cluster>,
    pub proxima: ProximaInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migration: Option<Migration>,
    /// The account that holds an insider's lock. A Solscan link, not the vesting site.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_account: Option<String>,
    /// That lock's terms, when its contract was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_terms: Option<LockTerms>,
    /// The coin losing most of its market cap within minutes after a run, and who sold into it (`crash`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crash: Option<Crash>,
}

#[derive(Default)]
struct Flow {
    bought: u128,
    sold: u128,
    sol_in: f64,
    sol_out: f64,
    buys: usize,
    sells: usize,
    first_buy: Option<i64>,
    first_trade: Option<i64>,
}

pub fn analyze(inp: &Input) -> Report {
    let share = |raw: u128| pct(raw, inp.supply);

    // What every wallet did, from the trade history.
    let mut flow: HashMap<&str, Flow> = HashMap::new();
    for t in inp.trades {
        let f = flow.entry(t.wallet.as_str()).or_default();
        f.first_trade.get_or_insert(t.time);
        if t.is_buy {
            f.bought += t.tokens as u128;
            f.sol_in += t.sol();
            f.buys += 1;
            f.first_buy.get_or_insert(t.time);
        } else {
            f.sold += t.tokens as u128;
            f.sol_out += t.sol();
            f.sells += 1;
        }
    }

    let mut roles: HashMap<String, Vec<String>> = inp.roles.clone();
    let mut reasons: HashMap<String, Vec<Reason>> = HashMap::new();

    // A late bundle: its wallets are the bundle's, insiders as a launch bundle's are, whenever they bought.
    let late = if inp.late_bundles { late_bundle(inp) } else { None };
    if let Some((b, bought)) = &late {
        for w in bought.keys() {
            if w != inp.creator {
                add_role(&mut roles, w, "bundle");
            }
            reasons.entry(w.clone()).or_default().push(Reason::new("late_bundle").value(b.after_secs as f64));
        }
    }
    let late_bundle = late.map(|(b, _)| b);

    // Wallets the launch's own side paid for, and its funder buying itself: the launch's, whenever they bought. One that
    // holds without a buy among the trades read bought after them.
    let holding: HashSet<&str> = inp.holders.iter().filter(|(_, a)| *a > 0).map(|(o, _)| o.as_str()).collect();
    let bought = |w: &str| flow.get(w).is_some_and(|f| f.buys > 0) || holding.contains(w);
    for (w, f) in inp.launch_funded.iter().filter(|(w, _)| *w != inp.creator && bought(w)) {
        add_role(&mut roles, w, "funded");
        let minutes = ((inp.created_at - f.time) as f64 / 60.0).round();
        let code = if f.funded_launch { "funded_the_launch" } else { "launch_funded" };
        reasons.entry(w.clone()).or_default().push(Reason::new(code).value(minutes).reference(&f.by));
    }

    // Snipers: fast first buys from wallets not already flagged at launch.
    for (w, f) in &flow {
        if roles.contains_key(*w) {
            continue;
        }
        if let Some(t) = f.first_buy {
            let after = t - inp.created_at;
            if (0..=SNIPE_SECS).contains(&after) {
                add_role(&mut roles, w, "sniper");
                reasons
                    .entry(w.to_string())
                    .or_default()
                    .push(Reason::new("sniper_bot").value(after as f64));
            }
        }
    }

    // Tokens sent straight from flagged wallets: the receivers are linked to
    // them, and a receiver that passes them on again adds a layer. Tiny sends
    // are dust, used to inflate the holder count.
    let dust_raw = (inp.supply as f64 * DUST_PCT / 100.0) as u64;
    let mut layer_of: HashMap<String, usize> = HashMap::new();
    let mut dust_recipients: HashSet<String> = HashSet::new();
    let mut transfer_layers = 0;
    let mut sorted_transfers: Vec<&Transfer> = inp.transfers.iter().collect();
    sorted_transfers.sort_by_key(|t| t.time);
    // Supply taken out of a curve's vault without buying it (a DBC curve's leftover), by wallet.
    let mut taken: HashMap<&str, u128> = HashMap::new();
    for t in sorted_transfers {
        if inp.curve_and_pools.contains(&t.from) {
            if !inp.curve_and_pools.contains(&t.to) {
                *taken.entry(t.to.as_str()).or_default() += t.raw as u128;
            }
            continue;
        }
        // A pool isn't a person: tokens sent into one were sold into it, by a trade too odd to price.
        if inp.curve_and_pools.contains(&t.to) {
            let f = flow.entry(t.from.as_str()).or_default();
            f.sold += t.raw as u128;
            f.sells += 1;
            continue;
        }
        if !roles.get(&t.from).is_some_and(|r| !r.is_empty()) {
            continue;
        }
        // A presale paying out is the people who paid into it claiming their share, not an insider handing supply on.
        if only_presale(&roles[&t.from]) {
            continue;
        }
        if t.raw < dust_raw {
            dust_recipients.insert(t.to.clone());
            continue;
        }
        let layer = layer_of.get(&t.from).copied().unwrap_or(0) + 1;
        layer_of.entry(t.to.clone()).or_insert(layer);
        transfer_layers = transfer_layers.max(layer);
        add_role(&mut roles, &t.to, "transfer");
        reasons.entry(t.to.clone()).or_default().push(
            Reason::new("received_transfer")
                .value(pct(t.raw as u128, inp.supply))
                .reference(&t.from),
        );
    }
    for (w, raw) in &inp.streamflow_claims {
        add_role(&mut roles, w, "streamflow");
        reasons
            .entry(w.clone())
            .or_default()
            .push(Reason::new("streamflow_claim").value(pct(*raw as u128, inp.supply)));
    }
    // The wallet a DBC curve keeps its leftover for is an insider: from launch on, by its config, and for what it took.
    let planned = inp.set_aside.as_ref().filter(|sa| share(sa.raw as u128) >= LEFTOVER_MIN_PCT);
    // Supply held outside a pool when trading began is the holders' without buying it: an insider's, as a leftover is.
    let kept_by: HashSet<&str> = planned.iter().flat_map(|sa| sa.held_by.iter().map(|(w, _)| w.as_str())).collect();
    if let Some(sa) = planned.filter(|sa| !sa.held_by.is_empty()) {
        for (w, raw) in &sa.held_by {
            *taken.entry(w.as_str()).or_default() += *raw as u128;
            add_role(&mut roles, w, "kept");
            reasons.entry(w.clone()).or_default().push(Reason::new("kept_outside_pool").value(share(*raw as u128)));
        }
    } else if let Some(sa) = planned {
        add_role(&mut roles, &sa.wallet, "leftover");
        // What it gets, until it has taken it.
        if !taken.contains_key(sa.wallet.as_str()) {
            reasons
                .entry(sa.wallet.clone())
                .or_default()
                .push(Reason::new("gets_leftover").value(share(sa.raw as u128)));
        }
    }
    // On a coin that launched in a pool, supply taken out of it without buying it is liquidity taken out: kept from
    // buyers, as what was held outside the pool when trading began.
    let (took_role, took_code) = if inp.opening_bundle { ("kept", "took_from_pool") } else { ("leftover", "took_leftover") };
    for (w, raw) in taken.iter().filter(|(w, _)| !kept_by.contains(**w)) {
        if share(*raw) >= LEFTOVER_MIN_PCT || planned.is_some_and(|sa| sa.wallet == *w) {
            add_role(&mut roles, w, took_role);
            reasons.entry(w.to_string()).or_default().push(Reason::new(took_code).value(share(*raw)));
        }
    }

    // Hand-offs: an insider sells, and a wallet trading this token for the
    // first time buys the same amount back within seconds. Repeat so a
    // hand-off's own hand-off is caught too (layers).
    let mut relays = 0;
    let mut possible = 0;
    let mut layers = 0;
    let mut relays_by_chance = 0;
    let mut used_buys: HashSet<usize> = HashSet::new();
    let mut used_sells: HashSet<usize> = HashSet::new();
    let first_trade = |w: &str| flow.get(w).and_then(|f| f.first_trade);
    for layer in 1..=3 {
        let insiders: HashSet<String> = roles
            .iter()
            .filter(|(_, r)| r.iter().any(|x| INSIDER_ROLES.contains(&x.as_str())))
            .map(|(w, _)| w.clone())
            .collect();
        let sells: Vec<usize> = (0..inp.trades.len())
            .filter(|i| {
                let s = &inp.trades[*i];
                !s.is_buy && insiders.contains(&s.wallet) && !used_sells.contains(i)
            })
            .collect();
        // Each sell hands off at most once, and each buy is used once.
        let mut used = used_buys.clone();
        let mut found: Vec<(usize, Handoff)> = vec![];
        for &i in &sells {
            let s = &inp.trades[i];
            if let Some(h) = handoff(inp.trades, &first_trade, &insiders, &used, i + 1, s.time, s.tokens) {
                used.extend(h.buys());
                found.push((i, h));
            }
        }
        if found.is_empty() {
            break;
        }
        // The same sells at other moments: how many look-alike buybacks the market gives by chance.
        let by_chance = CHANCE_SHIFTS
            .iter()
            .map(|shift| {
                let mut used = used_buys.clone();
                sells
                    .iter()
                    .filter(|&&i| {
                        let s = &inp.trades[i];
                        let at = s.time + shift;
                        let from = inp.trades.partition_point(|t| t.time < at);
                        let h = handoff(inp.trades, &first_trade, &insiders, &used, from, at, s.tokens);
                        h.map(|h| used.extend(h.buys())).is_some()
                    })
                    .count()
            })
            .sum::<usize>() as f64
            / CHANCE_SHIFTS.len() as f64;
        if by_chance >= RELAY_CHANCE_MAX * found.len() as f64 {
            relays_by_chance += found.len();
            break;
        }
        for (i, h) in found {
            let s = &inp.trades[i];
            used_sells.insert(i);
            used_buys.extend(h.buys());
            match h {
                Handoff::Direct(j) => {
                    let b = &inp.trades[j];
                    add_role(&mut roles, &b.wallet, "relay");
                    reasons.entry(b.wallet.clone()).or_default().push(
                        Reason::new("relay_buy")
                            .value((b.time - s.time) as f64)
                            .reference(&s.wallet),
                    );
                    relays += 1;
                    layers = layer;
                }
                Handoff::Split(js) => {
                    for &j in &js {
                        let b = &inp.trades[j];
                        add_role(&mut roles, &b.wallet, "relay");
                        reasons.entry(b.wallet.clone()).or_default().push(
                            Reason::new("relay_split")
                                .value(js.len() as f64)
                                .reference(&s.wallet),
                        );
                    }
                    relays += 1;
                    layers = layer;
                }
                Handoff::Possible(j) => {
                    let b = &inp.trades[j];
                    add_role(&mut roles, &b.wallet, "suspect");
                    reasons
                        .entry(b.wallet.clone())
                        .or_default()
                        .push(Reason::new("possible_relay").reference(&s.wallet));
                    possible += 1;
                }
            }
        }
        if layers < layer {
            break;
        }
    }

    // Bought with SOL from another wallet trading this coin, often through throwaway wallets on the way: money from the
    // coin buying it again from a new wallet, a crowd of buyers that's really a few (KURP, 2026-10-16). Only funding after
    // launch counts, from a wallet that had traded the coin by then.
    let mut recycled_wallets: Vec<String> = vec![];
    let mut recycled_from_insiders = 0;
    let mut recycled_traced = 0;
    for (w, f) in &inp.funding {
        if f.time < inp.created_at || has_role(&roles, w, &CORE_ROLES) {
            continue;
        }
        recycled_traced += 1;
        let src = f.root.as_deref().unwrap_or(&f.funder);
        if src == w || inp.curve_and_pools.contains(src) || first_trade(src).is_none_or(|t| t > f.time) {
            continue;
        }
        let insider = has_role(&roles, src, &CORE_ROLES);
        recycled_from_insiders += insider as usize;
        add_role(&mut roles, w, "recycled");
        reasons.entry(w.clone()).or_default().push(
            Reason::new(if insider { "insider_recycled" } else { "recycled_funding" })
                .value(f.hops as f64)
                .reference(src),
        );
        recycled_wallets.push(w.clone());
    }

    // Current balances, with the curve and pools set aside.
    let holdings: HashMap<&str, u64> = inp.holders.iter().map(|(o, a)| (o.as_str(), *a)).collect();
    let held = |w: &str| holdings.get(w).copied().unwrap_or(0) as u128;
    let curve_pool: u128 = inp
        .holders
        .iter()
        .filter(|(o, _)| inp.curve_and_pools.contains(o) && !inp.claim_vaults.contains(o))
        .map(|(_, a)| *a as u128)
        .sum();
    // A migration's claim vault: what it holds, and what it took in (bought, or
    // sent to it) or has already paid out to claimers, whichever we saw more of.
    let vault_held: u128 = inp
        .holders
        .iter()
        .filter(|(o, _)| inp.claim_vaults.contains(o))
        .map(|(_, a)| *a as u128)
        .sum();
    let vault_received: u128 = if inp.claim_vaults.is_empty() {
        0
    } else {
        let bought: u128 = inp.claim_vaults.iter().filter_map(|v| flow.get(v.as_str())).map(|f| f.bought).sum();
        let sent_in: u128 = inp.transfers.iter().filter(|t| inp.claim_vaults.contains(&t.to)).map(|t| t.raw as u128).sum();
        let paid_out: u128 = inp.transfers.iter().filter(|t| inp.claim_vaults.contains(&t.from)).map(|t| t.raw as u128).sum();
        // A migrated token's creator is the migration's admin: what it bought and neither sold nor holds went into
        // the vault, even when the deposit and the claims came after the hours of transfers we read.
        let creator_out = flow
            .get(inp.creator)
            .map_or(0, |f| f.bought.saturating_sub(f.sold))
            .saturating_sub(holdings.get(inp.creator).copied().unwrap_or(0) as u128);
        (bought + sent_in).max(vault_held + paid_out).max(creator_out)
    };
    // A Streamflow escrow holds supply for its contract, not for a person. A wallet the analysis knows as someone (the
    // creator, an insider, any wallet with a role) or a curve or pool is never one: its balance is counted where that
    // puts it, so it can't be counted again as locked.
    let escrows: HashSet<&str> = inp
        .lock_escrows
        .iter()
        .map(String::as_str)
        .filter(|o| *o != inp.creator && !inp.curve_and_pools.contains(*o) && !roles.get(*o).is_some_and(|r| !r.is_empty()))
        .collect();
    let people: Vec<&(String, u64)> = inp
        .holders
        .iter()
        .filter(|(o, a)| *a > 0 && !inp.curve_and_pools.contains(o) && !escrows.contains(o.as_str()))
        .collect();
    let dust_raw = (inp.supply as f64 * DUST_PCT / 100.0) as u64;
    let dust_holders = people.iter().filter(|(_, a)| *a < dust_raw).count();
    let mut sorted: Vec<&(String, u64)> = people.clone();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let top10: u128 = sorted.iter().take(10).map(|(_, a)| *a as u128).sum();
    // One identical balance across many wallets: a supply split up to look like a crowd of buyers.
    let mut same: HashMap<u64, usize> = HashMap::new();
    for p in &people {
        if p.1 >= dust_raw {
            *same.entry(p.1).or_default() += 1;
        }
    }
    let split_amount: Option<u64> = same
        .iter()
        .filter(|(_, n)| **n >= SPLIT_MIN_WALLETS)
        .max_by_key(|(a, n)| (**n, **a))
        .map(|(a, _)| *a);
    let split_wallets = split_amount.map_or(0, |a| same[&a]);
    let flagged = |w: &str| roles.get(w).is_some_and(|r| !r.is_empty());
    let split_unflagged: u128 = people
        .iter()
        .filter(|p| Some(p.1) == split_amount && !flagged(&p.0))
        .map(|p| p.1 as u128)
        .sum();

    let contiguous = contiguous_after_create(&inp.block0);
    // Wallet stats for everyone flagged.
    let mut wallets: Vec<WalletStat> = roles
        .iter()
        .map(|(w, r)| {
            let f = flow.get(w.as_str());
            let known = inp.known.get(w).cloned().unwrap_or_default();
            let mut rs = reasons.remove(w).unwrap_or_default();
            for role in r {
                match role.as_str() {
                    "creator" => rs.insert(0, Reason::new("dev")),
                    "bundle" if inp.declared_bundle => rs.push(Reason::new("declared_bundle")),
                    "bundle" if inp.opening_bundle => rs.push(Reason::new("opening_bundle")),
                    "bundle" => rs.push(Reason::new("in_create_bundle")),
                    "first_second" => rs.push(Reason::new("block0_buy")),
                    "proxima" => {
                        rs.push(Reason::new("proxima_router"));
                        if let (true, Some(after)) = (inp.proxima_later, inp.proxima_after_secs) {
                            rs.push(Reason::new("proxima_later").value(after as f64));
                        }
                    }
                    _ => {}
                }
            }
            if known.launches > 0 {
                rs.push(Reason::new("known_bundler").value(known.launches as f64));
            }
            if let Some(&(insider, made)) = inp.crew_wallets.get(w) {
                if insider > 0 {
                    rs.push(Reason::new("crew").value(insider as f64));
                }
                if made > 0 {
                    rs.push(Reason::new("crew_creator").value(made as f64));
                }
            }
            if let Some(n) = block0_cotraders(&inp.block0, w) {
                rs.push(Reason::new("multi_wallet_bundle").value(n as f64));
            }
            if contiguous.contains(w.as_str()) {
                rs.push(Reason::new("contiguous_after_create"));
            }
            if r.iter().any(|x| x == "backrun") {
                rs.push(Reason::new("backrun_snipe"));
            }
            if let Some(after) = f.and_then(|f| f.first_buy).map(|t| t - inp.created_at) {
                if after > 0 && !r.iter().any(|x| x == "sniper") {
                    rs.push(Reason::new("early_buy").value(after as f64));
                }
            }
            WalletStat {
                address: w.clone(),
                roles: r.clone(),
                bought_pct: f.map_or(0.0, |f| share(f.bought)),
                sold_pct: f.map_or(0.0, |f| share(f.sold)),
                hold_pct: share(held(w)),
                sol_in: f.map_or(0.0, |f| f.sol_in),
                sol_out: f.map_or(0.0, |f| f.sol_out),
                buys: f.map_or(0, |f| f.buys),
                sells: f.map_or(0, |f| f.sells),
                first_buy_after: f.and_then(|f| f.first_buy).map(|t| t - inp.created_at),
                taken_pct: share(taken.get(w.as_str()).copied().unwrap_or(0)),
                other_launches: known.launches,
                reasons: rs,
                funding: inp.funding.get(w).cloned(),
            }
        })
        .collect();
    wallets.sort_by(|a, b| b.bought_pct.total_cmp(&a.bought_pct));

    let has = |w: &str, set: &[&str]| {
        roles
            .get(w)
            .is_some_and(|r| r.iter().any(|x| set.contains(&x.as_str())))
    };
    let core = CORE_ROLES;
    let insiders_hold: u128 = roles.keys().filter(|w| has(w, &core)).map(|w| held(w)).sum();
    let insiders_holding = roles.keys().filter(|w| has(w, &core) && held(w) > 0).count();
    // What insiders got: what they bought, and supply they took out of a curve's vault without buying it.
    let (core_bought, core_sold) = roles
        .keys()
        .filter(|w| has(w, &core))
        .filter_map(|w| flow.get(w.as_str()))
        .fold((0u128, 0u128), |(b, s), f| (b + f.bought, s + f.sold));
    let core_bought = core_bought + roles.keys().filter(|w| has(w, &core)).filter_map(|w| taken.get(w.as_str())).sum::<u128>();
    // Each wallet counts in one bucket only, most telling first.
    let suspicious = ["first_second", "relay", "suspect", "transfer", "streamflow", "recycled"];
    let other_suspicious: u128 = roles
        .keys()
        .filter(|w| !has(w, &core) && has(w, &suspicious))
        .map(|w| held(w))
        .sum();
    let sniper_hold: u128 = roles
        .keys()
        .filter(|w| !has(w, &core) && !has(w, &suspicious) && has(w, &["sniper", "backrun"]))
        .map(|w| held(w))
        .sum();
    // A presale's unclaimed supply is its contributors', not a flagged wallet's.
    let flagged_hold: u128 = roles
        .iter()
        .filter(|(_, r)| !only_presale(r))
        .map(|(w, _)| held(w))
        .sum::<u128>()
        + split_unflagged;
    // What insiders bought and neither sold nor still hold left their wallets another way: sent on. A migration's
    // claim vault and a Streamflow escrow are not other wallets: a lock stays locked.
    // Only a deposit into a real escrow (see `escrows`) is a lock.
    let locks: Vec<&StreamLock> = inp.stream_locks.iter().filter(|l| escrows.contains(l.escrow.as_str())).collect();
    let locked_of = |w: &str| -> u128 { locks.iter().filter(|l| l.wallet == w).map(|l| l.raw as u128).sum() };
    let escrow_held: u128 = inp
        .holders
        .iter()
        .filter(|(o, _)| escrows.contains(o.as_str()))
        .map(|(_, a)| *a as u128)
        .sum();
    // What insiders put in when they locked it; what is still locked can't be more than the escrows hold now.
    let core_locked_in: u128 = roles.keys().filter(|w| has(w, &core)).map(|w| locked_of(w)).sum();
    let core_locked = core_locked_in.min(escrow_held);
    let dev_locked = locked_of(inp.creator).min(escrow_held);
    // A lock its maker can take back at any time keeps nothing from it: what a flagged wallet locked that way, still in
    // the escrow, counts as that wallet's in the risk. Only a lock whose terms were read and say so; each escrow once.
    let revocable: u128 = escrows
        .iter()
        .filter(|e| {
            locks.iter().any(|l| {
                l.escrow == **e
                    && (l.wallet == inp.creator || flagged(&l.wallet))
                    && l.terms.as_ref().is_some_and(|t| t.can_take_back(&l.wallet))
            })
        })
        .map(|e| held(e))
        .sum();
    let core_moved = core_bought
        .saturating_sub(core_sold)
        .saturating_sub(insiders_hold)
        .saturating_sub(vault_received)
        .saturating_sub(core_locked);
    let dev_moved = flow
        .get(inp.creator)
        .map_or(0, |f| f.bought.saturating_sub(f.sold).saturating_sub(held(inp.creator)))
        .saturating_sub(vault_received)
        .saturating_sub(dev_locked);
    let snipers = roles
        .values()
        .filter(|r| r.iter().any(|x| x == "sniper" || x == "backrun"))
        .count();

    // A DBC curve's leftover: what the wallets it went to took and sold, and what still waits in the curve's vault.
    let leftover_wallets: Vec<&String> =
        roles.iter().filter(|(_, r)| r.iter().any(|x| x == "leftover" || x == "kept")).map(|(w, _)| w).collect();
    let set_aside_taken: u128 = leftover_wallets.iter().filter_map(|w| taken.get(w.as_str())).sum();
    let set_aside_sold: u128 = leftover_wallets
        .iter()
        .map(|w| {
            let f = flow.get(w.as_str());
            f.map_or(0, |f| f.sold).min(f.map_or(0, |f| f.bought) + taken.get(w.as_str()).copied().unwrap_or(0))
        })
        .sum();
    let set_aside_waiting: u128 = planned
        .map_or(0, |sa| (sa.raw as u128).saturating_sub(set_aside_taken))
        .min(held(crate::addr::DBC_POOL_AUTHORITY))
        .min(curve_pool);
    let mut summary = Summary {
        insiders_hold_pct: share(insiders_hold),
        insiders_holding,
        bundle_sold_pct: if core_bought > 0 {
            (core_sold as f64 / core_bought as f64 * 100.0).min(100.0)
        } else {
            0.0
        },
        bundle_moved_pct: if core_bought > 0 {
            (core_moved as f64 / core_bought as f64 * 100.0).min(100.0)
        } else {
            0.0
        },
        bundle_locked_pct: if core_bought > 0 {
            (core_locked as f64 / core_bought as f64 * 100.0).min(100.0)
        } else {
            0.0
        },
        locked_pct: share(escrow_held),
        locked_revocable_pct: share(revocable),
        moved_pct: share(core_moved),
        dev_moved_pct: share(dev_moved),
        split_wallets,
        split_each_pct: split_amount.map_or(0.0, |a| share(a as u128)),
        split_pct: split_amount.map_or(0.0, |a| share(a as u128 * split_wallets as u128)),
        flagged_hold_pct: share(flagged_hold),
        sniper_hold_pct: share(sniper_hold),
        dev_hold_pct: share(held(inp.creator)),
        top10_pct: share(top10),
        curve_pool_pct: share(curve_pool - set_aside_waiting),
        holder_count: people.len(),
        dust_holders,
        snipers,
        relays,
        possible_relays: possible,
        transfers: roles.values().filter(|r| r.iter().any(|x| x == "transfer")).count(),
        transfer_layers,
        dust_recipients: dust_recipients.len(),
        cluster_wallets: 0,
        cluster_hold_pct: 0.0,
        set_aside_pct: planned.map_or(0.0, |sa| share(sa.raw as u128)),
        set_aside_taken_pct: share(set_aside_taken),
        set_aside_sold_pct: share(set_aside_sold),
        set_aside_waiting_pct: share(set_aside_waiting),
        curve_sells_pct: inp.set_aside.as_ref().and_then(|sa| sa.curve_sells_pct),
        creator_record: inp.creator_record,
        crew: inp.crew.clone(),
        relays_by_chance,
        new_wallets: new_wallets(inp, &flow, &held, &roles),
        recycled_wallets: recycled_wallets.len(),
        recycled_from_insiders,
        recycled_hold_pct: share(recycled_wallets.iter().map(|w| held(w)).sum()),
        recycled_traced,
        late_bundle: late_bundle.clone(),
    };

    let insiders_pct = share(insiders_hold);
    let other_pct = share(other_suspicious);
    let sniper_pct = share(sniper_hold);
    let curve_pct = share(curve_pool - set_aside_waiting);
    let split_pct = share(split_unflagged);
    let vault_pct = share(vault_held);
    let waiting_pct = share(set_aside_waiting);
    let burned_pct = share(inp.burned as u128);
    let supply = SupplySplit {
        insiders: insiders_pct,
        other_suspicious: other_pct,
        snipers: sniper_pct,
        split: split_pct,
        curve_pool: curve_pct,
        claim_vault: vault_pct,
        set_aside: waiting_pct,
        everyone_else: (100.0 - insiders_pct - other_pct - sniper_pct - split_pct - curve_pct - vault_pct - waiting_pct - share(escrow_held) - burned_pct)
            .max(0.0),
        locked: share(escrow_held),
        burned: burned_pct,
    };
    let migration = (!inp.claim_vaults.is_empty()).then(|| Migration {
        platform: "migrate.fun",
        pct: share(vault_received),
        left_pct: vault_pct,
    });

    let holders = sorted
        .iter()
        .take(60)
        .map(|(o, a)| {
            let mut r = roles.get(o).cloned().unwrap_or_default();
            if Some(*a) == split_amount {
                r.push("split".to_string());
            }
            Holder {
                owner: o.clone(),
                pct: share(*a as u128),
                roles: r,
            }
        })
        .collect();

    funding_reasons(&mut wallets);
    let coordinated = coordinated_snipers(&wallets);
    let clusters = clusters(&wallets, inp.creator);
    if let Some(dev) = clusters.iter().find(|c| c.is_dev) {
        summary.cluster_wallets = dev.wallets;
        summary.cluster_hold_pct = dev.hold_pct;
    }

    let mut signals = signals(inp, &roles, &summary, layers, &flow);
    let insider_funding: HashMap<String, Funding> = wallets
        .iter()
        .filter(|w| w.roles.iter().any(|r| CORE_ROLES.contains(&r.as_str())))
        .filter_map(|w| w.funding.clone().map(|f| (w.address.clone(), f)))
        .collect();
    signals.extend(funding::signals(&insider_funding).into_iter().map(|s| Signal {
        code: s.code,
        weight: s.weight,
        params: s.params,
    }));
    if let Some((n, pct)) = coordinated {
        let weight = if pct >= 20.0 { 40 } else { 20 };
        signals.push(signal("coordinated_snipers", weight, &[("wallets", n as f64), ("pct", pct)]));
    }
    // A red flag once they bought or hold a share worth telling; a wallet that bought a sliver is still the launch's.
    let funded: Vec<&WalletStat> = wallets.iter().filter(|w| w.roles.iter().any(|r| r == "funded")).collect();
    let bought: f64 = funded.iter().map(|w| w.bought_pct).sum::<f64>().min(100.0);
    let hold: f64 = funded.iter().map(|w| w.hold_pct).sum::<f64>().min(100.0);
    if bought.max(hold) >= LAUNCH_FUNDED_MIN_PCT {
        let weight = if bought.max(hold) >= LAUNCH_FUNDED_BIG_PCT { 40 } else { 20 };
        let is_funder = |w: &&&WalletStat| inp.launch_funded.get(&w.address).is_some_and(|f| f.funded_launch);
        let funder = funded.iter().filter(is_funder).count();
        signals.push(signal(
            "launch_funded",
            weight,
            &[
                ("wallets", (funded.len() - funder) as f64),
                ("funder", funder as f64),
                ("boughtPct", bought),
                ("holdPct", hold),
            ],
        ));
    }
    if let Some(m) = inp.mint_check.as_ref() {
        if m.can_mint {
            signals.push(signal("mint_open", MINT_WEIGHT, &[("mintedPct", m.minted_pct)]));
        }
        if m.can_freeze {
            signals.push(signal("freeze_open", MINT_WEIGHT, &[]));
        }
    }
    if let Some(b) = &late_bundle {
        // What they bought together weighs, and is told; `counted` is what it adds to the bundled share.
        let weight = if b.bought_pct >= 20.0 { 40 } else { 20 };
        signals.push(signal(
            "late_bundle",
            weight,
            &[
                ("pct", b.bought_pct),
                ("counted", b.pct),
                ("afterSecs", b.after_secs as f64),
                ("wallets", b.wallets as f64),
                ("txs", b.txs as f64),
                ("sol", b.sol),
                ("creator", if b.creator { 1.0 } else { 0.0 }),
            ],
        ));
    }
    // A crash after a run is a red flag whoever sold into it: buyers lost most of what they paid within minutes.
    let crash = crash::crash(inp.trades, inp.supply, |w| has_role(&roles, w, &CORE_ROLES), |w| has_role(&roles, w, &["sniper"]));
    if let Some(c) = &crash {
        signals.push(signal(
            "crashed",
            CRASH_WEIGHT,
            &[
                ("dropPct", c.drop_pct),
                ("secs", (c.end - c.start) as f64),
                ("afterSecs", (c.start - inp.created_at) as f64),
                ("sellers", c.sellers as f64),
                ("soldPct", c.sold_pct),
                ("insiderSoldPct", c.insider_sold_pct),
                ("stillDown", if c.still_down() { 1.0 } else { 0.0 }),
            ],
        ));
    }
    signals.sort_by(|a, b| b.weight.cmp(&a.weight));
    // A curve's leftover still in its vault is as good as held by the wallet it's kept for, and a curve that kept most
    // of the supply from buyers stays rigged after that wallet has sold it: what it kept counts at least.
    let kept = summary.set_aside_pct.max(summary.set_aside_taken_pct);
    // Coordinated snipers took their share at launch as a bundle does, and a late bundle is one: they count with the
    // bundled share.
    let took_at_launch = inp.bundled_pct + coordinated.map_or(0.0, |(_, pct)| pct) + late_bundle.as_ref().map_or(0.0, |b| b.pct);
    // So is a lock its flagged maker can take back (`locked_revocable_pct`).
    let still_theirs = summary.flagged_hold_pct + summary.set_aside_waiting_pct + summary.locked_revocable_pct;
    let (mut risk, risk_score) = risk(took_at_launch, still_theirs.max(kept), &signals);
    let insider_risk = risk;
    // A creator who can still mint more or freeze holders can take buyers' money whenever it likes: at least high.
    let open = inp.mint_check.as_ref().is_some_and(|m| m.can_mint || m.can_freeze);
    if open && RISK_LEVELS.iter().position(|l| *l == "high") > RISK_LEVELS.iter().position(|l| *l == risk) {
        risk = "high";
    }
    // A crash it hasn't come back from reads at least high, whatever insiders hold: "low" on a coin down 96% read clean.
    if let Some(floor) = crash.as_ref().and_then(|c| c.risk_floor()) {
        if RISK_LEVELS.iter().position(|l| *l == floor) > RISK_LEVELS.iter().position(|l| *l == risk) {
            risk = floor;
        }
    }
    let locked_in_pct = if core_bought > 0 {
        (core_locked_in as f64 / core_bought as f64 * 100.0).min(100.0)
    } else {
        0.0
    };
    let timeline = timeline(inp, &roles, &flow, &summary, &locks, locked_in_pct);
    let proxima_wallets: Vec<&String> = roles
        .iter()
        .filter(|(_, r)| r.iter().any(|x| x == "proxima"))
        .map(|(w, _)| w)
        .collect();
    let proxima = ProximaInfo {
        score: if inp.proxima {
            100
        } else if inp.proxima_later {
            70
        } else {
            0
        },
        level: if inp.proxima {
            "confirmed"
        } else if inp.proxima_later {
            "after_launch"
        } else {
            "none"
        },
        wallets: proxima_wallets.len(),
        txs: inp.router_txs,
        front_wallets: proxima_wallets
            .into_iter()
            .filter(|w| roles[*w].iter().any(|x| x == "first_second" || x == "bundle"))
            .cloned()
            .collect(),
    };

    let lock = locks.iter().filter(|l| has_role(&roles, &l.wallet, &CORE_ROLES)).max_by_key(|l| l.raw);
    let lock_account = lock.map(|l| l.escrow.clone());
    let lock_terms = lock.and_then(|l| l.terms.clone());

    Report {
        wallets,
        holders,
        summary,
        supply,
        signals,
        risk,
        insider_risk,
        risk_score,
        timeline,
        clusters,
        proxima,
        migration,
        lock_account,
        lock_terms,
        crash,
    }
}

/// The second the launch's block is in, as the chain times its transactions: the create transaction's, when it's among
/// `trades`. pump.fun's create event carries its own clock, a second before its block's time on about one launch in
/// five (35 of 169 pump.fun reports, 2026-10-24; never more than one): the whole launch block is then timed a second
/// after `created_at`, and the launch's own bundle read as a late one ($KFINE, Qq5c…pump: 74% bundled at launch and the
/// same 74% again "1 seconds after it", 148%, in a verdict and a receipt on X). Without the create among them, the
/// second after `created_at` may be the launch's too, so it's taken for it: only a late bundle in that second is lost.
pub fn launch_second(trades: &[Trade], create_sig: &str, created_at: i64) -> i64 {
    trades.iter().find(|t| t.sig == create_sig).map_or(created_at + 1, |t| t.time.max(created_at))
}

/// A late bundle (`LATE_BUNDLE_MIN_WALLETS`) of a launch's trades: buys after the launch's second (`launch_second`) in
/// transactions where two or more wallets bought, or that Jito landed in one buying bundle (`jito_sigs`, from
/// `late_bundle_bundle`), and what each wallet bought in them (raw). `pools` are the curve and pools, never a buyer.
/// How long after launch it was is counted from `created_at`, as everything else on the launch is.
///
/// Its share counts each token once (`bundle::count_once`): what the launch's bundle wallets (`launch_bundle`) or its
/// own wallets sold back before it bought counts for them only. $NODF (5saJ…pump, 2026-10-22) read "174% bundled": its
/// launch bundle took 96% and sold it back, and 9 minutes later 1,920 wallets bought 78% together. It's still a bundle,
/// its wallets insiders: only its share isn't counted again. And it adds at most what the launch's bundles (`launch_pct`)
/// left of the supply: a busy coin's history is sampled, and sells it didn't read can't count as sold back ($NODF again:
/// 2,200 wallets "bought 96.4%" for 3.3 SOL in its wash trading, its launch bundle's sells unread, 193%).
#[allow(clippy::too_many_arguments)]
pub fn late_bundle_in(
    trades: &[Trade],
    created_at: i64,
    launch_second: i64,
    creator: &str,
    supply: u64,
    pools: &HashSet<String>,
    jito_sigs: &HashSet<String>,
    launch_bundle: &HashSet<String>,
    launch_pct: f64,
) -> Option<(LateBundle, BTreeMap<String, u128>)> {
    let mut by_tx: HashMap<&str, Vec<&Trade>> = HashMap::new();
    for t in trades.iter().filter(|t| t.is_buy && t.time > launch_second.max(created_at) && !pools.contains(&t.wallet)) {
        by_tx.entry(t.sig.as_str()).or_default().push(t);
    }
    let mut bought: BTreeMap<String, u128> = BTreeMap::new();
    let (mut txs, mut sol, mut first) = (0, 0.0, i64::MAX);
    let mut counted: HashSet<&str> = HashSet::new();
    for (sig, buys) in &by_tx {
        if buys.iter().map(|t| t.wallet.as_str()).collect::<HashSet<_>>().len() < 2 && !jito_sigs.contains(*sig) {
            continue;
        }
        counted.insert(sig);
        txs += 1;
        for t in buys {
            *bought.entry(t.wallet.clone()).or_default() += t.tokens as u128;
            sol += t.sol();
            first = first.min(t.time);
        }
    }
    if bought.len() < LATE_BUNDLE_MIN_WALLETS || pct(bought.values().sum(), supply) < LATE_BUNDLE_MIN_PCT {
        return None;
    }
    // In chain order: the launch bundle's buys in the launch's second (counted at launch, group 1), the late bundle's
    // (group 0), and what any of them sold back, up to what it held from those.
    let mut order: Vec<&Trade> = trades.iter().collect();
    order.sort_by_key(|t| t.time);
    let mut held: HashMap<&str, u128> = HashMap::new();
    let mut flows = vec![];
    for t in order {
        let at_launch = t.time <= launch_second.max(created_at) && launch_bundle.contains(&t.wallet);
        if t.is_buy && (at_launch || counted.contains(t.sig.as_str())) {
            *held.entry(t.wallet.as_str()).or_default() += t.tokens as u128;
            flows.push(Move::Buy(if at_launch { 1 } else { 0 }, t.tokens as u128));
        } else if !t.is_buy {
            if let Some(h) = held.get_mut(t.wallet.as_str()) {
                let back = (*h).min(t.tokens as u128);
                *h -= back;
                // Both groups are bundles, each keeping what it bought (no plain group): which one sold doesn't matter.
                flows.push(Move::Sell(0, back));
            }
        }
    }
    let b = LateBundle {
        after_secs: first - created_at,
        pct: pct(count_once(&flows, 2, None)[0], supply).min((100.0 - launch_pct).max(0.0)),
        bought_pct: pct(bought.values().sum(), supply),
        wallets: bought.len(),
        txs,
        sol,
        creator: bought.contains_key(creator),
    };
    Some((b, bought))
}

fn late_bundle(inp: &Input) -> Option<(LateBundle, BTreeMap<String, u128>)> {
    let second = launch_second(inp.trades, inp.create_sig, inp.created_at);
    // The launch's bundle wallets, the creator with them when it bundled.
    let mut bundle: HashSet<String> = inp.roles.iter().filter(|(_, r)| r.iter().any(|x| x == "bundle")).map(|(w, _)| w.clone()).collect();
    if !bundle.is_empty() {
        bundle.insert(inp.creator.to_string());
    }
    late_bundle_in(inp.trades, inp.created_at, second, inp.creator, inp.supply, &inp.curve_and_pools, &inp.late_bundle_sigs, &bundle, inp.bundled_pct)
}

/// Blocks worth asking Jito about (`late_bundle_bundle`): after the launch's second (`launch_second`), where
/// `LATE_BUNDLE_MIN_WALLETS` or more wallets bought in two or more transactions, 1% of the supply or more between them.
/// One transaction per block, its biggest buy's, the blocks with the most bought first, at most `n`. `slot_of` gives a
/// transaction's block.
pub fn late_bundle_candidates(trades: &[Trade], slot_of: &HashMap<String, u64>, launch_second: i64, supply: u64, n: usize) -> Vec<String> {
    let mut by_slot: HashMap<u64, Vec<&Trade>> = HashMap::new();
    for t in trades.iter().filter(|t| t.is_buy && t.time > launch_second) {
        if let Some(slot) = slot_of.get(&t.sig) {
            by_slot.entry(*slot).or_default().push(t);
        }
    }
    let mut found: Vec<(u128, String)> = by_slot
        .values()
        .filter(|buys| buys.iter().map(|t| t.wallet.as_str()).collect::<HashSet<_>>().len() >= LATE_BUNDLE_MIN_WALLETS)
        .filter(|buys| buys.iter().map(|t| t.sig.as_str()).collect::<HashSet<_>>().len() >= 2)
        .map(|buys| (buys.iter().map(|t| t.tokens as u128).sum::<u128>(), buys))
        .filter(|(raw, _)| pct(*raw, supply) >= 1.0)
        .map(|(raw, buys)| (raw, buys.iter().max_by_key(|t| t.tokens).map(|t| t.sig.clone()).unwrap_or_default()))
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    found.into_iter().take(n).map(|(_, sig)| sig).collect()
}

/// Of a Jito bundle's transactions (`bundle_sigs`), those that bought the coin, when the bundle is a buying one: no
/// wallet sold the coin in it, and `LATE_BUNDLE_MIN_WALLETS` or more wallets bought. A bundle that sells is MEV, a
/// sandwich or a back-run around someone else's buy, and that buyer isn't the searcher's.
pub fn late_bundle_bundle(bundle_sigs: &[String], trades: &[Trade]) -> Vec<String> {
    let inside: Vec<&Trade> = trades.iter().filter(|t| bundle_sigs.contains(&t.sig)).collect();
    let buyers: HashSet<&str> = inside.iter().filter(|t| t.is_buy).map(|t| t.wallet.as_str()).collect();
    if inside.iter().any(|t| !t.is_buy) || buyers.len() < LATE_BUNDLE_MIN_WALLETS {
        return vec![];
    }
    let mut sigs: Vec<String> = inside.iter().map(|t| t.sig.clone()).collect::<HashSet<_>>().into_iter().collect();
    sigs.sort();
    sigs
}

/// Whether `w` has any of the roles in `set`.
fn has_role(roles: &HashMap<String, Vec<String>>, w: &str, set: &[&str]) -> bool {
    roles.get(w).is_some_and(|r| r.iter().any(|x| set.contains(&x.as_str())))
}

/// How an insider's sell came back.
enum Handoff {
    /// One fresh wallet bought the same amount back.
    Direct(usize),
    /// A few fresh wallets whose buys add up to it.
    Split(Vec<usize>),
    /// A weaker match: a similar amount, seconds later.
    Possible(usize),
}

impl Handoff {
    fn buys(&self) -> Vec<usize> {
        match self {
            Handoff::Direct(j) | Handoff::Possible(j) => vec![*j],
            Handoff::Split(js) => js.clone(),
        }
    }
}

/// The buyback of an insider's sell of `tokens` at `time`, among the trades from index `from` on: buys within
/// `RELAY_SECS` by wallets trading the token for the first time, not insiders and not used yet. The closest amount, then
/// the soonest; or a few buys that add up to it; or a weaker match.
fn handoff(
    trades: &[Trade],
    first_trade: &dyn Fn(&str) -> Option<i64>,
    insiders: &HashSet<String>,
    used: &HashSet<usize>,
    from: usize,
    time: i64,
    tokens: u64,
) -> Option<Handoff> {
    let candidates: Vec<(usize, &Trade)> = trades
        .iter()
        .enumerate()
        .skip(from)
        .take_while(|(_, b)| b.time - time <= RELAY_SECS)
        .filter(|(j, b)| {
            b.is_buy && !insiders.contains(&b.wallet) && !used.contains(j) && first_trade(&b.wallet) == Some(b.time)
        })
        .collect();
    let diff_of = |t: u64| (t as f64 - tokens as f64).abs() / (tokens.max(1) as f64);
    let best = candidates
        .iter()
        .map(|(j, b)| (*j, *b, diff_of(b.tokens)))
        .min_by(|a, b| a.2.total_cmp(&b.2).then((a.1.time - time).cmp(&(b.1.time - time))));
    if let Some((j, _, diff)) = best {
        if diff <= RELAY_TOLERANCE {
            return Some(Handoff::Direct(j));
        }
    }
    if let Some(split) = split_buyback(&candidates, tokens) {
        return Some(Handoff::Split(split.iter().map(|(j, _)| *j).collect()));
    }
    best.filter(|(_, b, diff)| *diff <= POSSIBLE_RELAY_TOLERANCE && b.time - time <= POSSIBLE_RELAY_SECS)
        .map(|(j, _, _)| Handoff::Possible(j))
}

/// Wallets made after launch among those checked (`Input::first_seen`), insiders aside: their share of the checked
/// wallets' trading, what they hold, and SOL in less SOL out, theirs and the older wallets'.
fn new_wallets(
    inp: &Input,
    flow: &HashMap<&str, Flow>,
    held: &dyn Fn(&str) -> u128,
    roles: &HashMap<String, Vec<String>>,
) -> Option<NewWallets> {
    if inp.first_seen.is_empty() {
        return None;
    }
    let volume = |f: &Flow| f.sol_in + f.sol_out;
    let all: f64 = flow.values().map(volume).sum();
    let mut out = NewWallets::default();
    let (mut checked_vol, mut new_vol, mut new_held) = (0.0, 0.0, 0u128);
    // Insiders are told on their own.
    for (w, first) in inp.first_seen.iter().filter(|(w, _)| !has_role(roles, w, &CORE_ROLES)) {
        let new = *first >= inp.created_at;
        out.checked += 1;
        out.new += new as usize;
        let f = flow.get(w.as_str());
        let v = f.map_or(0.0, volume);
        let net = f.map_or(0.0, |f| f.sol_in - f.sol_out);
        checked_vol += v;
        if new {
            new_vol += v;
            new_held += held(w);
            out.new_net_sol += net;
        } else {
            out.old_net_sol += net;
        }
    }
    out.volume_pct = if checked_vol > 0.0 { new_vol / checked_vol * 100.0 } else { 0.0 };
    out.checked_volume_pct = if all > 0.0 { checked_vol / all * 100.0 } else { 0.0 };
    out.hold_pct = pct(new_held, inp.supply);
    Some(out)
}

/// How sure we are the launch was run through Proxima, and which of its
/// wallets bought in the first second.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProximaInfo {
    pub score: u32,
    /// confirmed (router used at launch), after_launch, or none.
    pub level: &'static str,
    pub wallets: usize,
    pub txs: i64,
    /// Proxima wallets that also bought in the first second.
    pub front_wallets: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    pub id: usize,
    pub is_dev: bool,
    pub wallets: usize,
    pub hold_pct: f64,
    pub bought_pct: f64,
}

/// Where a wallet's SOL ultimately came from.
fn source(f: &Funding) -> &str {
    f.root.as_deref().unwrap_or(&f.funder)
}

/// Snipers that came in together: in within the launch's first seconds (`sniper`, `backrun`), not insiders already, and
/// linked by how they were funded or paid (`COORDINATED_REASONS`). Their number and what they bought, in % of the supply,
/// when they're enough to count (`COORDINATED_MIN_*`).
fn coordinated_snipers(wallets: &[WalletStat]) -> Option<(usize, f64)> {
    let linked: Vec<&WalletStat> = wallets
        .iter()
        .filter(|w| !w.roles.iter().any(|r| CORE_ROLES.contains(&r.as_str())))
        .filter(|w| w.roles.iter().any(|r| r == "sniper" || r == "backrun"))
        .filter(|w| w.reasons.iter().any(|r| COORDINATED_REASONS.contains(&r.code)))
        .collect();
    let pct = linked.iter().map(|w| w.bought_pct.min(100.0)).sum::<f64>().min(100.0);
    (linked.len() >= COORDINATED_MIN_WALLETS && pct >= COORDINATED_MIN_PCT).then_some((linked.len(), pct))
}

/// Per-wallet funding reasons: shared funders, funding from flagged wallets,
/// pass-throughs, exchange bursts, and wallets woken up for the launch.
fn funding_reasons(wallets: &mut [WalletStat]) {
    let flagged: HashSet<String> = wallets.iter().map(|w| w.address.clone()).collect();
    let funded: Vec<(String, Funding)> = wallets
        .iter()
        .filter_map(|w| w.funding.clone().map(|f| (w.address.clone(), f)))
        .collect();
    let mut by_source: HashMap<String, usize> = HashMap::new();
    for (_, f) in &funded {
        *by_source.entry(source(f).to_string()).or_default() += 1;
    }
    for w in wallets.iter_mut() {
        let Some(f) = w.funding.clone() else { continue };
        let src = source(&f).to_string();
        let shared = by_source.get(&src).copied().unwrap_or(1) - 1;
        // Exchanges and privacy pools fund everyone; sharing one isn't a link by itself.
        if shared > 0 && !labels::funds_everyone(&src) {
            w.reasons
                .push(Reason::new("same_funder").value(shared as f64).reference(&src));
        }
        if f.funder != w.address && flagged.contains(&f.funder) {
            w.reasons.push(Reason::new("funded_by_flagged").reference(&f.funder));
            // Not an insider itself, but funded by one: linked to the launch.
            if !w.roles.iter().any(|r| CORE_ROLES.contains(&r.as_str())) && !w.roles.iter().any(|r| r == "linked") {
                w.roles.push("linked".to_string());
            }
        }
        if f.hops > 0 {
            w.reasons.push(Reason::new("proxy_funding").reference(&src));
            if (0..=3600).contains(&f.seconds_before_launch) {
                w.reasons
                    .push(Reason::new("throwaway_funded").value((f.seconds_before_launch as f64 / 60.0).round()));
            }
        }
        if let Some(ex) = labels::exchange(&src) {
            let burst = funded.iter().find(|(other, g)| {
                *other != w.address
                    && labels::exchange(source(g)).as_deref() == Some(ex.as_str())
                    && (g.time - f.time).abs() <= funding::TOGETHER_SECS
            });
            if let Some((other, _)) = burst {
                w.reasons.push(Reason::new("cex_burst").reference(other));
            }
        }
        if let Some(days) = f.idle_days {
            if days >= funding::WOKEN_IDLE_DAYS && (0..=funding::WOKEN_FUNDED_WITHIN_SECS).contains(&f.seconds_before_launch) {
                w.reasons.push(Reason::new("woken_wallet").value(days.round()));
            }
        }
    }
}

/// Groups of flagged wallets linked by the creator's bundle, shared
/// non-exchange funding, funding from each other, or hand-offs.
fn clusters(wallets: &[WalletStat], creator: &str) -> Vec<Cluster> {
    let idx: HashMap<&str, usize> = wallets
        .iter()
        .enumerate()
        .map(|(i, w)| (w.address.as_str(), i))
        .collect();
    let mut parent: Vec<usize> = (0..wallets.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut c = i;
        while p[c] != r {
            let n = p[c];
            p[c] = r;
            c = n;
        }
        r
    }
    let union = |p: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            p[ra] = rb;
        }
    };
    let mut by_source: HashMap<&str, usize> = HashMap::new();
    for (i, w) in wallets.iter().enumerate() {
        if w.roles.iter().any(|r| r == "bundle") {
            if let Some(&c) = idx.get(creator) {
                union(&mut parent, i, c);
            }
        }
        if let Some(f) = &w.funding {
            let src = source(f);
            if !labels::funds_everyone(src) {
                match by_source.get(src) {
                    Some(&j) => union(&mut parent, i, j),
                    None => {
                        by_source.insert(src, i);
                    }
                }
            }
            if let Some(&j) = idx.get(f.funder.as_str()) {
                union(&mut parent, i, j);
            }
        }
        for r in w
            .reasons
            .iter()
            .filter(|r| r.code == "relay_buy" || r.code == "received_transfer")
        {
            if let Some(&j) = r.reference.as_deref().and_then(|s| idx.get(s)) {
                union(&mut parent, i, j);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..wallets.len() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<Cluster> = groups
        .into_values()
        .filter(|g| g.len() >= 2)
        .map(|g| Cluster {
            id: 0,
            is_dev: g.iter().any(|&i| wallets[i].address == creator),
            wallets: g.len(),
            hold_pct: g.iter().map(|&i| wallets[i].hold_pct).sum(),
            bought_pct: g.iter().map(|&i| wallets[i].bought_pct).sum(),
        })
        .collect();
    out.sort_by(|a, b| b.is_dev.cmp(&a.is_dev).then(b.wallets.cmp(&a.wallets)));
    for (i, c) in out.iter_mut().enumerate() {
        c.id = i + 1;
    }
    out
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn add_role(roles: &mut HashMap<String, Vec<String>>, wallet: &str, role: &str) {
    let e = roles.entry(wallet.to_string()).or_default();
    if !e.iter().any(|x| x == role) {
        e.push(role.to_string());
    }
}

fn signal(code: &'static str, weight: u32, params: &[(&'static str, f64)]) -> Signal {
    Signal {
        code,
        weight,
        params: params.iter().copied().collect(),
    }
}

fn signals(
    inp: &Input,
    roles: &HashMap<String, Vec<String>>,
    s: &Summary,
    relay_layers: usize,
    flow: &HashMap<&str, Flow>,
) -> Vec<Signal> {
    let mut out = vec![];
    let bundle_wallets = roles.values().filter(|r| r.iter().any(|x| x == "bundle")).count();
    let insiders: Vec<&String> = roles
        .iter()
        .filter(|(_, r)| r.iter().any(|x| CORE_ROLES.contains(&x.as_str())))
        .map(|(w, _)| w)
        .collect();

    if inp.proxima {
        out.push(signal(
            "proxima_router",
            100,
            &[("txs", inp.router_txs as f64), ("wallets", inp.proxima_wallets as f64)],
        ));
    } else if inp.proxima_later {
        out.push(signal(
            "proxima_after_launch",
            25,
            &[("wallets", inp.proxima_wallets as f64)],
        ));
    }
    if bundle_wallets > 0 {
        let code = if inp.declared_bundle {
            "declared_bundle"
        } else if inp.opening_bundle {
            "opening_bundle"
        } else {
            "front_mev_bundle"
        };
        out.push(signal(code, 15, &[("wallets", bundle_wallets as f64)]));
    }
    if s.relays > 0 {
        out.push(signal(
            "mev_relay",
            20,
            &[("edges", s.relays as f64), ("layers", relay_layers as f64)],
        ));
    }
    if s.possible_relays > 0 {
        out.push(signal("possible_relays", 5, &[("edges", s.possible_relays as f64)]));
    }
    if inp.offchain_creator {
        out.push(signal("offchain_creator", 15, &[]));
    }
    let known: Vec<&Known> = insiders
        .iter()
        .filter_map(|w| inp.known.get(*w))
        .filter(|k| k.launches > 0)
        .collect();
    if !known.is_empty() {
        out.push(signal(
            "known_bundler_wallets",
            10 + 5 * known.len().min(6) as u32,
            &[("wallets", known.len() as f64)],
        ));
    }
    let proxima_known = known.iter().filter(|k| k.proxima_launches > 0).count();
    if proxima_known > 0 {
        out.push(signal(
            "known_proxima_wallets",
            (10 * proxima_known as u32).min(60),
            &[("wallets", proxima_known as f64)],
        ));
    }
    if let Some(gap) = regular_buy_gap(inp, flow) {
        out.push(signal("regular_buy_delay", 10, &[("seconds", gap)]));
    }
    if (s.holder_count >= 20 && s.dust_holders * 3 >= s.holder_count) || s.dust_recipients >= 20 {
        let wallets = s.dust_holders.max(s.dust_recipients);
        out.push(signal("mass_distribution", 15, &[("wallets", wallets as f64)]));
    }
    if s.transfer_layers >= 2 {
        out.push(signal("temp_holder_layer", 10, &[("layers", s.transfer_layers as f64)]));
    }
    if s.moved_pct >= MOVED_MIN_PCT {
        out.push(signal(
            "supply_moved",
            20,
            &[("pct", s.moved_pct), ("ofBundle", s.bundle_moved_pct), ("dev", s.dev_moved_pct)],
        ));
    }
    if s.split_wallets >= SPLIT_MIN_WALLETS {
        out.push(signal(
            "split_supply",
            30,
            &[
                ("wallets", s.split_wallets as f64),
                ("each", s.split_each_pct),
                ("pct", s.split_pct),
            ],
        ));
    }
    if !inp.streamflow_claims.is_empty() {
        out.push(signal(
            "streamflow",
            10,
            &[("wallets", inp.streamflow_claims.len() as f64)],
        ));
    }
    // The supply a DBC curve keeps from buyers for one wallet: what its config says, or what was taken where the config
    // wasn't read.
    // On a coin that launched in a pool, only what was held outside it when trading began: liquidity taken out later is
    // its own flag (`liquidity_pulled`).
    let kept = if inp.opening_bundle { s.set_aside_pct } else { s.set_aside_pct.max(s.set_aside_taken_pct) };
    if kept >= LEFTOVER_MIN_PCT {
        let mut params = vec![("pct", kept), ("taken", s.set_aside_taken_pct), ("sold", s.set_aside_sold_pct)];
        params.extend(s.curve_sells_pct.map(|c| ("curve", c)));
        // Who gets it: the coin's own creator, or another wallet.
        if inp.set_aside.as_ref().is_some_and(|sa| sa.wallet == inp.creator) {
            params.push(("creator", 1.0));
        }
        // A coin that launched in a pool: how many wallets held it outside the pool when trading began.
        if let Some(sa) = inp.set_aside.as_ref().filter(|sa| inp.opening_bundle && !sa.held_by.is_empty()) {
            params.push(("pool", 1.0));
            params.push(("wallets", sa.held_by.len() as f64));
        }
        let weight = if kept >= 50.0 { 60 } else if kept >= 10.0 { 30 } else { 15 };
        out.push(signal("supply_set_aside", weight, &params));
    }
    // Liquidity taken out of the launch pool after trading began: the rug on a coin that launched in a pool. It weighs
    // as much as a creator known to sell out fast with a big buy (`creator_sold_fast`, 50), and what it took counts in
    // the risk as supply kept from buyers (`kept`).
    if let Some(p) = inp.liquidity_pulled.as_ref().filter(|p| pct(p.raw as u128, inp.supply) >= LEFTOVER_MIN_PCT || p.quote > 0.0) {
        let mut params = vec![("pct", pct(p.raw as u128, inp.supply)), ("quote", p.quote)];
        if p.quote_is_usd {
            params.push(("usd", 1.0));
        }
        if p.by_creator {
            params.push(("creator", 1.0));
        }
        out.push(signal("liquidity_pulled", 50, &params));
    }
    if inp.dev_launches_7d >= 3 {
        out.push(signal(
            "serial_launcher",
            // A creator that bundles its launches all week, not just one that launches a lot: up to 30 at 10 bundled.
            10 + 2 * inp.dev_bundled_7d.clamp(0, 10) as u32,
            &[
                ("nth", inp.dev_launches_7d as f64),
                ("bundled", inp.dev_bundled_7d as f64),
            ],
        ));
    }
    // The creator got out of its earlier launches within 2 minutes, most of the time. On 2026-10-16 such a creator was
    // out of the next one within a minute 80% of the time, against 36% for the rest.
    if let Some(r) = inp.creator_record.filter(|r| r.fast >= CREATOR_FAST_MIN && r.fast * 2 >= r.launches) {
        let weight = if inp.dev_pct > CREATOR_BIG_BUY_PCT {
            50
        } else if r.fast == r.launches && r.launches >= 3 {
            20
        } else {
            10
        };
        out.push(signal(
            "creator_sold_fast",
            weight,
            &[("fast", r.fast as f64), ("launches", r.launches as f64), ("dev", inp.dev_pct)],
        ));
    }
    // Insiders that were insiders together before: weighs more once they've done it on two or more launches.
    if let Some(c) = &inp.crew {
        out.push(signal(
            "same_crew",
            if c.launches >= 2 { 20 } else { 10 },
            &[
                ("wallets", c.wallets as f64),
                ("launches", c.launches as f64),
                ("most", c.most as f64),
                ("watched", c.watched as f64),
                ("fast", c.fast as f64),
            ],
        ));
    }
    // The buyers are mostly wallets made after launch: on a coin people found, most trading comes from wallets that
    // traded before (a clean coin of KURP's size: 0%).
    if let Some(n) = s.new_wallets.filter(|n| n.crowd(NEW_CROWD_PCT)) {
        out.push(signal(
            "new_wallet_crowd",
            20,
            &[
                ("pct", n.volume_pct),
                ("checked", n.checked as f64),
                ("new", n.new as f64),
                ("holdPct", n.hold_pct),
                ("newNet", n.new_net_sol),
                ("oldNet", n.old_net_sol),
            ],
        ));
    }
    if s.recycled_wallets >= RECYCLED_MIN_WALLETS {
        out.push(signal(
            "recycled_funding",
            match (s.recycled_from_insiders > 0, s.recycled_hold_pct >= RECYCLED_HOLD_PCT) {
                (true, true) => 35,
                (false, true) => 25,
                (true, false) => 20,
                (false, false) => 10,
            },
            &[
                ("wallets", s.recycled_wallets as f64),
                ("traced", s.recycled_traced as f64),
                ("insiders", s.recycled_from_insiders as f64),
                ("holdPct", s.recycled_hold_pct),
            ],
        ));
    }
    out.sort_by(|a, b| b.weight.cmp(&a.weight));
    out
}

/// Median gap between first buys in the first two minutes, if the gaps are
/// suspiciously even (a script, not people).
fn regular_buy_gap(inp: &Input, flow: &HashMap<&str, Flow>) -> Option<f64> {
    let mut firsts: Vec<i64> = flow
        .values()
        .filter_map(|f| f.first_buy)
        .filter(|t| (1..=120).contains(&(t - inp.created_at)))
        .collect();
    firsts.sort_unstable();
    let gaps: Vec<f64> = firsts
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64)
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.len() < 5 {
        return None;
    }
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    let var = gaps.iter().map(|g| (g - mean).powi(2)).sum::<f64>() / gaps.len() as f64;
    let cv = var.sqrt() / mean;
    (cv <= REGULAR_CV).then(|| {
        let mut g = gaps.clone();
        g.sort_by(f64::total_cmp);
        g[g.len() / 2]
    })
}

/// The risk levels, least first.
const RISK_LEVELS: [&str; 4] = ["low", "medium", "high", "extreme"];
/// A crash's red flag: a strong one (`weightTone`'s "bad").
const CRASH_WEIGHT: u32 = 40;

/// A score from how much was bundled, how much flagged wallets still hold,
/// and the red flags. Cut-offs fitted to the original's samples: 52% bundled
/// with Proxima and 2.5% still held was HIGH; 90% bundled with Sumo and 45%
/// flagged hold was EXTREME.
pub fn risk(bundled_pct: f64, flagged_hold_pct: f64, signals: &[Signal]) -> (&'static str, f64) {
    let weights: u32 = signals.iter().map(|s| s.weight).sum();
    let score = bundled_pct.min(100.0) * 0.6 + flagged_hold_pct.min(100.0) + (weights.min(100) as f64) * 0.3;
    let level = match score {
        s if s < 15.0 => "low",
        s if s < 40.0 => "medium",
        s if s < 90.0 => "high",
        _ => "extreme",
    };
    (level, score)
}

fn timeline(
    inp: &Input,
    roles: &HashMap<String, Vec<String>>,
    flow: &HashMap<&str, Flow>,
    s: &Summary,
    locks: &[&StreamLock],
    locked_in_pct: f64,
) -> Vec<TimelineStep> {
    let step = |stage, time, facts: &[(&'static str, f64)]| TimelineStep {
        stage,
        time,
        facts: facts.iter().copied().collect(),
    };
    let insider = |w: &str| {
        roles
            .get(w)
            .is_some_and(|r| r.iter().any(|x| CORE_ROLES.contains(&x.as_str())))
    };
    let insider_sells: Vec<&Trade> = inp.trades.iter().filter(|t| !t.is_buy && insider(&t.wallet)).collect();
    let first_exit = insider_sells.first().map(|t| t.time);
    let dev_pct = flow.get(inp.creator).map_or(0.0, |f| pct(f.bought, inp.supply));
    // A late bundle, in time order: before the first exit, or after the creator's when it had already sold.
    let late = s.late_bundle.as_ref().map(|b| {
        step(
            "late_bundle",
            Some(inp.created_at + b.after_secs),
            &[("pct", b.bought_pct), ("counted", b.pct), ("wallets", b.wallets as f64), ("secondsAfterLaunch", b.after_secs as f64)],
        )
    });
    let mut steps = vec![
        step("launch", Some(inp.created_at), &[("devBoughtPct", dev_pct)]),
        step("first_second", Some(inp.created_at), &[("bundledPct", inp.bundled_pct)]),
        step(
            "first_exit",
            first_exit,
            &[
                (
                    "secondsAfterLaunch",
                    first_exit.map_or(-1.0, |t| (t - inp.created_at) as f64),
                ),
                ("insiderSells", insider_sells.len() as f64),
                ("relays", s.relays as f64),
                ("movedPct", s.moved_pct),
                (
                    "lockAfter",
                    locks
                        .iter()
                        .filter(|l| insider(&l.wallet) && l.time > 0)
                        .map(|l| l.time)
                        .min()
                        .map_or(-1.0, |t| (t - inp.created_at) as f64),
                ),
            ],
        ),
        step(
            "now",
            None,
            &[
                ("insidersHoldPct", s.insiders_hold_pct),
                ("bundleSoldPct", s.bundle_sold_pct),
                ("bundleMovedPct", s.bundle_moved_pct),
                ("bundleLockedPct", s.bundle_locked_pct),
                // What they put in the contract when they locked it, of what they bought: more than is in it now once
                // some of it has come out.
                ("bundleLockedInPct", locked_in_pct),
                ("lockedPct", s.locked_pct),
                ("splitWallets", s.split_wallets as f64),
                ("splitPct", s.split_pct),
            ],
        ),
    ];
    if let Some(late) = late {
        let at = steps.iter().position(|x| x.stage == "first_exit" && x.time.is_some_and(|t| t > late.time.unwrap_or(0))).unwrap_or(3);
        steps.insert(at, late);
    }
    steps
}

/// Two or three buys from different wallets that together match `sold`
/// within the hand-off tolerance. Candidates are few: seconds after one sell.
fn split_buyback<'t>(candidates: &[(usize, &'t Trade)], sold: u64) -> Option<Vec<(usize, &'t Trade)>> {
    let c: Vec<&(usize, &Trade)> = candidates.iter().take(12).collect();
    let close = |sum: u64| (sum as f64 - sold as f64).abs() / (sold.max(1) as f64) <= RELAY_TOLERANCE;
    for x in 0..c.len() {
        for y in x + 1..c.len() {
            if c[x].1.wallet == c[y].1.wallet {
                continue;
            }
            if close(c[x].1.tokens + c[y].1.tokens) {
                return Some(vec![*c[x], *c[y]]);
            }
            for z in y + 1..c.len() {
                let distinct = c[z].1.wallet != c[x].1.wallet && c[z].1.wallet != c[y].1.wallet;
                if distinct && close(c[x].1.tokens + c[y].1.tokens + c[z].1.tokens) {
                    return Some(vec![*c[x], *c[y], *c[z]]);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPLY: u64 = 1_000_000_000_000_000;

    fn trade(time: i64, wallet: &str, is_buy: bool, pct_of_supply: f64) -> Trade {
        Trade {
            sig: format!("{wallet}-{time}"),
            time,
            wallet: wallet.into(),
            is_buy,
            lamports: 1_000_000_000,
            tokens: (SUPPLY as f64 * pct_of_supply / 100.0) as u64,
            virtual_sol: 30_000_000_000,
            virtual_tokens: 1_000_000_000_000_000,
            pool: None,
            generic: false,
            quote: None,
        }
    }

    fn input<'a>(trades: &'a [Trade], holders: &'a [(String, u64)]) -> Input<'a> {
        Input {
            mint: "M",
            creator: "dev",
            create_sig: "create",
            created_at: 1000,
            supply: SUPPLY,
            curve_and_pools: ["curve".to_string()].into(),
            claim_vaults: HashSet::new(),
            trades,
            roles: [
                ("dev".to_string(), vec!["creator".to_string()]),
                ("b1".to_string(), vec!["bundle".to_string()]),
            ]
            .into(),
            holders,
            known: HashMap::new(),
            bundled_pct: 12.0,
            proxima: false,
            proxima_later: false,
            router_txs: 0,
            proxima_after_secs: None,
            proxima_wallets: 0,
            offchain_creator: false,
            dev_launches_7d: 1,
            dev_bundled_7d: 1,
            dev_pct: 0.0,
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

    #[test]
    fn late_bundle_needs_wallets_buying_together_and_a_share() {
        let together = |time: i64, wallets: &[&str], pct_each: f64| -> Vec<Trade> {
            wallets
                .iter()
                .map(|w| Trade { sig: format!("tx-{time}"), ..trade(time, w, true, pct_each) })
                .collect()
        };
        let holders = vec![("dev".to_string(), 0u64)];
        let found = |trades: &[Trade]| analyze(&input(trades, &holders)).summary.late_bundle.map(|b| (b.wallets, b.txs, b.after_secs));
        // Three wallets in one transaction at 2 minutes, 6% between them.
        assert_eq!(found(&together(1120, &["a", "b", "c"], 2.0)), Some((3, 1, 120)));
        // Two transactions of two make four wallets.
        let two = [together(1120, &["a", "b"], 2.0), together(1121, &["c", "d"], 2.0)].concat();
        assert_eq!(found(&two), Some((4, 2, 120)));
        // Two wallets, or under 5% of the supply, aren't one.
        assert_eq!(found(&together(1120, &["a", "b"], 5.0)), None);
        assert_eq!(found(&together(1120, &["a", "b", "c"], 1.0)), None);
        // The same buys in separate transactions aren't.
        let apart: Vec<Trade> = ["a", "b", "c"].iter().map(|w| trade(1120, w, true, 2.0)).collect();
        assert_eq!(found(&apart), None);
        // In the first second, it's the launch's: the first-second read tells it.
        assert_eq!(found(&together(1000, &["a", "b", "c"], 2.0)), None);
    }

    #[test]
    fn a_late_bundle_counts_only_what_the_launch_bundle_had_not_sold_back() {
        // The launch bundle ("b", 50%) sells back 20% at 60 seconds; at 2 minutes three wallets buy 30% together.
        let launch = vec![Trade { sig: "launch".into(), ..trade(1000, "b", true, 50.0) }, trade(1060, "b", false, 20.0)];
        let late: Vec<Trade> = ["x", "y", "z"].iter().map(|w| Trade { sig: "late".into(), ..trade(1120, w, true, 10.0) }).collect();
        let trades = [launch, late].concat();
        let bundle: HashSet<String> = ["b".to_string()].into();
        let none = HashSet::new();
        let found = |sold: &HashSet<String>| late_bundle_in(&trades, 1000, 1000, "dev", SUPPLY, &none, &none, sold, 50.0).map(|(b, _)| b.pct);
        // Counted once: 10% beyond the 20% it sold back. Without the launch bundle's sells, all 30%.
        assert!(found(&bundle).is_some_and(|p| (p - 10.0).abs() < 1e-6), "{:?}", found(&bundle));
        assert!(found(&none).is_some_and(|p| (p - 30.0).abs() < 1e-6));
        // $NODF's shape: the launch bundle sold back more than the late one bought. Still a bundle, of 3 wallets that
        // bought 30%, adding nothing to the bundled share.
        let mut dumped = trades.clone();
        dumped[1] = trade(1060, "b", false, 50.0);
        let b = late_bundle_in(&dumped, 1000, 1000, "dev", SUPPLY, &none, &none, &bundle, 50.0).map(|(b, _)| b).expect("a late bundle");
        assert_eq!(b.wallets, 3);
        assert!(b.pct.abs() < 1e-6 && (b.bought_pct - 30.0).abs() < 1e-6, "{b:?}");
        // Its sells unread (a sampled history), it adds at most what the launch's 96% left: 4%, never past the supply.
        let unread: Vec<Trade> = trades.iter().filter(|t| t.is_buy).cloned().collect();
        let b = late_bundle_in(&unread, 1000, 1000, "dev", SUPPLY, &none, &none, &bundle, 96.0).map(|(b, _)| b).expect("a late bundle");
        assert!((b.pct - 4.0).abs() < 1e-6 && (b.bought_pct - 30.0).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn a_jito_bundle_of_single_buys_is_a_late_bundle_when_it_only_buys() {
        let holders = vec![("dev".to_string(), 0u64)];
        // Three wallets, each signing its own buy, in one block at 2 minutes.
        let apart: Vec<Trade> = ["a", "b", "c"].iter().map(|w| trade(1120, w, true, 2.0)).collect();
        let slots: HashMap<String, u64> = apart.iter().map(|t| (t.sig.clone(), 77)).collect();
        // One transaction of the block to ask Jito about.
        let asked = late_bundle_candidates(&apart, &slots, 1000, SUPPLY, 5);
        assert!(asked.len() == 1 && apart.iter().any(|t| t.sig == asked[0]), "{asked:?}");
        // Jito landed them in one bundle: a late bundle.
        let bundle: Vec<String> = apart.iter().map(|t| t.sig.clone()).collect();
        let sigs = late_bundle_bundle(&bundle, &apart);
        assert_eq!(sigs.len(), 3);
        let mut inp = input(&apart, &holders);
        inp.late_bundle_sigs = sigs.into_iter().collect();
        assert_eq!(analyze(&inp).summary.late_bundle.map(|b| (b.wallets, b.txs)), Some((3, 3)));
        // A bundle that also sells is MEV around someone else's buy: not one.
        let sandwich = [apart.clone(), vec![trade(1120, "a", false, 1.9)]].concat();
        let bundle: Vec<String> = sandwich.iter().map(|t| t.sig.clone()).collect();
        assert!(late_bundle_bundle(&bundle, &sandwich).is_empty());
        // Two buyers in a block aren't worth asking about, nor buys in separate blocks, nor the first second.
        let two: Vec<Trade> = ["a", "b"].iter().map(|w| trade(1120, w, true, 3.0)).collect();
        let slots2: HashMap<String, u64> = two.iter().map(|t| (t.sig.clone(), 77)).collect();
        assert!(late_bundle_candidates(&two, &slots2, 1000, SUPPLY, 5).is_empty());
        let spread: HashMap<String, u64> = apart.iter().enumerate().map(|(i, t)| (t.sig.clone(), 77 + i as u64)).collect();
        assert!(late_bundle_candidates(&apart, &spread, 1000, SUPPLY, 5).is_empty());
        let launch: Vec<Trade> = ["a", "b", "c"].iter().map(|w| trade(1000, w, true, 2.0)).collect();
        let slots3: HashMap<String, u64> = launch.iter().map(|t| (t.sig.clone(), 70)).collect();
        assert!(late_bundle_candidates(&launch, &slots3, 1000, SUPPLY, 5).is_empty());
    }

    #[test]
    fn creator_record_flags_a_creator_that_sells_out_fast() {
        let trades = vec![trade(1000, "dev", true, 2.0), trade(1000, "b1", true, 10.0)];
        let holders = vec![("dev".to_string(), 0u64)];
        let weight = |launches: i64, fast: i64, dev_pct: f64| {
            let mut inp = input(&trades, &holders);
            inp.creator_record = Some(CreatorRecord { launches, fast });
            inp.dev_pct = dev_pct;
            let r = analyze(&inp);
            assert_eq!(r.summary.creator_record.map(|c| (c.launches, c.fast)), Some((launches, fast)));
            r.signals.iter().find(|s| s.code == "creator_sold_fast").map(|s| s.weight)
        };
        // Under 2 fast, or under half: no flag.
        assert_eq!(weight(1, 1, 2.0), None);
        assert_eq!(weight(5, 2, 2.0), None);
        // Half or more, at least 2.
        assert_eq!(weight(4, 2, 2.0), Some(10));
        assert_eq!(weight(2, 2, 2.0), Some(10));
        // Every one of 3 or more.
        assert_eq!(weight(12, 12, 2.0), Some(20));
        // Buying over 4% again.
        assert_eq!(weight(12, 11, 6.0), Some(50));
        assert_eq!(weight(4, 2, 4.0), Some(10));
        // A clean record is kept for the report, without a flag.
        assert_eq!(weight(6, 0, 6.0), None);
    }

    #[test]
    fn crew_flags_the_launch_and_notes_its_wallets() {
        let trades = vec![trade(1000, "dev", true, 2.0), trade(1000, "b1", true, 10.0)];
        let holders = vec![("dev".to_string(), 0u64)];
        let mut inp = input(&trades, &holders);
        let r = analyze(&inp);
        assert!(r.summary.crew.is_none() && !r.signals.iter().any(|s| s.code == "same_crew"));

        let crew = |launches| Crew { wallets: 3, launches, most: 3, watched: 1, fast: 1, recent: vec![] };
        inp.crew = Some(crew(1));
        inp.crew_wallets = [("dev".to_string(), (0, 1)), ("b1".to_string(), (1, 0))].into();
        let r = analyze(&inp);
        assert_eq!(r.signals.iter().find(|s| s.code == "same_crew").map(|s| s.weight), Some(10));
        assert_eq!(r.summary.crew.as_ref().map(|c| c.launches), Some(1));
        let noted = |r: &Report, w: &str| r.wallets.iter().find(|x| x.address == w).unwrap().reasons.iter().any(|x| x.code == "crew");
        let made = |r: &Report, w: &str| r.wallets.iter().find(|x| x.address == w).unwrap().reasons.iter().any(|x| x.code == "crew_creator");
        assert!(!noted(&r, "dev") && made(&r, "dev") && noted(&r, "b1") && !made(&r, "b1"));

        inp.crew = Some(crew(4));
        inp.crew_wallets.remove("b1");
        let r = analyze(&inp);
        assert_eq!(r.signals.iter().find(|s| s.code == "same_crew").map(|s| s.weight), Some(20));
        assert!(made(&r, "dev") && !noted(&r, "b1"));
    }

    /// $HAMMER's shape: the launch's funder paid new wallets before launch, they bought hours later, spread out, and the
    /// funder bought too. A wallet the side paid only after its first buy isn't the launch's.
    #[test]
    fn wallets_the_launch_side_paid_for_are_insiders() {
        let t0 = input(&[], &[]).created_at;
        let trades = vec![
            trade(t0, "dev", true, 1.0),
            trade(t0 + 7_200, "fun", true, 2.0),
            trade(t0 + 17_000, "f1", true, 10.0),
            trade(t0 + 80_000, "f2", true, 15.0),
            trade(t0 + 90_000, "later", true, 1.0),
        ];
        let holders: Vec<(String, u64)> =
            [("fun", 2.0), ("f1", 10.0), ("f2", 15.0), ("later", 1.0), ("curve", 72.0)].iter().map(|(w, p)| (w.to_string(), (SUPPLY as f64 * p / 100.0) as u64)).collect();
        let paid = |by: &str, ago: i64, funded_launch: bool| LaunchFunded { by: by.into(), time: t0 - ago, sol: 0.5, funded_launch };
        let mut inp = input(&trades, &holders);
        let clean = analyze(&inp);
        inp.launch_funded = HashMap::from([
            ("f1".to_string(), paid("fun", 2_100, false)),
            ("f2".to_string(), paid("fun", 2_000, false)),
            ("fun".to_string(), paid("signer", 4_950, true)),
        ]);
        let r = analyze(&inp);
        let roles = |w: &str| r.wallets.iter().find(|x| x.address == w).map(|x| x.roles.clone()).unwrap_or_default();
        for w in ["f1", "f2", "fun"] {
            assert!(roles(w).contains(&"funded".to_string()), "{w}");
        }
        assert!(roles("later").is_empty());
        assert!((r.summary.insiders_hold_pct - 27.0).abs() < 1e-6, "{}", r.summary.insiders_hold_pct);
        assert!((r.supply.insiders - 27.0).abs() < 1e-6);
        let s = r.signals.iter().find(|s| s.code == "launch_funded").unwrap();
        assert_eq!((s.weight, s.params["wallets"], s.params["funder"], s.params["holdPct"]), (40, 2.0, 1.0, 27.0));
        assert_eq!(clean.risk, "low");
        assert!(RISK_LEVELS.iter().position(|l| *l == r.risk) >= Some(2), "{}", r.risk);
        let reason = r.wallets.iter().find(|x| x.address == "f1").unwrap().reasons.iter().find(|x| x.code == "launch_funded").unwrap();
        assert_eq!((reason.value, reason.reference.as_deref()), (Some(35.0), Some("fun")));
    }

    /// A kept mint or freeze authority is a red flag that reads at least high, whatever else the coin shows.
    #[test]
    fn a_kept_mint_or_freeze_authority_reads_at_least_high() {
        let trades = vec![trade(1000, "dev", true, 1.0)];
        let holders = vec![("curve".to_string(), SUPPLY)];
        let mut inp = input(&trades, &holders);
        assert_eq!(analyze(&inp).risk, "low");
        inp.mint_check = Some(MintCheck { can_mint: false, can_freeze: false, minted_pct: 0.0 });
        assert_eq!(analyze(&inp).risk, "low", "both given up");
        inp.mint_check = Some(MintCheck { can_mint: true, can_freeze: false, minted_pct: 9.5 });
        let r = analyze(&inp);
        assert_eq!(r.risk, "high");
        assert_eq!(r.signals.iter().find(|s| s.code == "mint_open").map(|s| s.params["mintedPct"]), Some(9.5));
        inp.mint_check = Some(MintCheck { can_mint: false, can_freeze: true, minted_pct: 0.0 });
        let r = analyze(&inp);
        assert_eq!(r.risk, "high");
        assert!(r.signals.iter().any(|s| s.code == "freeze_open") && !r.signals.iter().any(|s| s.code == "mint_open"));
    }

    #[test]
    fn finds_snipers_handoffs_and_holdings() {
        let trades = vec![
            trade(1000, "dev", true, 2.0),
            trade(1000, "b1", true, 10.0),
            trade(1004, "snipe", true, 1.0),
            trade(1100, "b1", false, 6.0),
            // A fresh wallet buys back the same amount 3 s later: hand-off.
            trade(1103, "fresh", true, 6.05),
            trade(1200, "rando", true, 0.5),
        ];
        let holders = vec![
            ("curve".to_string(), SUPPLY / 2),
            ("b1".to_string(), SUPPLY / 25),
            ("fresh".to_string(), SUPPLY * 6 / 100),
            ("snipe".to_string(), SUPPLY / 100),
            ("dev".to_string(), SUPPLY / 50),
        ];
        let r = analyze(&input(&trades, &holders));
        let role = |w: &str| {
            r.wallets
                .iter()
                .find(|x| x.address == w)
                .map(|x| x.roles.clone())
                .unwrap_or_default()
        };
        assert!(role("snipe").contains(&"sniper".to_string()));
        assert!(role("fresh").contains(&"relay".to_string()));
        assert!(role("rando").is_empty(), "late buyers aren't flagged");
        assert_eq!(r.summary.relays, 1);
        // dev 2% + b1 4% held by the insiders; b1 sold 6 of the 12% they bought.
        assert!((r.summary.insiders_hold_pct - 6.0).abs() < 1e-9);
        assert!((r.summary.bundle_sold_pct - 50.0).abs() < 1e-9);
        assert!((r.summary.curve_pool_pct - 50.0).abs() < 1e-9);
        assert_eq!(r.summary.holder_count, 4, "the curve isn't a holder");
        assert!(r.signals.iter().any(|s| s.code == "mev_relay"));
        assert!(r.signals.iter().any(|s| s.code == "front_mev_bundle"));
    }

    #[test]
    fn a_migration_claim_vault_is_neither_a_holder_nor_moved_supply() {
        // The dev buys 80% and deposits it in migrate.fun's claim vault; claimers have taken 30% out so far.
        let trades = vec![trade(1000, "dev", true, 80.0), trade(1000, "b1", true, 5.0)];
        let t = |from: &str, to: &str, pct: f64| Transfer {
            sig: format!("{from}-{to}"),
            time: 1100,
            from: from.into(),
            to: to.into(),
            raw: (SUPPLY as f64 * pct / 100.0) as u64,
        };
        let transfers = vec![t("dev", "vault", 80.0), t("vault", "claimer", 30.0)];
        let holders = vec![
            ("curve".to_string(), SUPPLY / 10),
            ("vault".to_string(), SUPPLY / 2),
            ("claimer".to_string(), SUPPLY * 3 / 10),
            ("b1".to_string(), SUPPLY / 20),
        ];
        let mut inp = input(&trades, &holders);
        inp.curve_and_pools.insert("vault".into());
        inp.claim_vaults.insert("vault".into());
        inp.transfers = &transfers;
        let r = analyze(&inp);
        let m = r.migration.expect("a migration");
        assert_eq!(m.platform, "migrate.fun");
        assert!((m.pct - 80.0).abs() < 1e-6, "vault took {}", m.pct);
        assert!((m.left_pct - 50.0).abs() < 1e-6);
        assert!(r.summary.moved_pct.abs() < 1e-9, "the deposit isn't hidden movement");
        assert!(!r.signals.iter().any(|s| s.code == "supply_moved"));
        assert!((r.supply.claim_vault - 50.0).abs() < 1e-6);
        assert!((r.supply.curve_pool - 10.0).abs() < 1e-6, "the vault isn't the curve");
        assert!(r.holders.iter().all(|h| h.owner != "vault"), "the vault isn't a holder");
    }

    #[test]
    fn a_streamflow_lock_is_not_sent_to_another_wallet() {
        // The creator buys 6.6% and locks all of it. A later balance in its own wallet is not that buy.
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(1000, "dev", true, 6.6)];
        let holders = vec![
            ("curve".to_string(), SUPPLY - raw(6.6) - raw(3.1)),
            ("escrow".to_string(), raw(6.6)),
            ("dev".to_string(), raw(3.1)),
        ];
        let mut inp = input(&trades, &holders);
        inp.stream_locks = vec![StreamLock {
            wallet: "dev".into(),
            raw: raw(6.6),
            escrow: "escrow".into(),
            time: 1100,
            contract: None,
            terms: None,
        }];
        inp.lock_escrows.insert("escrow".into());
        let r = analyze(&inp);
        assert!(r.summary.moved_pct.abs() < 1e-6, "moved {}", r.summary.moved_pct);
        assert!(r.summary.bundle_moved_pct.abs() < 1e-6, "sent {}", r.summary.bundle_moved_pct);
        assert!((r.summary.bundle_locked_pct - 100.0).abs() < 1e-6, "locked {}", r.summary.bundle_locked_pct);
        assert!((r.summary.locked_pct - 6.6).abs() < 0.01);
        assert!((r.supply.locked - 6.6).abs() < 0.01);
        assert!(r.holders.iter().all(|h| h.owner != "escrow"), "the escrow is not a holder");
        assert_eq!(r.lock_account.as_deref(), Some("escrow"));
        assert!(!r.signals.iter().any(|s| s.code == "supply_moved"));
        let exit = r.timeline.iter().find(|s| s.stage == "first_exit").unwrap();
        assert!((exit.facts["lockAfter"] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_lock_its_maker_can_cancel_counts_as_still_held_in_the_risk() {
        // The creator buys 30% and locks it. Held, the same 30% would count in full as the flagged wallets' hold.
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(1000, "dev", true, 30.0)];
        let holders = vec![("curve".to_string(), SUPPLY - raw(30.0)), ("escrow".to_string(), raw(30.0))];
        let terms = LockTerms {
            contract: "contract".into(),
            sender: "dev".into(),
            recipient: "dev".into(),
            deposited: raw(30.0),
            withdrawn: 0,
            start: 2000,
            cliff: 2000,
            cliff_amount: raw(30.0),
            period: 1,
            amount_per_period: raw(30.0),
            end: Some(2000),
            cancelable_by_sender: false,
            cancelable_by_recipient: false,
            transferable_by_sender: false,
            pausable: false,
            can_update_rate: false,
            canceled_at: None,
            closed: false,
        };
        let run = |terms: Option<LockTerms>, wallet: &str| {
            let mut inp = input(&trades, &holders);
            inp.stream_locks = vec![StreamLock {
                wallet: wallet.into(),
                raw: raw(30.0),
                escrow: "escrow".into(),
                time: 1100,
                contract: Some("contract".into()),
                terms,
            }];
            inp.lock_escrows.insert("escrow".into());
            analyze(&inp)
        };
        let held = {
            let holders = vec![("curve".to_string(), SUPPLY - raw(30.0)), ("dev".to_string(), raw(30.0))];
            analyze(&input(&trades, &holders))
        };
        let unread = run(None, "dev");
        let fixed = run(Some(terms.clone()), "dev");
        let cancelable = run(Some(LockTerms { cancelable_by_sender: true, ..terms.clone() }), "dev");
        // Terms not read, or read and fixed: as before, a lock is not a holding.
        for r in [&unread, &fixed] {
            assert!(r.summary.locked_revocable_pct.abs() < 1e-9);
            assert!(r.risk_score < held.risk_score - 20.0, "{} vs {}", r.risk_score, held.risk_score);
        }
        assert_eq!(fixed.lock_terms.as_ref(), Some(&terms));
        assert_eq!(unread.lock_terms, None);
        // Cancelable by the creator: the risk is the holding's, while the supply still shows it locked.
        assert!((cancelable.summary.locked_revocable_pct - 30.0).abs() < 0.01);
        assert!((cancelable.risk_score - held.risk_score).abs() < 1e-6, "{} vs {}", cancelable.risk_score, held.risk_score);
        assert_eq!(cancelable.risk, held.risk);
        assert!((cancelable.supply.locked - 30.0).abs() < 0.01);
        assert!(cancelable.summary.insiders_hold_pct.abs() < 1e-9, "the display keeps it in the contract");
        // Locked by a wallet nobody flagged: its own business, as its holding would be.
        let stranger = run(Some(LockTerms { cancelable_by_sender: true, ..terms }), "crowd");
        assert!(stranger.summary.locked_revocable_pct.abs() < 1e-9);
    }

    #[test]
    fn burned_supply_is_nobody_s() {
        // squirrel (AgdY…pump, 2026-10-28): a quarter of the supply burned by a buy-back bot, the curve holding 72.84%
        // of the launch's supply and the crowd 1.06%. Read as everyone else's, the burn made "Everyone else 26%".
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(9000, "a", true, 0.6), trade(9500, "b", true, 0.46)];
        let holders = vec![("curve".to_string(), raw(72.84)), ("a".to_string(), raw(0.6)), ("b".to_string(), raw(0.46))];
        let mut inp = input(&trades, &holders);
        inp.burned = SUPPLY - raw(72.84) - raw(0.6) - raw(0.46);
        let s = analyze(&inp).supply;
        let sum = s.insiders + s.other_suspicious + s.snipers + s.split + s.everyone_else + s.curve_pool + s.claim_vault + s.set_aside + s.locked + s.burned;
        assert!((sum - 100.0).abs() < 0.01, "buckets add up to {sum}");
        assert!((s.burned - 26.1).abs() < 0.01, "burned {}", s.burned);
        assert!((s.everyone_else - 1.06).abs() < 0.01, "everyone else {}", s.everyone_else);
    }

    #[test]
    fn an_insider_is_never_read_as_a_lock_escrow() {
        // The creator buys 6.6% and locks it in a real escrow. The bundle wallet buys 10% and keeps it; a Streamflow
        // payout to it (or a misread of one) named it an escrow too, and the creator as well. Each wallet's balance
        // counts once, so the buckets add up to 100%.
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(1000, "dev", true, 6.6), trade(1000, "b1", true, 10.0)];
        let holders = vec![
            ("curve".to_string(), SUPPLY - raw(6.6) - raw(10.0) - raw(2.0)),
            ("escrow".to_string(), raw(6.6)),
            ("b1".to_string(), raw(10.0)),
            ("crowd".to_string(), raw(2.0)),
        ];
        let mut inp = input(&trades, &holders);
        inp.stream_locks = vec![
            StreamLock { wallet: "dev".into(), raw: raw(6.6), escrow: "escrow".into(), time: 1100, contract: None, terms: None },
            StreamLock { wallet: "dev".into(), raw: raw(10.0), escrow: "b1".into(), time: 1200, contract: None, terms: None },
        ];
        inp.lock_escrows = ["escrow", "b1", "dev", "curve"].map(String::from).into();
        let r = analyze(&inp);
        let s = &r.supply;
        let sum = s.insiders + s.other_suspicious + s.snipers + s.split + s.everyone_else + s.curve_pool + s.claim_vault + s.set_aside + s.locked;
        assert!((sum - 100.0).abs() < 0.01, "buckets add up to {sum}");
        assert!((s.insiders - 10.0).abs() < 0.01, "insiders {}", s.insiders);
        assert!((s.locked - 6.6).abs() < 0.01, "locked {}", s.locked);
        assert!((s.everyone_else - 2.0).abs() < 0.01, "everyone else {}", s.everyone_else);
        assert!(r.holders.iter().any(|h| h.owner == "b1"), "the bundle wallet is a holder");
        assert!(r.holders.iter().all(|h| h.owner != "escrow"));
        assert_eq!(r.lock_account.as_deref(), Some("escrow"));
        // What insiders bought: 16.6%. The bundle holds 10%, the escrow 6.6%: nothing sent on, 6.6 of 16.6 locked.
        assert!(r.summary.moved_pct.abs() < 1e-6, "moved {}", r.summary.moved_pct);
        assert!((r.summary.bundle_locked_pct - 6.6 / 16.6 * 100.0).abs() < 0.01);
        let now = r.timeline.iter().find(|s| s.stage == "now").unwrap();
        assert!((now.facts["bundleLockedInPct"] - 6.6 / 16.6 * 100.0).abs() < 0.01, "only the real lock");
    }

    #[test]
    fn a_lock_that_has_come_out_keeps_what_was_put_in() {
        // The creator locked 6.6% and the contract has paid it all back out: nothing is locked now.
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(1000, "dev", true, 6.6)];
        let holders = vec![
            ("curve".to_string(), SUPPLY - raw(6.6)),
            ("escrow".to_string(), 0),
            ("dev".to_string(), raw(6.6)),
        ];
        let mut inp = input(&trades, &holders);
        inp.stream_locks = vec![StreamLock { wallet: "dev".into(), raw: raw(6.6), escrow: "escrow".into(), time: 1100, contract: None, terms: None }];
        inp.lock_escrows.insert("escrow".into());
        let r = analyze(&inp);
        assert!(r.summary.bundle_locked_pct.abs() < 1e-9);
        assert!(r.supply.locked.abs() < 1e-9);
        let now = r.timeline.iter().find(|s| s.stage == "now").unwrap();
        assert!((now.facts["bundleLockedInPct"] - 100.0).abs() < 1e-6);
        let exit = r.timeline.iter().find(|s| s.stage == "first_exit").unwrap();
        assert!((exit.facts["lockAfter"] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn finds_supply_moved_out_and_split_across_identical_wallets() {
        // The dev buys 80%, sells none, holds none: it sent its supply on, here into 40 identical 2% slices.
        let trades = vec![trade(1000, "dev", true, 80.0), trade(1000, "b1", true, 5.0)];
        let mut holders = vec![("curve".to_string(), SUPPLY / 10), ("b1".to_string(), SUPPLY / 20)];
        for i in 0..40 {
            holders.push((format!("slice{i}"), SUPPLY / 50));
        }
        // Curve 10%, b1 5%, slices 80%, a real buyer 5%: the whole supply.
        holders.push(("rando".to_string(), SUPPLY / 20));
        let r = analyze(&input(&trades, &holders));
        assert!((r.summary.moved_pct - 80.0).abs() < 1e-6, "moved {}", r.summary.moved_pct);
        assert!((r.summary.dev_moved_pct - 80.0).abs() < 1e-6);
        assert!((r.summary.bundle_moved_pct - 80.0 / 85.0 * 100.0).abs() < 1e-6);
        assert!(r.summary.bundle_sold_pct.abs() < 1e-9, "nothing was sold");
        assert_eq!(r.summary.split_wallets, 40);
        assert!((r.summary.split_each_pct - 2.0).abs() < 1e-9);
        assert!((r.supply.split - 80.0).abs() < 1e-6);
        assert!((r.supply.everyone_else - 5.0).abs() < 1e-6, "the slices aren't everyone else");
        assert!(r.holders.iter().filter(|h| h.roles.contains(&"split".to_string())).count() >= 40);
        assert!(r.signals.iter().any(|s| s.code == "supply_moved"));
        assert!(r.signals.iter().any(|s| s.code == "split_supply"));
    }

    #[test]
    fn a_few_equal_holders_are_not_a_split() {
        let trades = vec![trade(1000, "dev", true, 10.0)];
        let mut holders = vec![("dev".to_string(), SUPPLY / 10)];
        for i in 0..5 {
            holders.push((format!("same{i}"), SUPPLY / 100));
        }
        let r = analyze(&input(&trades, &holders));
        assert_eq!(r.summary.split_wallets, 0);
        assert!(r.summary.moved_pct.abs() < 1e-9);
        assert!(!r.signals.iter().any(|s| s.code == "split_supply" || s.code == "supply_moved"));
    }

    #[test]
    fn one_sell_hands_off_to_one_buyer() {
        let trades = vec![
            trade(1000, "b1", true, 10.0),
            trade(1100, "b1", false, 5.0),
            trade(1102, "fresh1", true, 5.04),
            trade(1103, "fresh2", true, 5.0),
        ];
        let r = analyze(&input(&trades, &[]));
        assert_eq!(r.summary.relays, 1);
        let relay = r
            .wallets
            .iter()
            .find(|w| w.roles.contains(&"relay".to_string()))
            .unwrap();
        assert_eq!(relay.address, "fresh2", "exact amount wins over the earlier near match");
    }

    /// A DBC config that keeps 99.9% of the supply for its owner ($ANSЕMINU's): before the owner takes it, and after it
    /// took it and sold it into the pool, partly by a swap too odd to price.
    #[test]
    fn a_dbc_curves_leftover_is_an_insiders_supply() {
        use crate::addr::DBC_POOL_AUTHORITY as VAULT;
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let set_aside = SetAside { wallet: "own".into(), raw: raw(99.896), curve_sells_pct: Some(0.054), held_by: vec![] };
        let sum = |r: &Report| {
            let s = &r.supply;
            s.insiders + s.other_suspicious + s.snipers + s.split + s.everyone_else + s.curve_pool + s.claim_vault + s.set_aside + s.locked
        };

        // Launched: the leftover still waits in the curve's vault.
        let trades = vec![trade(1005, "own", true, 0.03), trade(1100, "own", false, 0.03), trade(1006, "fan", true, 0.05)];
        let holders = vec![(VAULT.to_string(), raw(99.95)), ("fan".to_string(), raw(0.05))];
        let mut inp = input(&trades, &holders);
        inp.curve_and_pools = [VAULT.to_string(), "pool".to_string()].into();
        inp.bundled_pct = 0.0;
        inp.set_aside = Some(set_aside.clone());
        let r = analyze(&inp);
        let own = r.wallets.iter().find(|w| w.address == "own").expect("flagged");
        assert!(own.roles.contains(&"leftover".to_string()));
        assert!(own.reasons.iter().any(|x| x.code == "gets_leftover"));
        assert!((r.supply.set_aside - 99.896).abs() < 0.001, "{}", r.supply.set_aside);
        assert!((r.supply.curve_pool - 0.054).abs() < 0.001, "{}", r.supply.curve_pool);
        assert!((sum(&r) - 100.0).abs() < 0.01, "{}", sum(&r));
        let sig = r.signals.iter().find(|x| x.code == "supply_set_aside").expect("red flag");
        assert_eq!(sig.weight, 60);
        assert_eq!(sig.params.get("curve"), Some(&0.054));
        assert_eq!(r.risk, "extreme");

        // 14 minutes in: the owner takes it and sells it into the pool, the last bit by a transfer.
        let trades = vec![
            trade(1005, "own", true, 0.03),
            trade(1100, "own", false, 0.03),
            trade(1006, "fan", true, 0.05),
            trade(1850, "own", false, 99.0),
        ];
        let transfers = vec![
            Transfer { sig: "w".into(), time: 1826, from: VAULT.into(), to: "own".into(), raw: raw(99.896) },
            Transfer { sig: "s".into(), time: 1860, from: "own".into(), to: "pool".into(), raw: raw(0.896) },
        ];
        let holders = vec![("pool".to_string(), raw(99.95)), ("fan".to_string(), raw(0.05))];
        let mut inp = input(&trades, &holders);
        inp.curve_and_pools = [VAULT.to_string(), "pool".to_string()].into();
        inp.bundled_pct = 0.0;
        inp.set_aside = Some(set_aside);
        inp.transfers = &transfers;
        let r = analyze(&inp);
        let own = r.wallets.iter().find(|w| w.address == "own").expect("flagged");
        assert!((own.taken_pct - 99.896).abs() < 0.001);
        assert!(own.reasons.iter().any(|x| x.code == "took_leftover"));
        assert!(r.wallets.iter().all(|w| w.address != "pool"), "a pool isn't a person");
        assert!((r.summary.set_aside_taken_pct - 99.896).abs() < 0.001);
        assert!((r.summary.set_aside_sold_pct - 99.926).abs() < 0.001, "{}", r.summary.set_aside_sold_pct);
        assert_eq!(r.summary.set_aside_waiting_pct, 0.0);
        // What the insiders got adds up: all of it sold, none sent on, none held.
        assert!(r.summary.bundle_sold_pct > 99.9 && r.summary.bundle_moved_pct == 0.0 && r.summary.insiders_hold_pct == 0.0);
        assert_eq!(r.supply.other_suspicious, 0.0);
        assert!((sum(&r) - 100.0).abs() < 0.01, "{}", sum(&r));
        assert!(r.signals.iter().any(|x| x.code == "supply_set_aside" && x.params["taken"] > 99.0));
        assert!(!own.reasons.iter().any(|x| x.code == "gets_leftover"), "taken: no longer to come");
        assert_eq!(r.risk, "extreme", "still rigged once sold");
    }

    /// A coin that launched in a pool, whose creator kept 40% of the supply outside it when trading began: an insider's
    /// supply got without buying, whose sales and hand-ons show as a leftover taken out of a curve does.
    #[test]
    fn supply_kept_outside_a_pool_is_the_creators() {
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let kept = SetAside { wallet: "dev".into(), raw: raw(40.0), curve_sells_pct: None, held_by: vec![("dev".into(), raw(40.0))] };
        // A fan buys 5% from the pool; the creator sells 10% into it and sends 5% to a friend.
        let trades = vec![trade(1005, "fan", true, 5.0), trade(1300, "dev", false, 10.0)];
        let transfers = vec![Transfer { sig: "t".into(), time: 1400, from: "dev".into(), to: "friend".into(), raw: raw(5.0) }];
        let holders = vec![
            ("pool".to_string(), raw(65.0)),
            ("dev".to_string(), raw(25.0)),
            ("friend".to_string(), raw(5.0)),
            ("fan".to_string(), raw(5.0)),
        ];
        let mut inp = input(&trades, &holders);
        inp.curve_and_pools = ["pool".to_string()].into();
        inp.roles = [("dev".to_string(), vec!["creator".to_string()])].into();
        inp.bundled_pct = 0.0;
        inp.transfers = &transfers;
        inp.set_aside = Some(kept);
        inp.opening_bundle = true;
        let r = analyze(&inp);
        let s = &r.supply;
        let sum = s.insiders + s.other_suspicious + s.snipers + s.split + s.everyone_else + s.curve_pool + s.claim_vault + s.set_aside + s.locked;
        assert!((sum - 100.0).abs() < 0.01, "{sum}");
        let dev = r.wallets.iter().find(|w| w.address == "dev").unwrap();
        assert!(dev.roles.contains(&"kept".to_string()) && dev.reasons.iter().any(|x| x.code == "kept_outside_pool"));
        assert!(!dev.reasons.iter().any(|x| x.code == "gets_leftover" || x.code == "took_leftover"));
        let m = &r.summary;
        assert!((m.set_aside_pct - 40.0).abs() < 0.001 && (m.set_aside_taken_pct - 40.0).abs() < 0.001);
        assert!((m.set_aside_sold_pct - 10.0).abs() < 0.001, "{}", m.set_aside_sold_pct);
        assert_eq!(m.set_aside_waiting_pct, 0.0);
        // Held 25 + sold 10 + sent on 5 = the 40 kept.
        assert!((m.insiders_hold_pct - 25.0).abs() < 0.001 && (m.moved_pct - 5.0).abs() < 0.001, "{} {}", m.insiders_hold_pct, m.moved_pct);
        assert!((m.bundle_sold_pct - 25.0).abs() < 0.001 && (m.bundle_moved_pct - 12.5).abs() < 0.001);
        assert!(r.wallets.iter().any(|w| w.address == "friend" && w.roles.contains(&"transfer".to_string())));
        let sig = r.signals.iter().find(|x| x.code == "supply_set_aside").expect("red flag");
        assert_eq!((sig.weight, sig.params["pool"], sig.params["wallets"], sig.params["creator"]), (30, 1.0, 1.0, 1.0));
        assert_ne!(r.risk, "low");
    }

    /// Liquidity taken out of a pool after trading began: supply got without buying it, kept from buyers as what was held
    /// outside the pool at the start is, and told as taken out of the pool.
    #[test]
    fn supply_taken_out_of_a_pool_is_kept_from_buyers() {
        let raw = |p: f64| (SUPPLY as f64 * p / 100.0) as u64;
        let trades = vec![trade(1005, "fan", true, 5.0)];
        let transfers = vec![Transfer { sig: "l".into(), time: 1500, from: "pool".into(), to: "dev".into(), raw: raw(12.0) }];
        let holders = vec![("pool".to_string(), raw(83.0)), ("dev".to_string(), raw(12.0)), ("fan".to_string(), raw(5.0))];
        let mut inp = input(&trades, &holders);
        inp.curve_and_pools = ["pool".to_string()].into();
        inp.roles = [("dev".to_string(), vec!["creator".to_string()])].into();
        inp.bundled_pct = 0.0;
        inp.transfers = &transfers;
        inp.opening_bundle = true;
        inp.liquidity_pulled = Some(LiquidityPulled { raw: raw(12.0), quote: 3.2, quote_is_usd: false, by_creator: true });
        let r = analyze(&inp);
        let pulled = r.signals.iter().find(|x| x.code == "liquidity_pulled").expect("red flag");
        assert_eq!((pulled.weight, pulled.params["quote"], pulled.params["creator"]), (50, 3.2, 1.0));
        assert!((pulled.params["pct"] - 12.0).abs() < 0.001);
        let dev = r.wallets.iter().find(|w| w.address == "dev").unwrap();
        assert!(dev.roles.contains(&"kept".to_string()) && dev.reasons.iter().any(|x| x.code == "took_from_pool"));
        // Kept outside the pool and taken out of it are told apart: no kept-supply flag here.
        assert!(r.signals.iter().all(|x| x.code != "supply_set_aside"));
        assert!(r.risk_score >= 12.0, "what it took counts in the risk");
        // What the creator got without buying is what it holds.
        assert!((r.summary.insiders_hold_pct - 12.0).abs() < 0.001 && r.summary.moved_pct == 0.0);
    }

    #[test]
    fn transfers_link_receivers_layer_by_layer_and_spot_dust() {
        let t = |time: i64, from: &str, to: &str, pct_of: f64| Transfer {
            sig: format!("{from}-{to}"),
            time,
            from: from.into(),
            to: to.into(),
            raw: (SUPPLY as f64 * pct_of / 100.0) as u64,
        };
        let mut transfers = vec![t(1100, "b1", "x", 5.0), t(1200, "x", "y", 5.0)];
        transfers.extend((0..25).map(|i| t(1300 + i, "dev", &format!("dust{i}"), 0.00001)));
        let mut inp = input(&[], &[]);
        inp.transfers = &transfers;
        let r = analyze(&inp);
        let roles = |w: &str| {
            r.wallets
                .iter()
                .find(|x| x.address == w)
                .map(|x| x.roles.clone())
                .unwrap_or_default()
        };
        assert!(roles("x").contains(&"transfer".to_string()));
        assert!(roles("y").contains(&"transfer".to_string()), "second layer");
        assert_eq!(r.summary.transfer_layers, 2);
        assert_eq!(r.summary.dust_recipients, 25);
        assert!(r.signals.iter().any(|s| s.code == "temp_holder_layer"));
        assert!(r.signals.iter().any(|s| s.code == "mass_distribution"));
        assert!(roles("dust0").is_empty(), "dust receivers aren't flagged one by one");
    }

    #[test]
    fn a_presales_payouts_are_claims_not_insider_transfers() {
        // $WOJAK, 2026-10-07: a presale's contract bought 89% at launch, and 327 contributors claimed their shares
        // from it, each read as an insider's transfer.
        let transfers: Vec<Transfer> = (0..3)
            .map(|i| Transfer {
                sig: format!("claim{i}"),
                time: 1100 + i,
                from: "presale".into(),
                to: format!("contributor{i}"),
                raw: SUPPLY / 50,
            })
            .collect();
        // What it hasn't paid out yet is still theirs.
        let holders = vec![("presale".to_string(), SUPPLY / 8)];
        let mut inp = input(&[], &holders);
        inp.roles.insert("presale".into(), vec!["presale".into()]);
        inp.transfers = &transfers;
        let r = analyze(&inp);
        assert!(r.wallets.iter().all(|w| !w.roles.contains(&"transfer".to_string())));
        assert_eq!(r.summary.flagged_hold_pct, 0.0);
    }

    #[test]
    fn a_sell_split_across_fresh_wallets_is_a_handoff() {
        let trades = vec![
            trade(1000, "b1", true, 10.0),
            trade(1100, "b1", false, 6.0),
            trade(1102, "f1", true, 2.5),
            trade(1103, "f2", true, 3.5),
        ];
        let r = analyze(&input(&trades, &[]));
        assert_eq!(r.summary.relays, 1);
        for w in ["f1", "f2"] {
            let wallet = r.wallets.iter().find(|x| x.address == w).unwrap();
            assert!(wallet.reasons.iter().any(|x| x.code == "relay_split"), "{w}");
        }
    }

    /// Three snipers funded by one wallet, buying 30% within seconds and all sold: counted like a bundle, so the launch
    /// isn't low risk once they're gone. The same three, funded apart, are just snipers.
    fn snipers(funded_together: bool) -> Report {
        let trades = vec![
            trade(1000, "dev", true, 1.0),
            trade(1002, "s1", true, 10.0),
            trade(1003, "s2", true, 10.0),
            trade(1004, "s3", true, 10.0),
            trade(1100, "s1", false, 10.0),
            trade(1101, "s2", false, 10.0),
            trade(1102, "s3", false, 10.0),
        ];
        let holders = vec![("curve".to_string(), SUPPLY / 2), ("dev".to_string(), SUPPLY / 100)];
        let mut inp = input(&trades, &holders);
        inp.bundled_pct = 0.0;
        inp.roles = [("dev".to_string(), vec!["creator".to_string()])].into();
        let funding = |w: &str| Funding {
            funder: if funded_together { "boss".into() } else { format!("funder-{w}") },
            funder_label: None,
            amount: 1.0,
            time: 900,
            seconds_before_launch: 100,
            root: None,
            root_label: None,
            hops: 0,
            idle_days: None,
        };
        inp.funding = ["s1", "s2", "s3"].iter().map(|w| (w.to_string(), funding(w))).collect();
        analyze(&inp)
    }

    #[test]
    fn linked_snipers_weigh_like_a_bundle() {
        let r = snipers(true);
        let sig = r.signals.iter().find(|s| s.code == "coordinated_snipers").expect("coordinated_snipers");
        assert_eq!(sig.params["wallets"], 3.0);
        assert!((sig.params["pct"] - 30.0).abs() < 0.01, "{:?}", sig.params);
        assert_eq!(sig.weight, 40);
        assert_ne!(r.risk, "low", "score {}", r.risk_score);
        let lone = snipers(false);
        assert!(!lone.signals.iter().any(|s| s.code == "coordinated_snipers"));
        assert!(lone.risk_score < r.risk_score);
    }

    #[test]
    fn a_creator_bundling_all_week_weighs_more() {
        let trades = vec![trade(1000, "dev", true, 1.0)];
        let holders = vec![("curve".to_string(), SUPPLY / 2)];
        let mut inp = input(&trades, &holders);
        inp.dev_launches_7d = 189;
        inp.dev_bundled_7d = 121;
        let heavy = analyze(&inp).signals.into_iter().find(|s| s.code == "serial_launcher").unwrap();
        inp.dev_bundled_7d = 0;
        let light = analyze(&inp).signals.into_iter().find(|s| s.code == "serial_launcher").unwrap();
        assert_eq!((heavy.weight, light.weight), (30, 10));
    }

    #[test]
    fn handoffs_a_busy_market_gives_by_chance_are_dropped() {
        // A fresh wallet buys 0.1% every second, and the bundle sells 0.1% every 10 seconds: each sell has a look-alike
        // buyback, and so does the same sell minutes earlier or later.
        let mut trades = vec![trade(1000, "b1", true, 50.0)];
        for t in 1100..1700 {
            trades.push(trade(t, &format!("c{t}"), true, 0.1));
            if (1300..1500).contains(&t) && t % 10 == 0 {
                trades.push(trade(t, "b1", false, 0.1));
            }
        }
        trades.sort_by_key(|t| t.time);
        let r = analyze(&input(&trades, &[]));
        assert_eq!(r.summary.relays, 0);
        assert_eq!(r.summary.possible_relays, 0);
        assert_eq!(r.summary.relays_by_chance, 20);
        assert!(!r.wallets.iter().any(|w| w.roles.iter().any(|x| x == "relay" || x == "suspect")));
        assert!(!r.signals.iter().any(|s| s.code == "mev_relay"));
    }

    fn funded(funder: &str, root: Option<&str>, time: i64, hops: u32) -> Funding {
        Funding {
            funder: funder.into(),
            funder_label: None,
            amount: 1.0,
            time,
            seconds_before_launch: 1000 - time,
            root: root.map(str::to_string),
            root_label: None,
            hops,
            idle_days: None,
        }
    }

    #[test]
    fn new_wallets_bought_with_sol_from_traders_of_the_coin_are_recycled() {
        let trades = vec![
            trade(1000, "b1", true, 10.0),
            trade(1050, "t1", true, 1.0),
            trade(1100, "b1", false, 10.0),
            trade(1150, "t1", false, 1.0),
            trade(1200, "n1", true, 3.0),
            trade(1201, "n2", true, 1.0),
            trade(1202, "n3", true, 1.0),
            trade(1203, "n4", true, 1.0),
            trade(1204, "n5", true, 1.0),
        ];
        let holders = vec![("n1".to_string(), SUPPLY * 3 / 100), ("n2".to_string(), SUPPLY / 100)];
        let mut inp = input(&trades, &holders);
        inp.funding = HashMap::from([
            // From the bundle, through 3 throwaway wallets.
            ("n1".to_string(), funded("hop", Some("b1"), 1190, 3)),
            // Straight from a wallet that had traded the coin.
            ("n2".to_string(), funded("t1", None, 1195, 0)),
            // From a wallet that never traded it.
            ("n3".to_string(), funded("hop", Some("elsewhere"), 1196, 3)),
            // Before launch: nobody had traded it yet.
            ("n4".to_string(), funded("t1", None, 900, 0)),
            ("n5".to_string(), funded("t1", None, 1120, 0)),
        ]);
        let r = analyze(&inp);
        let role = |w: &str| r.wallets.iter().find(|x| x.address == w).map(|x| x.roles.clone()).unwrap_or_default();
        for w in ["n1", "n2", "n5"] {
            assert!(role(w).contains(&"recycled".to_string()), "{w}");
        }
        for w in ["n3", "n4"] {
            assert!(!role(w).contains(&"recycled".to_string()), "{w}");
        }
        let n1 = r.wallets.iter().find(|x| x.address == "n1").unwrap();
        assert!(n1.reasons.iter().any(|x| x.code == "insider_recycled" && x.reference.as_deref() == Some("b1")));
        assert!(!n1.reasons.iter().any(|x| x.code == "throwaway_funded"));
        assert_eq!(r.summary.recycled_wallets, 3);
        assert_eq!(r.summary.recycled_from_insiders, 1);
        assert_eq!(r.summary.recycled_traced, 4);
        assert!((r.summary.recycled_hold_pct - 4.0).abs() < 1e-9);
        // Holding under 5%, one of them from an insider.
        let s = r.signals.iter().find(|s| s.code == "recycled_funding").unwrap();
        assert_eq!(s.weight, 20);
        // Held by recycled wallets counts as flagged.
        assert!(r.summary.flagged_hold_pct >= 4.0);
    }

    #[test]
    fn a_crowd_of_wallets_made_after_launch_is_a_red_flag() {
        let mut trades = vec![];
        let mut first_seen = HashMap::new();
        for i in 0..60 {
            let w = format!("w{i}");
            trades.push(trade(1100 + i, &w, true, 0.5));
            // 40 made after launch, 20 long before.
            first_seen.insert(w, if i < 40 { 1050 } else { 100 });
        }
        let mut inp = input(&trades, &[]);
        inp.first_seen = first_seen.clone();
        let r = analyze(&inp);
        let n = r.summary.new_wallets.unwrap();
        assert_eq!((n.checked, n.new), (60, 40));
        assert!((n.volume_pct - 200.0 / 3.0).abs() < 1e-9);
        assert!((n.checked_volume_pct - 100.0).abs() < 1e-9);
        assert!((n.new_net_sol - 40.0).abs() < 1e-9 && (n.old_net_sol - 20.0).abs() < 1e-9);
        assert!(r.signals.iter().any(|s| s.code == "new_wallet_crowd"));
        // Too few checked to tell.
        inp.first_seen = first_seen.into_iter().filter(|(w, _)| w.len() == 2).collect();
        assert!(!analyze(&inp).signals.iter().any(|s| s.code == "new_wallet_crowd"));
    }

    #[test]
    fn risk_matches_the_original_samples() {
        let proxima = [signal("proxima_router", 100, &[])];
        assert_eq!(risk(52.0, 2.5, &proxima).0, "high");
        let sumo = [signal("front_mev_bundle", 15, &[]), signal("funded_together", 25, &[])];
        assert_eq!(risk(90.0, 45.0, &sumo).0, "extreme");
        assert_eq!(risk(0.0, 0.5, &[]).0, "low");
    }
}

/// Other wallets that bought in the same bundled transaction as `wallet`.
fn block0_cotraders(block0: &[Block0Tx], wallet: &str) -> Option<usize> {
    block0
        .iter()
        .filter(|t| t.kind == "bundle" || t.kind == "coordinated" || t.kind == "create")
        .find(|t| t.wallets.len() >= 2 && t.wallets.iter().any(|w| w == wallet))
        .map(|t| t.wallets.len() - 1)
}

/// Wallets whose buys landed back-to-back right after the creator's bundle,
/// outside it: next in the block with no unrelated transaction in between.
fn contiguous_after_create(block0: &[Block0Tx]) -> HashSet<&str> {
    let mut out = HashSet::new();
    let mut txs: Vec<&Block0Tx> = block0.iter().filter(|t| t.idx.is_some()).collect();
    txs.sort_by_key(|t| t.idx);
    let Some(last_own) = txs
        .iter()
        .filter(|t| t.kind == "create" || t.kind == "bundle")
        .filter_map(|t| t.idx)
        .max()
    else {
        return out;
    };
    let mut next = last_own + 1;
    for t in txs.iter().filter(|t| t.idx > Some(last_own)) {
        if t.idx != Some(next) {
            break;
        }
        out.extend(t.wallets.iter().map(String::as_str));
        next += 1;
    }
    out
}
