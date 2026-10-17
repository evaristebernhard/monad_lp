# LP pool decoder and SDK map

This is the decoder contract for the Rust refresher. A protocol family is
only marked `live` when all fields required by that family are decoded. A
successful `eth_call` to an unrelated selector is not evidence that a pool is
healthy.

## Decoder order

| Family | State source | Required state | Quote path |
| --- | --- | --- | --- |
| `constant_product_v2` | pair contract | `token0`, `token1`, `getReserves`, decimals, fee policy | pair/router exact-input quote |
| `concentrated_v3` | pool contract | `token0`, `token1`, `slot0`, `liquidity`, `fee`, `tickSpacing` | Quoter / exact protocol quote |
| `solidly_v2` | pair + factory | reserves, `stable` flag, factory fee | router quote; stable and volatile math differ |
| `curve` | pool + registry | `coins(i)`, `balances(i)`, `fee`, rates, decimals | `get_dy(i,j,dx)` |
| `balancer_weighted` | WeightedPool + Vault | `poolId`, normalized weights, pool fee; Vault balances for quote execution | Vault query / batch swap simulation |
| `liquidity_book` | LBPair | `tokenX/Y`, reserves, `activeId`, `binStep`, active bins, fee parameters | LBPair quote methods |
| `algebra_v3` | Algebra-style pool | `token0/1`, `globalState`, liquidity, fee | Algebra/DEX quoter |
| `maverick_v2` | Maverick V2 pool | `tokenA/B`, `getState`, directional fees, bin state | Maverick V2 quoter |
| `ekubo_v3` | Ekubo singleton Core storage | pool id, packed sqrt ratio, signed tick, active liquidity | Ekubo router/quoter |
| `uniswap_v4` | PoolManager/StateView | poolId, slot0, active liquidity, pool key, manager balances | V4 Quoter / SDK |
| `raydium_clmm` | Solana PoolState + tick arrays | mint/vaults, Q64.64 price, tick, liquidity, fee | Raydium SDK quote |
| `orca_whirlpool` | Whirlpool + tick arrays | mints, Q64.64 price, tick, liquidity, fee | Orca Whirlpool SDK quote |
| `meteora_dlmm` | pool + bin arrays | active id, bin step, active bin, dynamic fee | Meteora DLMM SDK quote |

## SDKs and source of truth

- Uniswap V4: [`@uniswap/v4-sdk`](https://developers.uniswap.org/docs/sdks/v4/overview)
  for PoolKey/PoolId and quoting, and the official
  [`StateView`](https://github.com/Uniswap/v4-periphery/blob/main/src/lens/StateView.sol)
  lens for off-chain state reads. V4 pools share a PoolManager; the indexed
  pool id is not a pair contract address.
- Curve: the [`Curve pool registry`](https://github.com/curvefi/curve-pool-registry/blob/master/contracts/Swaps.vy)
  and pool-native `get_dy`/`coins`/`balances` interfaces. Do not replace
  `balances(i)` with ERC-20 `balanceOf`: Curve's accounting balance can differ
  from the wallet balance.
- Balancer: [`balancer-sdk`](https://github.com/balancer/balancer-sdk) for pool
  discovery and query simulation; Vault `getPoolTokens(poolId)` is the
  canonical balance source. The Rust worker currently decodes the WeightedPool
  metadata (`poolId`, weights, fee) and retains the Vault address; balance
  reads remain a separate quote/execution adapter.
- Fluid DEX: [`fluid-contracts-public`](https://github.com/Instadapp/fluid-contracts-public)
  for the packed storage layout. The Rust worker reads the pool's two exposed
  storage words and keeps both raw values plus decoded price/fee bit fields.
- Liquidity Book: [`joe-v2 LBPair`](https://github.com/lfj-gg/joe-v2/blob/main/src/LBPair.sol)
  and its SDK conventions. `activeId` and `binStep` are required for price;
  total reserves alone are not active liquidity. The LFJ legacy-compatible
  path accepts the older `getReserves()` three-word response and uses the
  indexed token addresses when the immutable token getters are unavailable.
- Algebra: [`IAlgebraPoolState`](https://github.com/cryptoalgebra/algebra-integral-docs/blob/main/integration-of-algebra-integral-protocol/contracts-api/Core/interfaces/pool/IAlgebraPoolState.md)
  exposes `globalState()` rather than Uniswap V3 `slot0()` on Algebra-style
  pools. `lastFee` is in hundredths of a bip.
- Maverick V2: [`IMaverickV2Pool`](https://github.com/maverickprotocol/v2-interfaces/blob/main/contracts/interfaces/IMaverickV2Pool.sol)
  defines the packed `State` return from `getState()` and the directional
  `fee(bool)` calls. The Rust decoder preserves raw state words for later bin
  depth/quote work.
- PancakeSwap StableSwap: the official
  [`StableSwap pool catalog`](https://github.com/pancakeswap/pancake-developer/blob/master/docs/pages/contracts/stableswap/stableswap-pools.md)
  deployments expose the Curve-compatible `coins(i)`, `balances(i)`, `fee()`,
  and `get_virtual_price()` state used by the Rust Curve decoder.
- SyncSwap V2.1: the official
  [`SyncSwapClassicPool`](https://github.com/syncswap/core-contracts/blob/master/contracts/pool/classic/SyncSwapClassicPool.sol)
  exposes `token0`, `token1`, and `getReserves`, so these rows use the
  constant-product decoder rather than remaining `unsupported_protocol`.
- Ekubo V3: the official
  [`CoreStorageLayout`](https://github.com/EkuboProtocol/evm-contracts/blob/main/src/libraries/CoreStorageLayout.sol)
  stores pool state directly at the 32-byte pool id. The decoder calls the
  Core `sload()` exposure and unpacks the layout defined by
  [`poolState.sol`](https://github.com/EkuboProtocol/evm-contracts/blob/main/src/types/poolState.sol).
- Raydium/Orca/Meteora: the production account decoders are in Rust and use
  protocol-native fixed-point layouts. The Raydium CLMM reference is the
  official [`PoolState`](https://github.com/raydium-io/raydium-clmm/blob/master/programs/amm/src/states/pool.rs);
  Raydium CPMM uses the official [`PoolState`](https://github.com/raydium-io/raydium-cp-swap/blob/master/programs/cp-swap/src/states/pool.rs);
  Orca uses the official [`Whirlpool`](https://docs.rs/orca_whirlpools_client/latest/orca_whirlpools_client/struct.Whirlpool.html);
  Meteora uses the official [`LbPair` IDL](https://raw.githubusercontent.com/MeteoraAg/dlmm-sdk/main/idls/dlmm.json).
  Decoder versions are `raydium-clmm-account-v1`, `raydium-cpmm-account-v1`,
  `orca-whirlpool-account-v1`, and `meteora-dlmm-lbpair-account-v1`.
  The current Solana mint offsets (including the 8-byte account discriminator)
  are Raydium CLMM `73/105`, Orca Whirlpool `101/181`, Raydium CPMM `168/200`,
  and Meteora DLMM `88/120`. These are only used after the account family has
  been identified; the worker does not infer a protocol from arbitrary byte
  positions. Meteora's `LbPair` places `token_x_mint` and `token_y_mint` after
  the 32-byte static and 32-byte variable parameter blocks, matching the
  published account definition.

Uniswap V4 is a special catalog case: `StateView` returns the pool price and
liquidity for a `poolId`, but the state row need not repeat the two assets. The
price layer therefore falls back to `pools.token_a_address/token_b_address`
when the V4 state row has no token references. This is a catalog fallback, not
a second price decoder.

Raydium CPMM is a two-stage read: the PoolState gives the two vault addresses
and mint decimals, then the worker reads each SPL token account and decodes the
raw amount as the constant-product reserve. If a token has a previously
sampled USD price, that history may anchor a disconnected pool component; it
is not treated as a new volatility observation by itself.

## Storage changes required

The current two-token columns are retained for the common V2/V3 path. Every
decoder also emits:

```text
pool_protocol_state(
  pool_address, pool_family, decoder_version, decoder_status,
  state_block, state_json
)

pool_assets(
  pool_address, asset_index, token_address, decimals,
  balance_raw, balance_usd, asset_json
)
```

Curve and Balancer must not be flattened to `token_a/token_b`; V4 and Solana
must retain their protocol-native identifiers and fixed-point values.

## Safety rules

1. `live` means required state is complete; partial calls become
   `partial_state`.
2. `unsupported_protocol` never sends a V2/V3 selector.
3. Raw liquidity is never written to `liquidity_usd`.
4. TVL is only computed when token decimals and token prices are known.
5. A quote is only executable when the protocol-specific quote path is
   available; indexed APR remains research-only.
6. `invalid_index` is reserved for catalog rows that resolve to a contract but
   fail pool identity validation; these rows are excluded from live-pool use
   until re-indexed.
