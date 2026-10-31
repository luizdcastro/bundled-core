//! Known Solana addresses: exchange hot wallets, bridges and services.
//! Seeded from publicly known exchange and service addresses, and
//! added to at run time from Helius's identity data (`learn`), which the
//! server looks up for the wallets in a report and keeps in its database.

pub struct Label {
    pub address: &'static str,
    pub name: &'static str,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Exchange,
    Bridge,
    Service,
    Spam,
    /// A pool that pays out what many people deposited, so the deposit behind a payout can't be told: who it pays isn't
    /// linked by it.
    Privacy,
}

pub const LABELS: &[Label] = &[
    Label {
        address: "5tzFkiKscXHK5ZXCGbXZxdw7gTjjD1mBwuoFbhUvuAi9",
        name: "Binance Hot Wallet 2",
        kind: Kind::Exchange,
    },
    Label {
        address: "2ojv9BAiHUrvsm9gxDe7fJSzbNZSJcxZvf8dqmWGHG8S",
        name: "Binance Hot Wallet 1",
        kind: Kind::Exchange,
    },
    Label {
        address: "FpwQQhQQoEaVu3WU2qZMfF1hx48YyfwsLoRgXG83E99Q",
        name: "Coinbase Hot Wallet 3",
        kind: Kind::Exchange,
    },
    Label {
        address: "2AQdpHJ2JpcEgPiATUXjQxA8QmafFegfQwSLWSprPicm",
        name: "Coinbase Hot Wallet 10",
        kind: Kind::Exchange,
    },
    Label {
        address: "GJRs4FwHtemZ5ZE9x3FNvJ8TMwitKTh21yxdRPqn7npE",
        name: "Coinbase Hot Wallet 11",
        kind: Kind::Exchange,
    },
    Label {
        address: "FWznbcNXWQuHTawe9RxvQ2LdCENssh12dsznf4RiouN5",
        name: "Kraken Hot Wallet",
        kind: Kind::Exchange,
    },
    Label {
        address: "AC5RDfQFmDS1deWZos921JfqscXdByf8BKHs5ACWjtW2",
        name: "Bybit Hot Wallet 1",
        kind: Kind::Exchange,
    },
    Label {
        address: "iGdFcQoyR2MwbXMHQskhmNsqddZ6rinsipHc4TNSdwu",
        name: "Bybit Hot Wallet 10",
        kind: Kind::Exchange,
    },
    Label {
        address: "BmFdpraQhkiDQE6SnfG5omcA1VwzqfXrwtNYBwWTymy6",
        name: "KuCoin Hot Wallet 2",
        kind: Kind::Exchange,
    },
    Label {
        address: "u6PJ8DtQuPFnfmwHbGFULQ4u4EgjDiyYKjVEsynXq2w",
        name: "Gate Hot Wallet",
        kind: Kind::Exchange,
    },
    Label {
        address: "A77HErqtfN1hLLpvZ9pCtu66FEtM8BveoaKbbMoZ4RiR",
        name: "Bitget Hot Wallet",
        kind: Kind::Exchange,
    },
    Label {
        address: "8mowmVCEewZ9W2cEaQyQeQEeSxhGr1hvRviLwozwNtBt",
        name: "WhiteBIT Hot Wallet",
        kind: Kind::Exchange,
    },
    Label {
        address: "B48kNVXs4YK4amkBCH2XokQiv1SeiVQGHDR17xDeKAAn",
        name: "ChangeNOW Hot Wallet 5",
        kind: Kind::Exchange,
    },
    Label {
        address: "5ndLnEYqSFiA5yUFHo6LVZ1eWc6Rhh11K5CfJNkoHEPs",
        name: "FixedFloat Exchange",
        kind: Kind::Exchange,
    },
    Label {
        address: "BrGihgGjCmpu2p96rou6GQj3sQpwgeud937jf4RvZP9G",
        name: "Bridgers",
        kind: Kind::Bridge,
    },
    Label {
        address: "F7p3dFrjRTbtRp8FRF6qHLomXbKRBzpvBLjtQcfcgmNe",
        name: "Relay Solver",
        kind: Kind::Bridge,
    },
    Label {
        address: "AgmLJBMDCqWynYnQiPCuj9ewsNNsBJXyzoUhD9LJzN51",
        name: "FOMO Co-signer",
        kind: Kind::Service,
    },
    Label {
        address: "FLipG5QHjZe1H12f6rr5LCnrmqjhwuBTBp78GwzxnwkR",
        name: "Spam Dusting Account (FLipG5)",
        kind: Kind::Spam,
    },
    // Privacy Cash's SOL pool (program 9fhQ…QyQD): each withdrawal is signed by a relayer and paid out of it.
    Label {
        address: "4AV2Qzp3N4c9RfzyEbNZs2wqWfW4EwKnnxFAZCndvfGh",
        name: "Privacy Cash (privacy pool)",
        kind: Kind::Privacy,
    },
];

/// What we know an address is: a seeded label, or one learned at run time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub name: String,
    pub kind: Kind,
}

static LEARNED: std::sync::LazyLock<std::sync::RwLock<std::collections::HashMap<String, Known>>> =
    std::sync::LazyLock::new(Default::default);

/// Adds a label found at run time (an exchange, bridge or service; people are
/// never labels, since a person funding wallets is a link worth seeing).
pub fn learn(address: &str, name: &str, kind: Kind) {
    if let Ok(mut l) = LEARNED.write() {
        l.insert(address.to_string(), Known { name: name.to_string(), kind });
    }
}

pub fn label(address: &str) -> Option<Known> {
    if let Some(l) = LABELS.iter().find(|l| l.address == address) {
        return Some(Known { name: l.name.to_string(), kind: l.kind });
    }
    LEARNED.read().ok()?.get(address).cloned()
}

/// An exchange or a privacy pool: it pays out for many people, so two wallets it funded aren't linked by that.
pub fn funds_everyone(address: &str) -> bool {
    label(address).is_some_and(|l| matches!(l.kind, Kind::Exchange | Kind::Privacy))
}

/// "Binance Hot Wallet 2" → "Binance", to group funding by exchange.
pub fn exchange(address: &str) -> Option<String> {
    label(address)
        .filter(|l| l.kind == Kind::Exchange)
        .map(|l| l.name.split_whitespace().next().unwrap_or(&l.name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_and_learned_labels() {
        assert_eq!(exchange("5tzFkiKscXHK5ZXCGbXZxdw7gTjjD1mBwuoFbhUvuAi9").as_deref(), Some("Binance"));
        assert_eq!(exchange("learned-kraken"), None);
        learn("learned-kraken", "Kraken Hot Wallet 1", Kind::Exchange);
        assert_eq!(exchange("learned-kraken").as_deref(), Some("Kraken"));
        learn("learned-app", "Axiom Rewards", Kind::Service);
        assert!(exchange("learned-app").is_none() && label("learned-app").is_some());
    }
}
