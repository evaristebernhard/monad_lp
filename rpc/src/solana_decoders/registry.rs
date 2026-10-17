//! Solana program registry.
//!
//! The registry is deliberately metadata-only.  It gives the listener a
//! complete set of programs to watch and a stable protocol label, while the
//! protocol modules add richer instruction/account decoding where we have
//! verified the layout.  This is the same useful split used by Carbon's
//! generated decoders, without adding Carbon as a runtime dependency.

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProtocolDescriptor {
    pub protocol: &'static str,
    pub slug: &'static str,
    pub program_id: &'static str,
    pub category: &'static str,
}

macro_rules! protocol {
    ($protocol:literal, $slug:literal, $program_id:literal, $category:literal) => {
        ProtocolDescriptor {
            protocol: $protocol,
            slug: $slug,
            program_id: $program_id,
            category: $category,
        }
    };
}

/// Programs represented by the local Carbon decoder inventory plus the
/// protocol programs already handled by the listener.  Infrastructure
/// programs are included because routed transactions frequently invoke them;
/// the UI can filter by `category` when it wants only market venues.
pub(crate) const ALL: &[ProtocolDescriptor] = &[
    protocol!(
        "Meteora DAMM v2",
        "meteora_damm_v2",
        "cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG",
        "amm"
    ),
    protocol!(
        "Meteora DBC",
        "meteora_dbc",
        "dbcij3LWUppWqq96dh6gJWwBifmcGfLSB5D4DuSMaqN",
        "launchpad"
    ),
    protocol!(
        "Meteora DLMM",
        "solana_dlmm",
        "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo",
        "amm"
    ),
    protocol!(
        "Meteora Pools",
        "meteora_pools",
        "Eo7WjKq67rjJQSZxS6z3YkapzY3eMj6Xy8X5EQVn5UaB",
        "amm"
    ),
    protocol!(
        "Meteora Vault",
        "meteora_vault",
        "24Uqj9JCLxUeoC3hGfh5W3s9FM9uCHDS2SG3LYwBpyTi",
        "vault"
    ),
    protocol!(
        "Orca Whirlpools",
        "solana_whirlpool",
        "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
        "amm"
    ),
    protocol!(
        "Raydium AMM v4",
        "solana_standard",
        "675kPX9MHTjS2zt1qfr1NYHuzeLXfQM9H24wFSUt1Mp8",
        "amm"
    ),
    protocol!(
        "Raydium CLMM",
        "solana_clmm",
        "CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK",
        "amm"
    ),
    protocol!(
        "Raydium CPMM",
        "raydium_cpmm",
        "CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C",
        "amm"
    ),
    protocol!(
        "Raydium Launchpad",
        "raydium_launchpad",
        "LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj",
        "launchpad"
    ),
    protocol!(
        "Raydium Stable Swap",
        "raydium_stable_swap",
        "5quBtoiQqxF9Jv6KYKctB59NT3gtJD2Y65kdnB1Uev3h",
        "amm"
    ),
    protocol!(
        "Raydium Liquidity Locking",
        "raydium_liquidity_locking",
        "LockrWmn6K5twhz3y9w1dQERbmgSaRkfnTeTKbpofwE",
        "liquidity"
    ),
    protocol!(
        "Pump.fun",
        "pumpfun",
        "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P",
        "launchpad"
    ),
    protocol!(
        "Pump Swap",
        "pump_swap",
        "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA",
        "amm"
    ),
    protocol!(
        "Pump Fees",
        "pump_fees",
        "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ",
        "fees"
    ),
    protocol!(
        "Phoenix",
        "phoenix",
        "PhoeNiXZ8ByJGLkxNfZRnkUfjvmuYqLR89jjFHGqdXY",
        "orderbook"
    ),
    protocol!(
        "OpenBook v2",
        "openbook_v2",
        "opnb2LAfJYbRMAHHvqjCwQxanZn7ReEHp1k81EohpZb",
        "orderbook"
    ),
    protocol!(
        "Lifinity",
        "lifinity",
        "2wT8Yq49kHgDzXuPxZSaeLaH1qbmGXtEyPy64bL7aD3c",
        "amm"
    ),
    protocol!(
        "BonkSwap",
        "bonkswap",
        "BSwp6bEBihVLdqJRKGgzjcGLHkcTuzmSo1TQkHepzH8p",
        "amm"
    ),
    protocol!(
        "FluxBeam",
        "fluxbeam",
        "FLUXubRmkEi2q6K3Y9kBPg9248ggaZVsoSFhtJHSrm1X",
        "amm"
    ),
    protocol!(
        "Stabble Stable Swap",
        "stabble_stable_swap",
        "swapNyd8XiQwJ6ianp9snpu4brUqFxadzvHebnAXjJZ",
        "amm"
    ),
    protocol!(
        "Stabble Weighted Swap",
        "stabble_weighted_swap",
        "swapFpHZwjELNnjvThjajtiVmkz3yPQEHjLtka2fwHW",
        "amm"
    ),
    protocol!(
        "PancakeSwap",
        "pancake_swap",
        "HpNfyc2Saw7RKkQd8nEL4khUcuPhQ7WwY1B2qjx8jxFq",
        "amm"
    ),
    protocol!(
        "Jupiter Swap",
        "jupiter_swap",
        "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4",
        "router"
    ),
    protocol!(
        "Jupiter DCA",
        "jupiter_dca",
        "DCA265Vj8a9CEuX1eb1LWRnDT7uK6q1xMipnNyatn23M",
        "router"
    ),
    protocol!(
        "Jupiter Limit Order",
        "jupiter_limit_order",
        "jupoNjAxXgZ4rjzxzPMP4oxduvQsQtZzyknqvzYNrNu",
        "router"
    ),
    protocol!(
        "Jupiter Limit Order v2",
        "jupiter_limit_order_2",
        "j1o2qRpjcyUwEvwtcfhEQefh773ZgjxcVRry7LDqg5X",
        "router"
    ),
    protocol!(
        "Jupiter Lend",
        "jupiter_lend",
        "jupeiUmn818Jg1ekPURTpr4mFo29p46vygyykFJ3wZC",
        "lending"
    ),
    protocol!(
        "Jupiter Perpetuals",
        "jupiter_perpetuals",
        "PERPHjGBqRHArX4DySjwM6UJHiR3sWAatqfdBS2qQJu",
        "perpetuals"
    ),
    protocol!(
        "Drift",
        "drift",
        "dRiftyHA39MWEi3m9aunc5MzRF1JYuBsbn6VPcn33UH",
        "perpetuals"
    ),
    protocol!(
        "Marginfi",
        "marginfi",
        "MFv2hWf31Z9kbCa1snEPYctwafyhdvnV7FZnsebVacA",
        "lending"
    ),
    protocol!(
        "Kamino Lending",
        "kamino_lending",
        "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD",
        "lending"
    ),
    protocol!(
        "Kamino Farms",
        "kamino_farms",
        "FarmsPZpWu9i7Kky8tPN37rs2TpmMrAZrC7S7vJa91Hr",
        "farming"
    ),
    protocol!(
        "Kamino Limit Order",
        "kamino_limit_order",
        "LiMoM9rMhrdYrfzUCxQppvxCSG1FcrUK9G8uLq4A1GF",
        "router"
    ),
    protocol!(
        "Kamino Vault",
        "kamino_vault",
        "kvauTFR8qm1dhniz6pYuBZkuene3Hfrs1VQhVRgCNrr",
        "vault"
    ),
    protocol!(
        "Marinade",
        "marinade",
        "MarBmsSgKXdrN1egZf5sqe1TMai9K1rChYNDJgjq7aD",
        "staking"
    ),
    protocol!(
        "Solayer",
        "solayer",
        "sSo1iU21jBrU9VaJ8PJib1MtorefUV4fzC9GURa2KNn",
        "staking"
    ),
    protocol!(
        "Zeta",
        "zeta",
        "ZETAxsqBRek56DhiGXrn75yj2NHU3aYUnxvHXpkf3aD",
        "perpetuals"
    ),
    protocol!(
        "Virtuals",
        "virtuals",
        "5U3EU2ubXtK84QcRjWVmYt9RaDyA8gKxdUrPFXmZyaki",
        "launchpad"
    ),
    protocol!(
        "Moonshot",
        "moonshot",
        "MoonCVVNZFSYkqNXP6bxHLPL6QQJiMagDL3qcqUQTrG",
        "launchpad"
    ),
    protocol!(
        "Heaven",
        "heaven",
        "HEAVENoP2qxoeuF8Dj2oT1GHEnu49U5mJYkdeC8BAX2o",
        "launchpad"
    ),
    protocol!(
        "Boop",
        "boop",
        "boop8hVGQGqehUK2iVEMEnMrL5RbjywRzHKBmBE7ry4",
        "launchpad"
    ),
    protocol!(
        "Onchain Labs DEX",
        "onchain_labs_dex",
        "proVF4pMXVaYqmy4NjniPh4pqKNfMmsihgd4wdkCX3u",
        "orderbook"
    ),
    protocol!(
        "DFlow",
        "dflow",
        "DF1ow4tspfHX9JwWJsAb9epbkA8hmpSEAtxXy1V27QBH",
        "router"
    ),
    protocol!(
        "OKX DEX",
        "okx_dex",
        "6m2CDdhRgxpH4WjvdzxAYbGxwdGUz5MziiL5jek2kBma",
        "router"
    ),
    protocol!(
        "Gavel",
        "gavel",
        "srAMMzfVHVAtgSJc8iH6CfKzuWuUTzLHVCE81QU1rgi",
        "orderbook"
    ),
    protocol!(
        "Vertigo",
        "vertigo",
        "vrTGoBuy5rYSxAfV3jaRJWHH6nN9WK4NRExGxsk1bCJ",
        "amm"
    ),
    protocol!(
        "Sharky",
        "sharky",
        "SHARKobtfF1bHhxD2eqftjHBdVSCbKo9JtgK71FhELP",
        "lending"
    ),
    protocol!(
        "Swig",
        "swig",
        "swigypWHEksbC64pWKwah1WTeh9JXwx8H1rJHLdbQMB",
        "account"
    ),
    protocol!(
        "Wavebreak",
        "wavebreak",
        "waveQX2yP3H1pVU8djGvEHmYg8uamQ84AuyGtpsrXTF",
        "amm"
    ),
    protocol!(
        "Token Program",
        "token_program",
        "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
        "infrastructure"
    ),
    protocol!(
        "Token-2022",
        "token_2022",
        "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
        "infrastructure"
    ),
    protocol!(
        "Associated Token Account",
        "associated_token_account",
        "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL",
        "infrastructure"
    ),
    protocol!(
        "System Program",
        "system_program",
        "11111111111111111111111111111111",
        "infrastructure"
    ),
    protocol!(
        "Stake Program",
        "stake_program",
        "Stake11111111111111111111111111111111111111",
        "infrastructure"
    ),
    protocol!(
        "Address Lookup Table",
        "address_lookup_table",
        "AddressLookupTab1e1111111111111111111111111",
        "infrastructure"
    ),
    protocol!(
        "Memo",
        "memo",
        "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
        "infrastructure"
    ),
    protocol!(
        "Metaplex Token Metadata",
        "mpl_token_metadata",
        "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s",
        "infrastructure"
    ),
    protocol!(
        "Metaplex Core",
        "mpl_core",
        "CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d",
        "infrastructure"
    ),
    protocol!(
        "Bubblegum",
        "bubblegum",
        "BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY",
        "infrastructure"
    ),
    protocol!(
        "Circle Message Transmitter",
        "circle_message_transmitter",
        "CCTPV2Sm4AdWt5296sk4P66VBZ7bEhcARwFaaS9YPbeC",
        "bridge"
    ),
    protocol!(
        "Circle Token Messenger",
        "circle_token_messenger",
        "CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe",
        "bridge"
    ),
];

pub(crate) fn by_program(program_id: &str) -> Option<&'static ProtocolDescriptor> {
    ALL.iter()
        .find(|descriptor| descriptor.program_id == program_id)
}

pub(crate) fn by_slug(slug: &str) -> Option<&'static ProtocolDescriptor> {
    ALL.iter().find(|descriptor| descriptor.slug == slug)
}

pub(crate) fn all_program_ids() -> Vec<&'static str> {
    ALL.iter().map(|descriptor| descriptor.program_id).collect()
}
