# Solana Orca Whirlpool 一日波动率小范围实测

日期：2026-10-07。范围：单池、只读实验，不修改生产采集器、RPC 配置或 `lp_price_history.db`，不使用 oracle、TWAP 或外部 token 市场价。

## 结果

样本池：Orca Whirlpool `MPLX/USDC`

池地址：`78QBVQz3EnpuzmY42LRcCkn7gSyuFEMmQn1m5bY1yHgw`

数据源：Solana 原生 `getSignaturesForAddress` + `getTransaction`，Helius 免费 RPC 路由。交易确认口径为 `confirmed`；没有把这次结果宣称为 finalized 的永久历史快照。

窗口为 2026-10-06 13:49:28 UTC 至 2026-10-07 13:49:28 UTC，共 86,400 秒。取 25 个小时边界价格，得到 24 个一小时 log return。

| 项目 | 结果 |
| --- | ---: |
| 成功签名索引 | 4,630 条，6 页 |
| 获取交易详情 | 100 笔 |
| 已验证 Orca `Traded` 事件 | 92 条，92 个交易签名 |
| 价格点 | 25 |
| 收益率区间 | 24 |
| 一日小时 log-return 样本标准差 | 0.0064765293 |
| 一日小时波动率 | **0.6476529%** |

波动率定义为 24 个小时 log return 的样本标准差，没有年化。无交易的小时沿用此前最后一个已证明的池状态；不是把失败请求、当前价格或 oracle 值复制进去。

## 价格含义

每个事件使用 Orca Whirlpool `Traded` 事件中的 `post_sqrt_price`，按 Q64.64 解码：

`USDC per MPLX = (post_sqrt_price / 2^64)^2 × 10^(decimals_MPLX - decimals_USDC)`

本池两个 token 都是 6 decimals，所以没有额外小数位缩放。该价格是交易完成后的池状态 spot price，不是该笔交易的输入/输出金额比。

Orca 官方事件定义包含 `pre_sqrt_price`、`post_sqrt_price`、输入输出数量和费用字段：[Whirlpool events.rs](https://raw.githubusercontent.com/orca-so/whirlpools/main/programs/whirlpool/src/events.rs)。实验解析的是事件的池地址、方向和 `post_sqrt_price`，并按交易/日志顺序处理。

## 可靠性检查

- 每个采样边界只使用此前最后一个已验证的池事件；第一点的来源事件比采样边界早 12 秒。
- 所有 25 个价格点均为正数且有限。
- 交易失败项没有作为价格状态使用。
- 事件中包含的 Whirlpool 公钥与目标池地址一致。
- 事件数据长度为 121 字节，符合 Anchor 事件：8 字节 discriminator、32 字节池地址、方向、两个 `u128` sqrt price 和六个 `u64` 金额/费用字段。
- 交易详情只取小时边界附近的有限候选，不遍历整个池的所有历史交易。

## 发现的 Solana 数据路径差异

PublicNode 对该池的成功签名历史只覆盖到约 20 小时前，不能满足一日基线；Helius 能返回超过两天的成功签名，因此同一个协议×链必须把“事件格式”和“历史交易索引能力”分别配置。第一次 Helius 交易详情并发过高触发 429，降为单线程后完成，说明免费 RPC 的吞吐限制会决定取数调度。

实验期间发现现有 `/history/solana` 对包含 `getSignaturesForAddress` 的请求会因方法名包含 `sign` 被误判为写操作，最初因此使用了只读的 `/rpc/solana` 路由。现已将路由判定改为精确匹配写方法前缀，并加入 3 个单元测试；`getSignaturesForAddress` 现在会被视为可重试的只读方法。

## 复现

在仓库根目录并确保本地 RPC 代理监听 `127.0.0.1:8545`：

```bash
python3 -B rpc/tools/solana-orca-history-probe.py --provider helius-free
```

脚本只输出实验 JSON，不写数据库。它固定为一个池、一个一日窗口、有限分页和有限交易详情请求；遇到 RPC 限流会失败，不进行无限重试。

这证明了 Solana Orca Whirlpool 的一日事件路线可行，但不代表 Raydium、Meteora、其他 Orca 池或所有 Solana provider 都已完成验证。

## 追加的 Solana 家族检查

Raydium CLMM 的一个 `AVAX/USDC` 池也找到了可解码的 `SwapEvent`：事件长度 221 字节，包含池地址和交易后的 `sqrt_price_x64`，与 Raydium 官方事件定义一致：[Raydium CLMM pool events](https://github.com/raydium-io/raydium-clmm/blob/master/programs/amm/src/states/pool.rs)。但这个样本的一日事件价格几乎没有变化，和目录中的交易量/流动性指标不够一致，因此暂不把它写成生产波动率结果；需要再用独立的池状态或更活跃的样本交叉验证。

Raydium standard 最近交易没有发现同等的池价格事件，后续需要从交易 `postTokenBalances` 对已确认的两个 vault 余额做状态恢复。这个路径随后在一个 `WSOL/AU79` 池上跑通：

| 家族 | 样本池 | 详情交易 | 唯一状态交易 | 点数 | 小时收益 | 未年化波动率 |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Raydium standard | `CqmwcyB7fGor8z7As56yXBQzWjy7MnACtmj6wuLHgV5H` | 28 | 28 | 25 | 24 | **0.5592735% / 1d** |

每笔交易都必须在完整账户键（含 versioned transaction 的 loaded addresses）中找到两个已知 vault，并从 `postTokenBalances` 读取两个 raw amount；没有匹配 vault 的余额不会被当作池状态。这比按 token mint 猜用户账户安全得多。

Meteora DLMM 的价格是 active bin 价格，不能复用 sqrt-price 解码。Meteora 的官方 IDL 将 `Swap` 和 `Swap2Evt` 定义为包含 `lb_pair`、开始/结束 bin 的事件；实测它们通过 inner instruction 的 event-CPI 数据出现，而不是普通 `Program data` 日志。对应的官方 IDL：[Meteora DLMM IDL](https://raw.githubusercontent.com/MeteoraAg/dlmm-sdk/main/idls/dlmm.json)。两个只读验证如下：

| 样本池 | 窗口 | 详情交易 | 唯一 swap 交易 | 点数 | 未年化波动率 |
| --- | --- | ---: | ---: | ---: | ---: |
| `J27e5izvX4nbaaRDjMKv7DogQzcPidCAECxzE6rK4bF7`（JLP/USDC） | 1d | 122 | 95 | 25 | **0.3262361%** |
| `CCgY3zpCrhxEXHFjrde4bXrDnp6RJGDTjLXLbeQE2qXr`（MYRC/USDC） | 7d | 39 | 39 | 169 | **0.0545812%** |

7 天样本中没有交易的小时只沿用此前最后一个已证明的 `end_bin_id`，没有使用当前状态倒灌或外部 oracle。

## 取数成本结论

Solana 当前目录有 3,228 个池：Raydium CLMM 1,250、Raydium standard 1,093、Meteora DLMM 722、Orca Whirlpool 163。对每个池从最新签名逐页回翻并逐笔取交易详情，确实是错误的默认方案：一个高活跃 Meteora 池在 12 页签名中只有约 5,883 条成功记录，却只覆盖约 1.5 小时。

小范围 RPC 测试得到更好的调度方向：

- Helius 免费路由一次 JSON-RPC batch 取 8 笔 `getTransaction` 稳定完成；同一测试把 batch 放到 20 笔时返回 502，所以 batch 大小必须按 provider 配置，而不是盲目加并发。
- `getBlock(transactionDetails=accounts)` 在一个目标小时附近的 block 约 0.96 秒返回 1,625 笔交易及完整账户键；与目录交集命中 18 个池、49 笔交易。它适合先做共享的小时边界/候选池索引，命中后再取 full transaction 解事件；Raydium standard 甚至可以直接使用 block 返回的 `postTokenBalances`。
- 因此正式实现应采用“按链建立边界 block 映射 + block accounts 快速命中 + 协议最小解码 + 对未命中池按 provider 能力回退”的分层路径。高活跃池不能再用每池顺序翻签名作为主路径。

以上实验仍然只读，没有写入 `lp_price_history.db`，也没有声称 3,228 个池已完成历史回填。实验探针位于 [`solana-orca-history-probe.py`](../../rpc/tools/solana-orca-history-probe.py)。
