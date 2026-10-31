//! Program and account addresses we match on.

/// pump.fun bonding-curve program.
pub const PUMP_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// pump.fun mint authority PDA. Only create instructions (`create`, `create_v2`)
/// touch it, so subscribing to it gives a creates-only stream.
pub const PUMP_MINT_AUTHORITY: &str = "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM";

/// PumpSwap AMM, where tokens trade after migrating off the curve.
pub const PUMPSWAP_PROGRAM: &str = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA";

/// Meteora Dynamic Bonding Curve: another launchpad, where Proxima is also used.
pub const DBC_PROGRAM: &str = "dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN";

/// Owns every DBC pool's token vault, so it holds supply for the market.
pub const DBC_POOL_AUTHORITY: &str = "FhVo3mqL8PW5pH5U2CN4XE33DokiyZnUwuGpH2hmHLuM";

/// Meteora DAMM v2, where DBC coins trade once they migrate.
pub const DAMM_V2_PROGRAM: &str = "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG";

/// Owns every Meteora DAMM v2 pool's vaults, so it holds supply for the market.
pub const DAMM_V2_POOL_AUTHORITY: &str = "HLnpSz9h2S4hiLQ43rnSD9XkcUThA7B8hQMKmDaiTLcC";

/// Metaplex Token Metadata. A DBC create of an SPL token writes its metadata here and
/// swaps never touch it, so DBC and this together pick out DBC's creates.
pub const METAPLEX_METADATA: &str = "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s";

/// Bags' wallet. It signs every DBC create made on bags.fun (21 of 26 SPL creates in an hour on 2026-10-16).
pub const BAGS_WALLET: &str = "BAGSB9TpGrZxQbEsrEznv5jXXdwyP6AXerN8aVRiAmcv";

/// Raydium LaunchLab, the bonding-curve program behind Bonk (LetsBonk) and others.
pub const LAUNCHLAB_PROGRAM: &str = "LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj";

/// pump.fun's Mayhem mode agent: an AI trader pump.fun runs on a coin for its first 24 hours, with a billion tokens
/// minted for it on top of the supply and burned after. It trades from the first seconds and holds about a whole
/// supply, so read as a wallet it was a sniper holding ~100% (2,285 reports, 2026-10-14 to 17). Identified by that
/// behaviour across those coins; pump.fun doesn't publish the address.
pub const PUMP_MAYHEM_AGENT: &str = "BwWK17cbHxwWBKZkUYvzxLcNQ1YVyaFezduWbtm2de6s";

/// Owns every LaunchLab pool's token vault, so it holds supply for the market.
pub const LAUNCHLAB_AUTHORITY: &str = "WLHv2UAZm6z4KyaaELi5pjdbJh6RESMva1Rnn8pJVVh";
/// migrate.fun's program (Hustle Migration, from its SDK docs). Its claim vaults are
/// accounts this program owns: supply held for holders of the old token to claim.
pub const MIGRATE_FUN_PROGRAM: &str = "migK824DsBMp2eZXdhSBAWFS6PbvA6UN8DV15HfmstR";

/// LetsBonk's platform config on LaunchLab. Every Bonk create and trade touches it.
pub const BONK_PLATFORM: &str = "FfYek5vEz23cMkWsdJwG2oa6EphsvXSHrGpdALN4g6W1";

/// Proxima's on-chain router. Any trade through it is the "Made with Proxima" proof.
pub const PROXIMA_ROUTER: &str = "GPJ2n2DBFLh9ZARWkygcJHYuefHydBwCGqwsvNYMrB75";

/// uXento's fee wallet, paid in the create transaction of launches made with it.
/// Found in 5 of 6 uXento-labelled creates on the original, and none of the others.
pub const UXENTO_FEE_WALLET: &str = "uxtoRPdPjRekYmxs1uUqVRZWh38iqonc9KgPuZXPeSY";

pub const WRAPPED_SOL: &str = "So11111111111111111111111111111111111111112";

/// Dollar stablecoins, with their symbols: what people pay with rather than coins, counted at $1.
pub const STABLECOINS: [(&str, &str); 3] = [
    ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", "USDC"),
    ("Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB", "USDT"),
    ("USD1ttGY1N17NEEHLmELoaybftRBUSErhqYiQzvEmuB", "USD1"),
];

pub fn is_stablecoin(mint: &str) -> bool {
    STABLECOINS.iter().any(|(m, _)| *m == mint)
}

/// pump.fun tokens always mint 1B with 6 decimals.
pub const PUMP_TOTAL_SUPPLY: u64 = 1_000_000_000_000_000;

/// Jito tip accounts, for the bundle fallback when the Jito API has no answer yet.
pub const JITO_TIP_ACCOUNTS: [&str; 8] = [
    "96gYZGLnJYVFmbjzopPSU6QiEV5fGqZNyN9nmNhvrZU5",
    "HFqU5x63VTqvQss8hp11i4wVV8bD44PvwucfZ2bU7gRe",
    "Cw8CFyM9FkoMi7K7Crf6HNQqf4uEMzpKw6QNghXLvLkY",
    "ADaUMid9yfUytqMBgopwjb2DTLSokTSzL1zt6iGPaS49",
    "DfXygSm4jCyNCybVYYK6DwvWqjKee8pbDmJGcLWNDXjh",
    "ADuUkR4vqLUMWXxW9gh6D6L8pMSawimctcNZ5pGwDcEt",
    "DttWaMuVvTiduZRnguLF7jNxTgiMBZ1hyAumKUiL2KRL",
    "3AVi9Tg9Uo68tJfuvoKvqKNWKkC5wPdSSdeBnizKZ6jT",
];
