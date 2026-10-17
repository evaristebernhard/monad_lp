# Monad Testnet tiny on-chain AI trader

This experiment targets Monad **testnet 10143 only**. Do not deploy to chain 143.

## Verified components

- Existing 131-parameter model: `0x5dd31af6d70b7633fd5da3e30c03feb5fde00618`.
- PuddleSwap testnet Router: `0x430c23895c8D44883526e3E0B09327dAD8766660`.
- USDC: `0x534b2f3A21130d7a60830c2Df862319e593943A3` (6 decimals).
- Router WMON: `0x97B3070F9Da6C002343862b35E68Bd8e22608943` (18 decimals).
- Read-only probe at block 71754270 found Router, factory, tokens, and USDC/WMON pair code. It returned nonzero reserves and buy/sell quotes.
- Official-list WMON has a **different address**. Do not mix tokens or assume they are interchangeable. Verify wrapping and unwrapping for the router WMON before depositing native MON.

## Scope

`../evm/src/TinyMonadTrader.sol` calls the already-deployed model's Solidity `predict()`. Its output selects HOLD, USDC -> WMON, or WMON -> USDC. This is real inference inside the transaction, not an off-chain BUY/SELL signal. A keeper supplies the 12 quantized features and completed hourly candle end time. The contract does not authenticate those features. The keeper must calculate them from current completed market data, using the original model's training means and standard deviations. The model was trained on Monad **mainnet** market history; the testnet USDC/WMON price is unrelated. Do not interpret this as validated trading profitability.

Safety limits: immutable model, tokens, router, trade ceilings; testnet chain ID; fixed keeper; owner pause and withdrawal; once-per-hour timestamp replay guard, three-hour freshness bound; caller-specified minimum receive amount within a 3% bound from on-chain router quote; post-swap balance checks; reentrancy lock. Funds reside in this contract only after the owner deposits. Fund with test assets only. It is not audited.

## Verification commands

```sh
node research/ai/monad-small/probe-dex.mjs
forge test --root research/ai/evm --match-contract TinyMonadTraderTest -vv
```

## Completed deployment — 2026-10-17

- Foundry Forge 1.6.0-rc1 compiled the contracts with solc 0.8.28.
- All three initial `TinyMonadTraderTest` tests passed.
- The preflight confirmed model hash, router WMON, token decimals and bytecode.
- `TinyMonadTrader` was deployed and getter-verified on Monad Testnet (10143).
- Contract: `0x8263ef59897d68484f4d813b316b7bda590557c9`.
- Transaction: `0x617f8c06c701d7a3b165082e77413e40f850aff4e1f8ff94004c3d65234b464c`.
- Block: `71757311`. Charged deployment gas fee: `0.1686648` test MON.
- Keeper/owner: `0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79`.
- Trading caps: 1 USDC per BUY; 0.25 WMON per SELL.
- Deployment manifest: `trader-deployment.json`; rerunning `node research/ai/monad-small/trader-deploy.mjs --deploy` verifies the existing deployment rather than redeploying.
- No assets have been funded and no BUY or SELL swaps have been sent as part of this deployment.

## Additional deployment: 2026-10-17, newer block

The working tree also contains a second testnet deployment from the same model and Router, but with a 1 WMON SELL ceiling rather than 0.25 WMON. Both contracts exist on chain. This is a **second deployment**, not an upgrade of the first contract.

- Newer contract: `0xcc0ea79ebbce1121fc90f026b28b995ae5e2741e`.
- Deployment transaction: `0xa1591589239be07d25b6f9039bd8c563856ec49ba7801d98d59a2a549da8f005`.
- Block: `71757703`. Paid gas limit fee: `0.1763314` test MON.
- Owner and keeper: `0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79`.
- Trading caps: 1 USDC per BUY and 1 WMON per SELL.
- Manifest: `trader-testnet-deployment.json`; tool: `deploy-trader.mjs`. Source code was compiled and four local execution tests passed.
- Both contracts have no automated keeper loop or test-swap verification yet. Do not fund or automate both accidentally.



## Next steps: end-to-end swap verification

1. Verify the Router's swap ABI against its deployed code and confirm WMON wrapping and redeemability on testnet before depositing MON. Use only small test assets.
2. Fund the deployed trader with test USDC and compatible test WMON. Run a fresh transaction gas estimate before each transaction.
3. Call `execute()` with known BUY, SELL and HOLD feature vectors for a fresh closed candle. Confirm emitted events, real token balance deltas, cooldown and replay rejection using receipts.
4. Add a keeper to fetch *completed* candles, build the exact 12 Q12 features from the training scaler, and submit them. If input is missing or stale, do not send a transaction.
5. Display confirmed actions, transaction hashes, token changes and gasLimit charges in the existing web UI.

Pool verification is read-only, not evidence that trade execution succeeded. V2 Router quote is not guaranteed realized output. The public testnet may change between probe and broadcast.
