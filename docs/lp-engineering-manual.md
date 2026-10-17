# LP 池子数据与 RPC 解码工程手册

## 1. 这套系统解决什么问题

LP 数据不是“读取一个池子余额”这么简单。当前系统面对的是：

- 14 条链、65 个协议、40,631 个池子；
- 同一个协议名下也可能同时存在 V2、V3、稳定池、LB、Weighted、Singleton 等不同合约；
- 同一类 AMM 在不同协议里可能改了函数名、返回值布局或存储位置；
- RPC 的免费层有速率、批量大小和计算单元限制，瞬时 429/502 不能直接等同于池子坏了。

因此系统采用三层结构：

```text
前端 -> 后端 API / 刷新任务 -> 本地 RPC 层 -> 上游免费/付费 RPC
                                      |
                                      +-> SQLite: chain -> endpoint -> benchmark
```

RPC 层是独立服务，后端只访问本地 RPC；池子状态写入 `data/lp_screener.db` 的 `pool_rpc_state`。RPC 层的运行目录是 `rpc/`，运行时数据库是 `rpc/data/rpc.db`。

## 2. 当前数据基线

以下结果来自本地 SQLite，而不是手工估算：

```sql
SELECT state_status, COUNT(*) AS n
FROM pool_rpc_state
GROUP BY state_status
ORDER BY n DESC;
```

当前结果为：

| 状态 | 数量 | 含义 |
|---|---:|---|
| `live` | 37,398 | 解码器拿到了该协议所需的核心状态 |
| `account_present` | 3,233 | 账户/合约存在，但不一定已经完成池子状态解码；主要是 3,232 个 Solana 账户，加 1 个待校正的 Velodrome 索引地址 |
| `rpc_error` | 0 | 当前没有遗留的协议状态错误 |
| `transport_error` | 0 | 当前没有被 RPC 网络层阻断的池子 |
| `unsupported_protocol` | 0 | 当前没有进入未实现协议分支的池子 |

池子数量最多的链是 BSC（25,340），其次是 Polygon（3,491）、Solana（3,232）、Base（1,700）、Ethereum（1,684）和 Arbitrum（1,603）。这并不表示 BSC 一定更重要，而是 BSC 上存在大量 PancakeSwap V2、路由分叉和长尾池。

主要池子族：

| 池子族 | 数量 | 典型解码方式 |
|---|---:|---|
| Constant Product V2 | 20,750 | `token0/token1/getReserves` |
| Concentrated V3/Algebra | 9,425 | `slot0/liquidity/fee` 或 `globalState/liquidity` |
| Uniswap V4 | 5,855 | 通过链上的 singleton state view 读取 pool key 对应状态 |
| Solana CLMM/Standard/DLMM/Whirlpool | 3,232 | 账户 owner 与账户数据，不使用 EVM selector |
| Liquidity Book | 529 | reserves、active bin、bin step |
| Solidly V2 | 383 | Pair reserves，并保留 stable/volatile 语义 |
| StableSwap | 217 | Curve/Pancake StableSwap 的 coins、balances、虚拟价格 |
| Weighted | 189 | Balancer Vault、pool id、权重或稳定池公共字段 |

## 3. 为什么必须维护这么多协议和池子

### 3.1 链是流动性的物理分区

USDC/USDT 在 Ethereum、Arbitrum、Polygon、Base 上不是同一个池子，也不是同一个可结算余额。跨链桥资产地址不同，交易顺序不同，Gas 和最终性不同。后端即使使用同一个路由算法，也必须按链分别读取和报价。

### 3.2 一个交易对会被多个 AMM 分割

同一条链上，USDC/WETH 可能同时存在：

- Pancake/Uniswap V2 的宽范围恒定乘积池；
- Uniswap V3、Algebra、Ramses、Kyber Elastic、iZiSwap 的集中流动性池；
- Curve/Pancake StableSwap 的稳定曲线池；
- Trader Joe/LFJ、Merchant Moe 的 Liquidity Book；
- Balancer/Beethoven X 的多资产 Weighted 或 Stable 池；
- Uniswap V4/Ekubo 这类 singleton 或特殊存储池。

这些池的价格曲线、费率、流动性定义和可执行报价都不同。只保留一个“主池”会漏掉价格更好、深度更大的路由，也会漏掉套利机会。

### 3.3 长尾池有研究价值，但不应和生产池混用

建议把池子分成三层：

1. **生产候选池**：有可验证状态、足够 TVL/成交量、代币地址可信。
2. **研究池**：协议已知，但流动性小、状态尚未完整或需要专用解码器。
3. **观察池**：合约存在，但地址、资产、流动性或交易量质量不足。

数据库保留长尾池是为了可追溯和发现新流动性；执行层不应自动把所有池都纳入报价或下单。

## 4. AMM 类型和 LP 收益来源

### 4.1 恒定乘积池：`x * y = k`

V2/传统 Solidly volatile 池的核心关系近似为：

```text
(x + dx * (1 - fee)) * (y - dy) = x * y
```

LP 收益来自交易费。优点是流动性覆盖整个价格区间，维护简单；缺点是资本效率低，深度越浅，价格影响越大。

Solidly 的 stable 池不能简单当成普通 V2。它对接近锚定价格的资产使用不同曲线，因此本系统把 stable/volatile 语义保留在协议和池类型中，而不是只看 `getReserves()`。

参考：[Velodrome Pool 合约](https://github.com/velodrome-finance/contracts/blob/main/contracts/Pool.sol)、[THENA Pair 合约](https://github.com/ThenafiBNB/THENA-Contracts/blob/main/contracts/Pair.sol)。

### 4.2 集中流动性：V3、Algebra、Ramses、Kyber Elastic、iZiSwap

LP 只在价格区间内提供流动性。区间内资本效率高，区间外几乎不产生该方向的手续费；LP 还承担价格穿越区间、再平衡和无常损失风险。

这也是为什么不能把所有“V3”池都调用 Uniswap V3 的 `slot0()`：

- Algebra/Lynex/THENA Fusion 使用 `globalState()`；
- Kyber Elastic 使用 `getPoolState()` 和 `getLiquidityState()`；
- iZiSwap 使用自有的 `state()` 布局；
- Ramses V2 的实际部署暴露集中流动性状态，而不是 Solidly 的 `getReserves()`。

参考：[Uniswap V3 文档](https://docs.uniswap.org/concepts/protocol/concentrated-liquidity)、[Algebra Pool State API](https://docs-v1.algebra.finance/en/docs/contracts/API-reference-v1.9/core/IAlgebraPoolState/)、[KyberSwap Elastic 说明](https://support.kyberswap.com/hc/en-us/articles/13785791497113-What-is-KyberSwap-Elastic)、[iZiSwap Core](https://github.com/izumiFinance/iZiSwap-core)、[Ramses 合约地址](https://docs.ramses.exchange/pages/contract-addresses)。

### 4.3 Liquidity Book：bin 而不是 tick

Liquidity Book 把价格离散成 bin，状态至少需要 reserves、active bin 和 bin step。LP 收益依赖资金是否落在当前活跃 bin 附近；价格跳出资金所在 bin 后，收益特征会改变。

LFJ V2.1/V2.2 和 Merchant Moe 的池使用这一类状态 API，但某些历史索引标签不准确，因此必须以合约实际 selector 为准。

### 4.4 StableSwap

StableSwap 对相关资产在锚定区间提供更低滑点，适合稳定币和相关资产。其收益通常来自较高周转，而不是极高单笔费率。Curve 池需要读取 `coins()`、`balances()`、费率和虚拟价格；Pancake StableSwap 也不能套用普通 V2 公式。

参考：[Curve Registry 文档](https://curve.readthedocs.io/registry-pools.html)、[PancakeSwap StableSwap 文档](https://docs.pancakeswap.finance/products/pancakeswap-exchange/stableswap)。

### 4.5 Weighted 与多资产池

Balancer/Beethoven X 可以使用 2 个以上资产和非 50/50 权重。WeightedPool 读取 normalized weights；Stable/ComposableStable 池没有 normalized weights，不能因为该调用 revert 就判定 RPC 错误。本系统现在把 Vault、PoolId 作为公共状态；权重存在时再记录权重。

参考：[Balancer SDK 文档](https://docs.balancer.fi/developer-reference/sdk/)、[Balancer V2 Vault](https://docs.balancer.fi/concepts/vault/)。

### 4.6 Singleton 与特殊存储

Uniswap V4 把多个池放进 singleton，需要通过 PoolId 查询，而不是把 PoolId 当普通 EVM 地址调用。Ekubo V3 的状态也需要通过 Core 的存储视图读取。此类池必须专门维护 state view 和编码规则。

## 5. 为什么有些 LP 池手续费特别高

池的名义费率高，通常是对风险和流动性成本的补偿，不等于 LP 一定赚钱。常见原因：

1. **资产波动大**：MEME、长尾 token 或低流动性资产的 adverse selection 风险高。
2. **池子很浅**：单笔交易会造成更大价格移动，协议用高费率吸引 LP 承担库存风险。
3. **集中区间窄**：资金效率高，但更容易出区间；高费率是对再平衡和失效时间的补偿。
4. **交易流量有毒**：套利机器人或知情交易者会在价格更新前交易，LP 收到的手续费可能抵不过库存损失。
5. **动态费率**：部分协议根据波动、bin、tick 或插件配置调整费率，数据库中的某个时点费率不能代表长期实现费率。
6. **激励与手续费混在一起**：激励 APR、协议补贴和真实 swap fee 必须分开计算。

LP 的第一近似收入不是“费率越高越好”，而是：

```text
fee income ≈ trading volume × fee rate × LP fee share
net LP return ≈ fee income + incentives - impermanent loss - rebalance/gas cost
```

实际评估应至少同时看：

- 7d/30d volume，而不是只看瞬时 24h volume；
- active liquidity，而不是只看总 TVL；
- 价格是否长期停留在 LP 区间；
- 交易量/TVL 的稳定性；
- 资产相关性和价格跳跃；
- 协议费、激励解锁和退出成本。

因此本项目的 `gross_fee_apr_estimate` 只能作为筛选信号，不能直接作为投资收益承诺。

## 6. RPC 错误的分类规则

刷新器现在把问题拆成四类：

| 类别 | 例子 | 处理方式 |
|---|---|---|
| `transport_error` | 403、429、502、超时、连接失败 | 换免费 endpoint、重试、降低 batch/RPS；不能判成池子坏了 |
| `rpc_error` | eth_call 执行 revert、返回字段不完整 | 检查协议分类、selector、返回值布局 |
| `unsupported_protocol` | 没有安全解码器 | 先进入研究队列，不发送猜测性的 V2/V3 调用 |
| `account_present` | Solana 账户存在，或合约存在但池状态接口不匹配 | 记录地址质量问题，不能直接用于报价 |

本次 1,046 个历史 `rpc_error` 的主要根因是：

- 大批 Polygon 错误来自免费 RPC 的 429/计算单元突发，而不是池子本身；
- THENA Fusion 被错误当成 Liquidity Book，实际是 Algebra 风格集中池；
- KyberSwap 普通标签对应 Classic `getReserves()`，不能统一当 Elastic V3；
- Balancer Stable/ComposableStable 没有 `getNormalizedWeights()`；
- LFJ/Merchant Moe 的 token 发现与 LB 状态需要兼容历史变体；
- 一个 Velodrome 记录实际指向 SinkConverter，属于索引地址质量问题。

## 7. RPC 层运行原则

默认选择顺序：

```text
free -> priority -> 当前 in-flight -> paid fallback
```

付费 RPC 仍保留在数据库中，但 priority 必须低于免费 RPC 的选择顺序；只有免费端点饱和或失败重试后才使用付费端点。每条链的压力测试应记录到 `rpc_benchmarks`，不能只看 provider 的宣传 RPS。

常用查询：

```bash
# 查看某条链所有 RPC
sqlite3 -header -column rpc/data/rpc.db \
  "SELECT chain_name, title, provider, tier, priority, enabled
   FROM chain_rpc_catalog
   WHERE chain_name='polygon-mainnet'
   ORDER BY tier, priority;"

# 查看当前池子状态
sqlite3 -header -column data/lp_screener.db \
  "SELECT state_status, COUNT(*)
   FROM pool_rpc_state GROUP BY state_status;"

# 查看某协议剩余问题
sqlite3 -header -column data/lp_screener.db \
  "SELECT p.chain,p.protocol,s.state_status,COUNT(*)
   FROM pools p JOIN pool_rpc_state s ON s.pool_address=p.pool_address
   WHERE s.state_status <> 'live'
   GROUP BY p.chain,p.protocol,s.state_status;"
```

## 8. 每次优化的测试流程

1. 先选 10～50 个代表池：高 TVL、长尾、不同链、不同协议各占一部分。
2. 只跑目标状态，例如 `--state-status rpc_error`，不要每次全量刷新。
3. 先验证 RPC transport，再验证 selector 和 ABI，再验证状态完整性。
4. 记录修复前后请求数、batch size、RPS、p95、错误分类和写入数量。
5. 小样本通过后，再按链扩展；最后才做全量刷新。

这样可以把“RPC 压力问题”“解码器问题”“索引数据问题”分开，避免一次全量运行后只看到一个巨大的错误数字。

## 9. Token 价格历史、均价和波动率

USD 锚定价格按 token 维护，便于复用外部价格源；但价格观测按 pool 维护。价格历史单独存放在 `data/lp_price_history.db`，由 API 以 `price` schema attach；catalog/current RPC state 仍存放在 `data/lp_screener.db`。`pool_price_history` 以 `(chain, pool_address, timestamp)` 唯一，每小时写入该池自己的相对价格和 token0/token1 USD 快照，最多保留 30 天。没有可执行价格的 pool 也写入 `price_status='unavailable'` 和原因，但 `token1_per_token0` 保持 NULL，绝不伪造价格。

`token_volatility` 保存 token 的 1d/3d/7d/30d 观测均价和波动率。pool 的 1d/3d 波动率只使用该 pool 自己的 `pool_price_history` 序列计算：

```text
P_A/B(t) = P_A(t) / P_B(t)
r_t = ln(P_A/B(t) / P_A/B(t-1))
vol = annualize(stdev(r_t))
```

只有某个 pool 自己的原始价格历史不足时才需要触发后台价格补齐；前端读取池列表不能同步对大量 token 或 pool fan-out RPC。RPC 返回 token 锚点和 pool 历史，backend 负责均价、pair price 和波动率，前端只展示结果。
