# All DEX 与期权对冲研究边界

本文是项目当前的协议目录和期权对冲研究说明。它回答“我们现在收录了哪些 DEX、每类池子需要读取什么状态、期权对冲做到哪一步”，不把未完成的研究结果包装成可执行策略。

## 1. 当前数据库快照

来源：`data/lp_screener.db`。

该文件是本地运行时快照；由于超过 GitHub 普通 Git 文件的大小限制，完整副本通过项目的 GitHub Release asset 分发。协议清单和对冲边界以本文档及数据库结构为准，不依赖旧的 JSON/CSV 导出。

| 指标 | 数量 |
| --- | ---: |
| 池子 | 40,627 |
| 链 | 14 |
| DEX/协议名称 | 65 |
| AMM family | 13 |
| `stable_quote` 池 | 1,316 |
| `priority_pools` | 1,000 |
| 当前期权 snapshot | 0 |
| 当前 `hedge_analysis` | 0 |

`stable_quote` 只表示池子的一侧被识别为稳定币报价，并不表示池子已经可以期权对冲。期权市场是否存在，取决于 volatile token 是否有可用的期权标的、期限、执行价和报价。

## 2. 数据和计算边界

```text
rpc/workers/lp-refresh  读取池状态、区块、储备、tick/bin 和协议参数
rpc/src/price           写入 token 原始价格历史
rpc/option              读取公共 Deribit 报价，写入 option_snapshots/option_quotes
backend/lp-metrics      计算均价、pair price、日波动率和手续费指标
backend/lp-options      读取池数据和期权报价，计算 hedge_analysis
frontend                只展示 backend API
```

期权对冲的正确顺序是：

1. 先确认池子的 AMM family 和当前链上状态；
2. 确认 token 身份和稳定币报价方向；
3. 由 backend 使用历史价格计算 token 波动率、均价和 LP 区间；
4. 由 `rpc/option` 保存同一时期的公共期权报价；
5. 由 `backend/lp-options` 计算权利金、复制误差和净收益；
6. 结果才写入 `hedge_analysis`。

池目录重建不能从旧 JSON 或旧报告恢复 `hedge_analysis`。没有最新 option snapshot 时，网页必须显示未加载，不得显示旧的对冲方案。

## 3. 期权标的范围

当前 Rust 期权采集器使用 Deribit 公共接口，已经定义的直接标的是：

| 标的 | 池内别名 | 直接对冲状态 |
| --- | --- | --- |
| SOL | `SOL`、`WSOL` | 可研究，前提是有新 snapshot |
| ETH | `ETH`、`WETH` | 可研究，前提是有新 snapshot |
| BTC | `BTC`、`WBTC` | 可研究，前提是有新 snapshot |

例如 `MON/USDC`、普通 ERC-20、长尾 Solana token 即使池子数据完整，也不能因为有 `stable_quote` 就自动生成期权费。它们应记录为没有直接期权面，而不是填充一个估算的权利金。

当前数据库没有 option snapshot，因此当前没有任何完成的期权对冲方案。这是正确状态，不是数据缺失错误。

## 4. AMM family 与对冲模型

| AMM family | 池数 | 需要的链上状态 | 对冲研究重点 |
| --- | ---: | --- | --- |
| `constant_product_v2` | 20,714 | reserves、fee、token 顺序 | 可复用统一 LP 区间和价格路径模型 |
| `concentrated_v3` | 9,493 | sqrt price、tick、liquidity、tick spacing | 必须区分当前价格和 LP position range |
| `uniswap_v4` | 5,855 | poolId、StateView、hooks、动态 fee | hook 和动态手续费不能套用 V3 默认值 |
| `solana_clmm` | 1,250 | pool account、sqrt price、tick/liquidity | 需要 Raydium CLMM 专用解码 |
| `solana_standard` | 1,093 | reserves、token 顺序、fee | 可使用 constant-product 类模型，但要确认账户布局 |
| `solana_dlmm` | 722 | bin step、active bin、bin liquidity、fee | 需要 Meteora bin 暴露和区间映射 |
| `liquidity_book` | 487 | bins、active id、bin liquidity、动态 fee | 不能当成普通 V2 储备池 |
| `solidly_v2` | 405 | reserves、稳定/volatile 曲线、方向 fee | 要保留方向性和曲线类型 |
| `stable_swap` | 217 | balances、amp/virtual price、fee | 主要研究稳定资产风险，不能用普通 V2 曲线 |
| `weighted` | 189 | balances、weights、fee | LP 暴露是加权组合，不是 50/50 常数乘积 |
| `solana_whirlpool` | 163 | Whirlpool account、tick、liquidity、fee | 需要 Orca Whirlpool 专用解码 |
| `fluid_dex` | 24 | packed storage、价格和 fee 状态 | 需要 Fluid 专用存储解码 |
| `maverick_v2` | 15 | state、bins、方向 fee、token 状态 | 需要 Maverick 的 bin/方向模型 |

AMM family 只决定池状态和 LP 暴露的读取方式，不决定是否存在期权。期权是否可用由 token 和外部期权市场决定。

## 5. 当前收录的全部 DEX

下面的数量由当前 SQLite 快照生成；括号内依次为“部署链 / AMM family / 池数 / `stable_quote` 池数”。

| DEX/协议 | 当前部署 |
| --- | --- |
| PancakeSwap V2 | BSC / `constant_product_v2` / 17,095 / 7；Ethereum / `constant_product_v2` / 6 / 0 |
| Uniswap V4 | Arbitrum 367、Base 883、BSC 3,610、Ethereum 388、Monad 47、Polygon 560；合计 5,855 / 22 |
| Uniswap V3 | Arbitrum 510、Base 257、BSC 916、Ethereum 509、Monad 1,482、Optimism 424、Polygon 834；合计 4,932 / 15 |
| PancakeSwap V3 | Arbitrum 96、Base 28、BSC 3,208、Ethereum 24、Linea 18、Monad 37；合计 3,411 / 26 |
| Raydium | Solana `solana_clmm` 1,250、`solana_standard` 1,093；合计 2,343 / 860 |
| QuickSwap V2 | Polygon / `constant_product_v2` / 1,170 / 0 |
| Uniswap V2 | Arbitrum、Base、Ethereum、Monad、Optimism、Polygon；合计 1,069 / 4 |
| Meteora DLMM | Solana / `solana_dlmm` / 722 / 261 |
| SushiSwap | Arbitrum、Base、Ethereum、Polygon；合计 503 / 0 |
| LFJ | Arbitrum、Avalanche / `liquidity_book` / 411 / 2 |
| Velodrome V2 | Optimism / `solidly_v2` / 272 / 4 |
| ApeSwap | BSC / `constant_product_v2` / 223 / 0 |
| QuickSwap V3 | Polygon / `concentrated_v3` / 203 / 0 |
| Curve | Arbitrum、Ethereum、Optimism、Polygon / `stable_swap` / 177 / 3 |
| Balancer V2 | Arbitrum、Ethereum、Polygon / `weighted` / 165 / 0 |
| Orca Whirlpools | Solana / `solana_whirlpool` / 163 / 73 |
| Pangolin | Avalanche / `constant_product_v2` / 157 / 0 |
| SushiSwap V3 | Arbitrum、Base、Ethereum、Optimism / `concentrated_v3` / 139 / 1 |
| Camelot V2 | Arbitrum / `constant_product_v2` / 126 / 0 |
| Mdex | BSC / `constant_product_v2` / 125 / 0 |
| Aerodrome SlipStream | Base / `concentrated_v3` / 93 / 0 |
| Aerodrome | Base / `solidly_v2` / 91 / 0 |
| Camelot V3 | Arbitrum / `concentrated_v3` / 88 / 0 |
| SyncSwap | Linea、zkSync / `constant_product_v2` / 85 / 5 |
| Aerodrome SlipStream 3 | Base / `concentrated_v3` / 81 / 0 |
| Uniswap v3 | Avalanche / `concentrated_v3` / 69 / 0 |
| BakerySwap | BSC / `constant_product_v2` / 61 / 0 |
| Pharaoh V3 | Avalanche / `concentrated_v3` / 60 / 1 |
| Blackhole V3 | Avalanche / `concentrated_v3` / 59 / 1 |
| Agni Finance V3 | Mantle / `concentrated_v3` / 47 / 3 |
| Velodrome SlipStream | Optimism / `concentrated_v3` / 47 / 0 |
| THENA FUSION | BSC / `solidly_v2` / 42 / 0 |
| Thena V3 | BSC / `concentrated_v3` / 36 / 0 |
| Aerodrome SlipStream 2 | Base / `concentrated_v3` / 31 / 0 |
| LFJ V2.2 | Arbitrum、Avalanche / `liquidity_book` / 30 / 1 |
| Lynex | Linea / `concentrated_v3` / 28 / 0 |
| Thruster v3 | Blast / `concentrated_v3` / 28 / 8 |
| Curve TwoCrypto | Ethereum / `stable_swap` / 27 / 0 |
| Etherex | Linea / `concentrated_v3` / 26 / 6 |
| Merchant Moe Liquidity Book | Mantle / `liquidity_book` / 25 / 2 |
| Beethoven X | Optimism / `weighted` / 24 / 0 |
| Fluid | Arbitrum、Ethereum / `fluid_dex` / 24 / 0 |
| Pancakeswap v3 | zkSync / `concentrated_v3` / 24 / 0 |
| KyberSwap | BSC、Ethereum、Polygon / `constant_product_v2` / 23 / 0 |
| Retro | Polygon / `concentrated_v3` / 23 / 0 |
| LFJ V2.1 | Arbitrum、Avalanche / `liquidity_book` / 21 / 0 |
| Ramses V2 | Arbitrum / `concentrated_v3` / 20 / 0 |
| zkSwap | zkSync / `constant_product_v2` / 17 / 0 |
| Maverick V2 | Arbitrum、Base、BSC、Ethereum / `maverick_v2` / 15 / 0 |
| PancakeSwap StableSwap | BSC / `stable_swap` / 13 / 0 |
| Pharaoh | Avalanche / `constant_product_v2` / 13 / 0 |
| Nuri V2 | Scroll / `concentrated_v3` / 12 / 2 |
| SyncSwap V2.1 | Linea / `constant_product_v2` / 12 / 2 |
| Hydrex | Base / `constant_product_v2` / 8 / 0 |
| Nile | Linea / `concentrated_v3` / 8 / 1 |
| ShibaSwap | Ethereum / `constant_product_v2` / 7 / 0 |
| iZiSwap | Linea / `concentrated_v3` / 7 / 0 |
| Alien Base V3 | Base / `concentrated_v3` / 6 / 0 |
| Honeypop DEX | Scroll / `concentrated_v3` / 6 / 3 |
| Nile V1 | Linea / `constant_product_v2` / 6 / 0 |
| Sushiswap v3 | Scroll / `concentrated_v3` / 5 / 1 |
| Ekubo V3 | Ethereum / `concentrated_v3` / 4 / 0 |
| FraxSwap | Ethereum / `constant_product_v2` / 4 / 0 |
| Agni Finance V2 | Mantle / `constant_product_v2` / 2 / 0 |
| DyorSwap | Monad / `constant_product_v2` / 2 / 2 |

## 6. 后续工作顺序

1. 先保持 `pools`、`pool_rpc_state` 和 token metadata 的事实一致；
2. 继续补齐 AMM family 的 RPC 解码和同区块刷新；
3. 对 `stable_quote` 池建立可靠的 token 价格历史；
4. 只对 SOL、ETH、BTC 等存在直接期权市场的标的刷新 option snapshot；
5. 由 backend 生成对冲研究结果；
6. 对没有直接期权面的 token 明确标记原因，不伪造方案。

因此，“all DEX”在本项目中首先意味着所有 DEX 都进入同一套目录、状态和研究边界；并不意味着所有 DEX 的所有 token 现在都已经有真实可执行的期权对冲报价。
