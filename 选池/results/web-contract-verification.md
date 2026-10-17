# Web / Solidity verification — 2026-10-17

- Foundry: 17 tests passed (8 V4 selector + 9 DCAR V2).
- Node integer replay / input validation: 4 tests passed.
- Existing Python audit regressions: 9 passed.
- Backend `cargo build` and frontend `npm run build`: passed. Existing main frontend bundle >500 kB warning remains; selection page is lazy-loaded.
- Browser -> API -> actual Monad testnet RPC: V4 fixed-block quote selection and DCAR default sample both matched local output and input hash.
- Real DCAR testnet deployment: 0xf4c5146a6535c7f58052e4ec185b3cbdfce4d2ff. Three actual read-only replays saved in dcar-testnet-deployment.json.
- Browser: invalid amount handling, clearing verification after input changes, desktop/mobile layout, no horizontal overflow at 390px, no page errors: passed.
- Mock injected wallet: rejected network switch, account changed during switch, rejected deployment signature: passed. Confirmed submission only after switching to chain 10143. Wallet tests do not broadcast a transaction.
- API rejects eth_sendRawTransaction and returns chain ID 0x279f for eth_chainId.

Scope: V4 inputs are historical real quotes; DCAR V2 inputs are synthetic. Neither contract swaps or authenticates the supplied observations. DCAR is a bounded numerical candidate-core port, not a certified thousand-pool optimizer.
