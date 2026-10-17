# Pool 历史价格：按链与协议选择低成本来源

日期：2026-10-07。范围：研究，不实施、不测速、不发新的链上 RPC 请求。使用已有知识、此前已查阅的官方资料，以及项目代码与数据库的只读检查。

当前目录数据库读数：65 个原始协议标签、大小写归一后 62 个、14 条链、108 个 protocol × chain 组合。标签数量不是独立实现数量。下表涵盖全部原始标签，但路线是候选，不代表每个部署的 ABI、日志完整性和端点能力已经验证。

## 结论

常见池的最小价格状态很小。问题是历史数据获取方式和重复工作，而不是 168 个数字本身很大。最合适的路线应按「池实现 × 链时间模型 × 端点能力 × 交易密度」选择，不能一律历史 view，也不能一律事件。

历史 eth_call 不收取交易 gas，但节点仍需读取对应历史状态并执行合约；RPC 服务有计算和限流成本。历史日志通常不需要保留每个历史状态的 trie，却仍需要区块/收据数据及可用的日志索引。日志可用与历史状态可用是两项不同能力。

先固定价格含义：token1 per token0 的池状态价格，统一 decimals 和方向。成交金额之比是某笔成交的平均执行价格，不能自动替代交易后的 spot。外部 oracle、TWAP、EMA 不作为这条序列的替代品。

## 链这一层如何影响方案

| 库中链 | 必须分清的特性 | 对低成本取数的影响 |
| --- | --- | --- |
| ethereum | EVM 历史状态与历史日志分别保留；slot 不等于每个 slot 都有执行块 | 同类多池批量过滤日志；最小历史 view 是另一个候选，时间用实际执行块 timestamp |
| bsc | 公共端点的方法支持、日志索引与限流不能从“健康”推断 | 先确认端点提供 logs；大量池共享块窗口和头信息，不能每池扫描同一段链 |
| polygon | 本库 polygon 指 Polygon PoS，不是 zkEVM；不能照搬 Ethereum 出块假设 | 通用 EVM 协议解码可复用，区块时间映射和端点能力单独配置 |
| arbitrum | RPC 的 L2 block number 与 Solidity block.number 语义不同；连续块可能同 timestamp | 按 RPC L2 块与真实时间采样；不要拿合约读出的 block.number 当历史 RPC 块号 |
| base | OP 系 L2；文档中的日志响应有 blockTimestamp 扩展 | 端点确实返回该字段时可省头查询；不能假设所有兼容端点均返回 |
| optimism | OP 系 L2；latest/safe/finalized 不能混为一谈 | 历史窗口选一致确认口径；小时边界与时间缓存全链共享 |
| blast | 可复用 EVM/OP 系取数思路，不代表端点配置与 Base 相同 | 目前只有 Thruster v3，先限定部署 ABI；数值型日志上限未知 |
| mantle | EVM L2 的执行块与 L1 发布/批次不等价 | 用执行块 timestamp；LFJ/Moe 的 bin 价格逻辑与普通 V3 分开 |
| linea | EVM L2 的交易执行时间与证明完成时间不等价 | 价格小时序列按执行块，不按 proof 时间；协议实现和端点能力仍需单独核对 |
| scroll | EVM L2，兼容接口不意味着相同历史保留与日志区间限制 | 协议解码复用，provider 配置独立；不能给所有 Scroll RPC 写死一个范围上限 |
| zksync | L2 blocks/miniblocks 与 L1 batches 是不同坐标 | 用 L2 block/timestamp，不用 L1 batch timestamp 代替每个事件的时间 |
| avalanche | 本项目池在 C-Chain；日志范围和历史状态保留可由节点配置 | 不把某节点配置默认值当公共 RPC 承诺；LB 与 V3 分别解码 |
| monad | EVM 接口兼容不等于历史状态保留和服务限额相同 | 快链按同样 block 数分段可能导致更多分页；需要端点范围/返回量能力，不能按 Ethereum 固定速度倒推 |
| solana | 账户读取不是 EVM 历史 eth_call；minContextSlot 不是指定过去 slot | 需要程序事件/历史交易或带历史账户版本的索引来源；普通 getAccountInfo 无法直接读七天前账户 |

上述是架构差异，不是所有端点的实测能力。14 条链的具体 provider 日志区间、返回大小、近七日保留情况仍未知，不能声称已找到了全部 cheap endpoint。

已查阅的直接依据：

- [BSC 官方端点说明](https://docs.bnbchain.org/bnb-smart-chain/developers/json_rpc/json-rpc-endpoint/)说明所列主网公共端点禁用了 eth_getLogs；[节点说明](https://docs.bnbchain.org/bnb-smart-chain/developers/node_operators/full_node/)说明禁用日志索引会让日志查询变慢。这不是 BSC 协议不支持事件。
- [Arbitrum Solidity 差异](https://docs.arbitrum.io/arbitrum-essentials/arbitrum-vs-ethereum/solidity-support)区分合约 block.number 的语义。
- [Base eth_getLogs](https://docs.base.org/base-chain/api-reference/ethereum-json-rpc-api/eth_getLogs)列出地址数组、topics 过滤以及 blockTimestamp；部署端点是否支持扩展仍待确认。
- [ZKsync blocks](https://docs.zksync.io/zksync-protocol/era-vm/transactions/blocks)区分 L2 block 与 L1 batch。
- [Avalanche C-Chain 配置](https://build.avax.network/docs/nodes/chain-configs/primary-network/c-chain)将日志请求范围和裁剪模式下历史状态保留分别配置。
- [Solana getAccountInfo](https://solana.com/docs/rpc/http/getaccountinfo)没有任意历史 slot 状态读取参数。

## 协议路线代号

| 代号 | 首选研究方向 | 最小状态备选与注意事项 |
| --- | --- | --- |
| R | 普通 V2 类：完整 Sync 事件恢复储备，然后计算恒定乘积池状态价格 | 每小时只读 getReserves；不得仅按名称断言部署为恒定乘积 |
| S | V2 接口但可能有 stable、特殊 invariant 或混合子类型：先确认数学模型及完整事件 | 储备事件不自动等于价格；stable 需 invariant 导数/明确大小的 quote，Kyber Classic 需核对其虚拟储备模型 |
| C | 标准 V3/Slipstream 候选：Initialize + 含交易后 sqrt price 的 Swap 事件 | 每小时只读 slot0/对应价格状态；分叉 ABI 需确认，不额外读 liquidity 和 fee 来算 sqrt spot |
| A | Algebra 候选：按版本解码含价格的 Swap/初始化事件 | globalState 等最小状态；Original/Integral、插件与部署版本不能混用 |
| U | V4：PoolManager 单例事件按 poolId 过滤并恢复 sqrt price | StateView 最小 slot0；poolId 不是合约地址，hooks/custom accounting 的可执行价格需另定义 |
| L | Liquidity Book：完整 activeId 变更路径与初始化状态恢复 bin 价格 | activeId + 一次性 binStep 元数据；不能用总储备比替代 active bin，指数中心偏移需按实现 |
| Q | StableSwap/Curve：最小历史价格 view 或有明确交易量的 get_dy 类报价 | quote 不是自动无费用无滑点的边际价；A ramp、rates、rebase 等会让“没 swap 所以价格不变”不成立 |
| B | Balancer 类：Vault 全部相关余额变更 + 权重/rate 模型；条件不足就最小历史状态/quote | Swap 金额不含完整 post-state；只读 weights/fee 而没有对应 balances 不能恢复 weighted spot |
| T | TWAMM/FraxSwap：研究虚拟订单执行与池状态更新路径 | 不能直接当成普通 V2；定义存储状态价还是包含待执行虚拟订单的可执行价 |
| X | 特殊模型：按原生 pool state/事件逐一研究 | Ekubo、Fluid、Maverick、iZiSwap 不共享一种价格存储格式；现有读取分支不证明历史价格正确 |
| O | Orca：Whirlpool 历史程序事件/交易中的 post-sqrt 或历史账户版本 | 普通当前账户读取只解决当前价；事件字段和 CPI 覆盖需确认 |
| M | Meteora DLMM：历史 active bin 状态与程序事件 | 与 EVM LB 思路相似，但数据通道、程序版本和事件完整性不同 |
| Y | Raydium：CLMM 与 standard 子类型拆分，再选历史事件/交易/账户版本 | standard 还需区分具体程序和 OpenBook 等依赖，不能所有池统一按 vault 余额比 |

标准族的依据可见 [Uniswap V2 Pair](https://github.com/Uniswap/v2-core/blob/master/contracts/UniswapV2Pair.sol)、[V3 events](https://github.com/Uniswap/v3-core/blob/main/contracts/interfaces/pool/IUniswapV3PoolEvents.sol)、[V4 PoolManager interface](https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol)。这些上游定义不能替代分叉部署的 ABI 核验。其余分组是已有知识与项目分类支持的候选，不能据此宣布全部事件可完整重建。

## 全部 protocol × chain 候选对应

同一行多个 chain 分别算组合；共 65 行、108 组合。特别是 Ramses V2、Nuri V2 等名字里的 V2 不等于普通 V2 储备池。分类参考项目已有分支/目录，仅是研究起点。

| 原始协议标签 | chain | 候选路线 |
| --- | --- | --- |
| Aerodrome | base | S |
| Aerodrome SlipStream | base | C |
| Aerodrome SlipStream 2 | base | C |
| Aerodrome SlipStream 3 | base | C |
| Agni Finance V2 | mantle | R |
| Agni Finance V3 | mantle | C |
| Alien Base V3 | base | C |
| ApeSwap | bsc | R |
| BakerySwap | bsc | R |
| Balancer V2 | arbitrum, ethereum, polygon | B |
| Beethoven X | optimism | B |
| Blackhole V3 | avalanche | A |
| Camelot V2 | arbitrum | S |
| Camelot V3 | arbitrum | A |
| Curve | arbitrum, ethereum, optimism, polygon | Q |
| Curve TwoCrypto | ethereum | Q |
| DyorSwap | monad | R |
| Ekubo V3 | ethereum | X |
| Etherex | linea | C |
| Fluid | arbitrum, ethereum | X |
| FraxSwap | ethereum | T |
| Honeypop DEX | scroll | C |
| Hydrex | base | S |
| KyberSwap | bsc, ethereum, polygon | S |
| LFJ | arbitrum, avalanche | L |
| LFJ V2.1 | arbitrum, avalanche | L |
| LFJ V2.2 | arbitrum, avalanche | L |
| Lynex | linea | A |
| Maverick V2 | arbitrum, base, bsc, ethereum | X |
| Mdex | bsc | R |
| Merchant Moe Liquidity Book | mantle | L |
| Meteora DLMM | solana | M |
| Nile | linea | C |
| Nile V1 | linea | S |
| Nuri V2 | scroll | C |
| Orca Whirlpools | solana | O |
| PancakeSwap StableSwap | bsc | Q |
| PancakeSwap V2 | bsc, ethereum | R |
| PancakeSwap V3 | arbitrum, base, bsc, ethereum, linea, monad | C |
| Pancakeswap v3 | zksync | C |
| Pangolin | avalanche | R |
| Pharaoh | avalanche | S |
| Pharaoh V3 | avalanche | C |
| QuickSwap V2 | polygon | R |
| QuickSwap V3 | polygon | A |
| Ramses V2 | arbitrum | C |
| Raydium | solana | Y |
| Retro | polygon | C |
| ShibaSwap | ethereum | R |
| SushiSwap | arbitrum, base, ethereum, polygon | R |
| SushiSwap V3 | arbitrum, base, ethereum, optimism | C |
| Sushiswap v3 | scroll | C |
| SyncSwap | linea, zksync | S |
| SyncSwap V2.1 | linea | S |
| THENA FUSION | bsc | A |
| Thena V3 | bsc | A |
| Thruster v3 | blast | C |
| Uniswap V2 | arbitrum, base, ethereum, monad, optimism, polygon | R |
| Uniswap V3 | arbitrum, base, bsc, ethereum, monad, optimism, polygon | C |
| Uniswap V4 | arbitrum, base, bsc, ethereum, monad, polygon | U |
| Uniswap v3 | avalanche | C |
| Velodrome SlipStream | optimism | C |
| Velodrome V2 | optimism | S |
| iZiSwap | linea | X |
| zkSwap | zksync | R |

## 为什么不一律事件，或一律 168 次 view

估算比较两个候选，不能先承诺日志一定更便宜：

- 最小 view：每池每小时通常一个主要价格读取，额外元数据一次性缓存；可同块多池 batch/Multicall，但仍有执行、返回量和计费限制。
- 事件：窗口内事件返回量、分段范围、地址/topic 分组、初始化基线及时间戳成本。高频池可能返回大量 swap，168 次 slot0 反而更小。
- 快链若端点按 block 数给很小区间，七天分页次数可能很大；这时要换合适日志来源或最小历史状态路线，而不是硬扫。
- 多池同类事件可在地址数组/单例 poolId 上批量过滤，实际每批规模由端点承受能力决定，不是把所有池无限塞进一条请求。
- JSON-RPC batch 是传输优化，不能假设服务商按一个方法计费；例如 [Alchemy 计算单位说明](https://www.alchemy.com/docs/reference/compute-unit-costs)按方法累加 batch 成本。

可把成本抽象为：
view 成本 ≈ 池数 × 小时数 × 最小价格调用成本；
logs 成本 ≈ 区间分页 × 地址/topic 分组 × 每请求成本 + 事件数据量 + 基线 + 去重后的时间戳。
provider 计费、节点执行成本、网络延迟是三个不同指标，不能混成一个请求数。

## 数据正确性与共享工作

一个链只维护一份小时边界→实际 block 映射。按 timestamp 定位并验证边界；最近块平均速度只用于初始猜测，不能直接写成真实历史时间。事件块头按 block hash 去重；如果日志原生带 timestamp，可省相应头读取。

从事件恢复要有窗口起点的有效基线，覆盖全部会改变目标价格的状态路径，并按 block/transaction/log 顺序处理。没有交易不等于缺数据：对于已证明价格仅被这些事件改变的池，可以沿用之前的有效状态；请求失败、遗漏事件或时间相关价格则不允许这样填充。

选一致的历史确认口径，保存 source block/hash、actual timestamp、事件位置、价格定义和覆盖状态。新建不足七天的池不能制造历史；1d fallback 应是完整可证明的一天，不能把零散点标成完整。

168 个小时收盘点有 167 个相邻小时收益率；若要覆盖完整 168 个小时区间，需要 169 个边界价格。实现前需要固定这一统计约定，不混用 TWAP、成交均价和池 spot。

长期增量更新：先建立有证据的历史基线，之后只拉新增事件/必要新小时状态，不在每次刷新重读全部七天。独立 price 数据库有利于保存和复用这些状态，但拆库本身不会减少上游 RPC 成本。

## 项目代码已确认的问题（未修改）

位于 rpc/workers/lp-refresh/src/main.rs：

- evm_requests（当前约 1028 行）为价格历史复用整池刷新：普通 V3 读 slot0、liquidity、fee；Curve 分支读 fee、virtual_price 以及最多 8 coins + 8 balances。这不是最小价格读取。
- 历史调用点（当前约 3228 行）仍调用这个通用请求构造器。
- historical_blocks（当前约 3055 行）用最近 1000 块的平均速度反推小时块号，未验证选中历史块的实际 timestamp；链时间差异没有被正确建模。
- Weighted 分支只有 Vault/PoolId/weights/fee，没有对应历史 Vault balances；现有分支的存在不代表已获得有效 weighted price。

本轮结果：完整候选对应表与架构判断。没有新增 RPC 测试，没有修改采集代码，没有证明所有 108 组合的速度/覆盖率，也没有将原采集目标标记完成。

## Solana 的 Binance pair-vol 备选

如果目标是筛选 LP 风险，而不是证明每个池的 pool-local 可成交价格，可以把 Solana 的主路径改成外部市场 pair proxy：

```text
Binance 1h kline(token0/USD, token1/USD)
    -> token1_usd / token0_usd
    -> 168 个小时点
    -> ln(pair_t / pair_{t-1})
    -> sample standard deviation
```

直接有 Binance pair 时优先使用直接 pair；否则才用两个 token 的 USD 价格合成。这个指标必须标记为 `binance_pair_proxy`，不能继续叫 `pool-local volatility`。Binance 的公开 Spot API 支持 `exchangeInfo`、按 `startTime/endTime` 获取 Kline，且返回时间序列按时间顺序排列；公共行情不需要交易签名。[官方 Spot API 说明](https://developers.binance.com/en/docs/products/spot/rest-api)

对当前 3,228 个 Solana 池做的 symbol-only 上限统计：约 350 个可能有直接 Binance pair，约 475 个两边 token 都能在 Binance 找到 base asset。按目录类别看，`stable_quote` 的上限约为 332/1,194（直接 pair）和 380/1,194（两边资产），`major_pair` 约为 9/1,364 和 82/1,364。它只是上限，因为 token symbol 可能重名或错误，生产映射必须是 address→canonical asset 的显式映射，不能只按 symbol 猜。

因此推荐保留三层来源：

1. `pool_local`：已经存在的可靠池内历史；
2. `binance_pair_proxy`：没有池内历史但地址映射和 Binance 市场都明确；
3. `missing`：无法证明映射，不用 oracle 或伪造值。

### 已实现的市场历史回退

`rpc/tools/market-token-history.py` 按上述顺序写入独立的 `lp_price_history.db`：

- DefiLlama 先按小批次请求多个 Solana token 的稀疏 USD 历史，再在覆盖跨度足够时做小时 carry-forward；原始点来源为 `defillama`，carry 来源记为 `defillama_hourly_carry`；
- Binance 只使用明确地址或唯一目录 symbol 映射，按唯一 `(asset, market)` 拉取 1h Kline，再复用到同一资产的多个地址；来源记为 `binance_1h`；
- GeckoTerminal 选择目录中流动性和成交量最高的代表池，读取带 `token` 参数的 pool OHLCV；来源记为 `geckoterminal_pool_1h`，语义固定为 `external_market_pair_proxy`；
- 代表池无数据、请求失败或地址映射不可信时保留缺失，不用当前价格补历史；任务可重复运行，已覆盖的 7d 市场序列会跳过。

pair vol 先使用完整的 pool-local 小时序列；如果池内只有少量旧快照，而 token 市场序列明显更长，则选择 token USD 比值，并在 API 中返回 `token_price_history_market_proxy` 和 `external_market_pair_proxy`。这两个字段是防止外部市场价格被误读为池内可执行价格的必要 provenance。

GeckoTerminal 公共 API 当前不要求认证，但统一限速为 30 calls/min；付费 CoinGecko on-chain API 才适合提高批量配额。因此工作区中任何疑似密钥文件都不作为默认凭据使用，429 只记录为缺失/可重试状态。[官方认证说明](https://apiguide.geckoterminal.com/authentication) [官方限速说明](https://apiguide.geckoterminal.com/faq)

这条路径的成本按唯一 Binance asset/pair 计算，而不是按 3,228 个 pool 计算；它很适合先让筛选器恢复 1d/7d pair vol，但不能承诺所有长尾 Solana 池都有结果。
