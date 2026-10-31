//! The subset of a Solana transaction we read. Accepts both shapes we receive:
//! `getTransaction` with `encoding: "json"` (string account keys plus a header)
//! and Helius `transactionSubscribe` with `encoding: "jsonParsed"` (account keys
//! as objects that carry a `signer` flag and already include lookup-table keys).

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx {
    #[serde(default)]
    pub slot: Option<u64>,
    #[serde(default)]
    pub block_time: Option<i64>,
    /// Position within its block, when the source provides it.
    #[serde(default)]
    pub transaction_index: Option<u64>,
    pub transaction: TxBody,
    pub meta: Option<Meta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TxBody {
    pub signatures: Vec<String>,
    pub message: Message,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub account_keys: Vec<AccountKey>,
    #[serde(default)]
    pub header: Option<Header>,
    #[serde(default)]
    pub instructions: Vec<Value>,
    /// Address lookup tables the transaction loads accounts from (version 0).
    #[serde(default)]
    pub address_table_lookups: Vec<TableLookup>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableLookup {
    pub account_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AccountKey {
    Plain(String),
    Parsed {
        pubkey: String,
        #[serde(default)]
        signer: bool,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub num_required_signatures: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    #[serde(default)]
    pub err: Option<Value>,
    #[serde(default)]
    pub fee: u64,
    #[serde(default)]
    pub pre_balances: Vec<u64>,
    #[serde(default)]
    pub post_balances: Vec<u64>,
    #[serde(default)]
    pub pre_token_balances: Vec<TokenBalance>,
    #[serde(default)]
    pub post_token_balances: Vec<TokenBalance>,
    #[serde(default)]
    pub log_messages: Option<Vec<String>>,
    #[serde(default)]
    pub inner_instructions: Option<Vec<InnerInstructions>>,
    #[serde(default)]
    pub loaded_addresses: Option<LoadedAddresses>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenBalance {
    pub account_index: usize,
    pub mint: String,
    #[serde(default)]
    pub owner: Option<String>,
    pub ui_token_amount: UiTokenAmount,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UiTokenAmount {
    pub amount: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InnerInstructions {
    pub instructions: Vec<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LoadedAddresses {
    #[serde(default)]
    pub writable: Vec<String>,
    #[serde(default)]
    pub readonly: Vec<String>,
}

impl Tx {
    /// The address lookup tables this transaction uses.
    pub fn lookup_tables(&self) -> impl Iterator<Item = &str> {
        self.transaction.message.address_table_lookups.iter().map(|l| l.account_key.as_str())
    }

    pub fn signature(&self) -> &str {
        self.transaction.signatures.first().map(String::as_str).unwrap_or("")
    }

    pub fn succeeded(&self) -> bool {
        match &self.meta {
            Some(m) => matches!(m.err, None | Some(Value::Null)),
            None => false,
        }
    }

    /// All account keys in index order, lookup-table keys included.
    pub fn account_keys(&self) -> Vec<&str> {
        let msg = &self.transaction.message;
        let mut keys: Vec<&str> = msg
            .account_keys
            .iter()
            .map(|k| match k {
                AccountKey::Plain(s) => s.as_str(),
                AccountKey::Parsed { pubkey, .. } => pubkey.as_str(),
            })
            .collect();
        let parsed = matches!(msg.account_keys.first(), Some(AccountKey::Parsed { .. }));
        if !parsed {
            if let Some(loaded) = self.meta.as_ref().and_then(|m| m.loaded_addresses.as_ref()) {
                keys.extend(loaded.writable.iter().map(String::as_str));
                keys.extend(loaded.readonly.iter().map(String::as_str));
            }
        }
        keys
    }

    pub fn signers(&self) -> Vec<&str> {
        let msg = &self.transaction.message;
        if let Some(h) = &msg.header {
            return self
                .account_keys()
                .into_iter()
                .take(h.num_required_signatures)
                .collect();
        }
        msg.account_keys
            .iter()
            .filter_map(|k| match k {
                AccountKey::Parsed { pubkey, signer: true } => Some(pubkey.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn mentions(&self, address: &str) -> bool {
        self.account_keys().iter().any(|k| *k == address)
    }

    /// Net change in raw `mint` balance per owner. Positive means the owner received tokens.
    pub fn token_deltas(&self, mint: &str) -> BTreeMap<String, i128> {
        let mut out: BTreeMap<String, i128> = BTreeMap::new();
        let Some(meta) = &self.meta else { return out };
        let amount = |b: &TokenBalance| b.ui_token_amount.amount.parse::<i128>().unwrap_or(0);
        for post in meta.post_token_balances.iter().filter(|b| b.mint == mint) {
            let Some(owner) = &post.owner else { continue };
            let pre = meta
                .pre_token_balances
                .iter()
                .find(|b| b.account_index == post.account_index)
                .map(amount)
                .unwrap_or(0);
            *out.entry(owner.clone()).or_default() += amount(post) - pre;
        }
        // Accounts that were closed in this tx only appear in the pre balances.
        for pre in meta.pre_token_balances.iter().filter(|b| b.mint == mint) {
            let closed = !meta
                .post_token_balances
                .iter()
                .any(|b| b.account_index == pre.account_index);
            if let (true, Some(owner)) = (closed, &pre.owner) {
                *out.entry(owner.clone()).or_default() -= amount(pre);
            }
        }
        out
    }

    /// Distinct token mints whose balances this tx touched, wrapped SOL excluded.
    pub fn mints(&self) -> Vec<&str> {
        let Some(meta) = &self.meta else { return vec![] };
        let mut out: Vec<&str> = meta
            .pre_token_balances
            .iter()
            .chain(meta.post_token_balances.iter())
            .map(|b| b.mint.as_str())
            .filter(|m| *m != crate::addr::WRAPPED_SOL)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Net SOL change per account, in lamports, in account-key order.
    pub fn sol_deltas(&self) -> Vec<(&str, i64)> {
        let Some(m) = &self.meta else { return vec![] };
        self.account_keys()
            .into_iter()
            .zip(m.pre_balances.iter().zip(m.post_balances.iter()))
            .map(|(k, (pre, post))| (k, *post as i64 - *pre as i64))
            .collect()
    }

    /// The token account `owner` holds `mint` in after this transaction, if it holds any: where its buy landed.
    pub fn token_account(&self, owner: &str, mint: &str) -> Option<&str> {
        let held = self.meta.as_ref()?.post_token_balances.iter().find(|b| {
            b.mint == mint
                && b.owner.as_deref() == Some(owner)
                && !b.ui_token_amount.amount.trim_start_matches('0').is_empty()
        })?;
        self.account_keys().get(held.account_index).copied()
    }

    /// The mint of token account `account`, when this transaction's token balances list it.
    pub fn mint_of(&self, account: &str) -> Option<&str> {
        let m = self.meta.as_ref()?;
        let keys = self.account_keys();
        m.pre_token_balances
            .iter()
            .chain(m.post_token_balances.iter())
            .find(|b| keys.get(b.account_index) == Some(&account))
            .map(|b| b.mint.as_str())
    }

    /// Net change in each token `owner` holds, by mint, wrapped SOL left out.
    pub fn owner_token_deltas(&self, owner: &str) -> BTreeMap<String, i128> {
        let mut out = BTreeMap::new();
        for mint in self.mints() {
            if let Some(&d) = self.token_deltas(mint).get(owner).filter(|d| **d != 0) {
                out.insert(mint.to_string(), d);
            }
        }
        out
    }

    /// The mints `owner` holds token accounts for in this transaction.
    pub fn mints_held_by(&self, owner: &str) -> Vec<&str> {
        let Some(m) = &self.meta else { return vec![] };
        let mut out: Vec<&str> = m
            .pre_token_balances
            .iter()
            .chain(m.post_token_balances.iter())
            .filter(|b| b.owner.as_deref() == Some(owner))
            .map(|b| b.mint.as_str())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether `owner` holds a token account for `mint` in this transaction.
    pub fn owns_token_account(&self, owner: &str, mint: &str) -> bool {
        self.meta.as_ref().is_some_and(|m| {
            m.pre_token_balances
                .iter()
                .chain(m.post_token_balances.iter())
                .any(|b| b.mint == mint && b.owner.as_deref() == Some(owner))
        })
    }

    /// Inner instructions invoked by `program`, as raw instruction data.
    /// Handles both the `json` shape (programIdIndex) and `jsonParsed` (programId).
    pub fn inner_instruction_data(&self, program: &str) -> Vec<Vec<u8>> {
        let Some(groups) = self.meta.as_ref().and_then(|m| m.inner_instructions.as_ref()) else {
            return vec![];
        };
        let keys = self.account_keys();
        let mut out = vec![];
        for ix in groups.iter().flat_map(|g| g.instructions.iter()) {
            let pid = match (ix.get("programId"), ix.get("programIdIndex")) {
                (Some(Value::String(p)), _) => Some(p.as_str()),
                (_, Some(Value::Number(n))) => n.as_u64().and_then(|i| keys.get(i as usize).copied()),
                _ => None,
            };
            if pid != Some(program) {
                continue;
            }
            if let Some(Value::String(data)) = ix.get("data") {
                if let Some(bytes) = crate::b58::decode(data) {
                    out.push(bytes);
                }
            }
        }
        out
    }

    pub fn logs(&self) -> &[String] {
        self.meta
            .as_ref()
            .and_then(|m| m.log_messages.as_deref())
            .unwrap_or(&[])
    }
}

impl Tx {
    /// Every instruction for `program`, top-level and inner, as (data, accounts).
    /// Handles both the `json` shape (indices) and `jsonParsed` (addresses).
    pub fn program_instructions(&self, program: &str) -> Vec<(Vec<u8>, Vec<String>)> {
        let keys = self.account_keys();
        let inner = self
            .meta
            .as_ref()
            .and_then(|m| m.inner_instructions.as_ref())
            .map(|g| g.iter().flat_map(|g| g.instructions.iter()).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut out = vec![];
        for ix in self.transaction.message.instructions.iter().chain(inner) {
            let pid = match (ix.get("programId"), ix.get("programIdIndex")) {
                (Some(Value::String(p)), _) => Some(p.as_str()),
                (_, Some(Value::Number(n))) => n.as_u64().and_then(|i| keys.get(i as usize).copied()),
                _ => None,
            };
            if pid != Some(program) {
                continue;
            }
            let Some(Value::String(data)) = ix.get("data") else {
                continue;
            };
            let Some(bytes) = crate::b58::decode(data) else {
                continue;
            };
            let accounts = ix
                .get("accounts")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            Value::Number(n) => n.as_u64().and_then(|i| keys.get(i as usize)).map(|s| s.to_string()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.push((bytes, accounts));
        }
        out
    }
}
