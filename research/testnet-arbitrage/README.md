# Monad 测试网池价与测试币研究

实查时间：2026-10-17 14:18–14:21（北京时间）。仅链 **10143**，未查询主网 RPC，未签名、未提交交易、未领取代币。

结论：**本次覆盖内没有确认可执行的套利；补充测试代币优先使用水龙头。** 不能把同名 demo USDC、Circle USDC 或 demo WMON、可赎回 WMON 视作相同资产。

## 现有余额

读取项目已有部署记录中的公开地址 `0x41d5D3B5f2e497d1F4bCF767ffac9DDAb85D2b79`，未读取私钥。区块 71747921：

| 资产 | 余额 |
|---|---:|
| 原生测试 MON | 33.86423694 |
| Circle 测试 USDC | 0 |
| 官方清单 WETH / WMON | 均为 0 |
| FableSwap demo USDC / WMON | 均为 0 |

此地址仅为项目已有测试部署钱包，不代表用户其他钱包的余额。

## 池价与可交易性

| 对象 | 实查价格 | 状态 |
|---|---|---|
| FableSwap demo USDC / WMON | 1 demo WMON ≈ 1.996011974 demo USDC | 储备约 19,980.07986 demo USDC、10,010 demo WMON；手续费 0.30% |
| Monad 维护的 V4 示例 MHTA / MHTB | 1 MHTA ≈ 0.999999999999 MHTB | 当前活跃 liquidity=0，不可将槽位价格当作可成交报价 |
| 旧 Uniswap V2 / V3 工厂 | 无报价 | 对已核对的旧地址 eth_getCode 返回空 |
| 旧 Pancake 候选、DODO 配置 V2 / V3 工厂 | 无报价 | 对这些候选地址 eth_getCode 返回空 |

FableSwap 池：`0x9d237bf8fb87cdea6a88fc08e0dafc04335a7528`。这是自定义常数乘积 demo DEX，合约 `getReserves()` 仅返回两个值，已按其源码单独读取。两种代币均为 18 位小数。demo WMON 是可公开 mint 的测试 ERC20，不能据其名称认定可以赎回原生 MON。

V4 PoolManager：`0x451d64ab3b650040d2ae1886602b97ed6edc643d`；池 ID：`0x035701c97330e08dcbfba844d0f85471da9bc4739b28afae2f059025f25df976`。此部署由 Monad 维护，[清单明确注明并非 Uniswap 官方部署](https://github.com/monad-developers/uniswap-v4-hooks-example/blob/c3cc78a88f10e5ba141cd6fb632f31db83c0daf8/shared-infrastructure.json)。

## 套利测算

FableSwap 已按工厂枚举确认 1 个池，没有发现同一代币地址组合的第二个交易场所。V4 示例使用另外两种代币，无法与它直接闭环。

从 FableSwap 储备按源码常数乘积及 0.30% 手续费计算，以下是未广播的本地报价，未模拟税费等非标准代币行为。往返第二腿使用第一腿成交后的储备：

| 投入 demo WMON | 第一腿得到 demo USDC | 同池换回 demo WMON |
|---:|---:|---:|
| 1 | 1.989826 | 0.994010 |
| 10 | 19.880438 | 9.940149 |
| 100 | 197.039869 | 99.406791 |
| 1,000 | 1,809.770112 | 994.550176 |

同池往返在 gas 前已亏损。实查 gas price 为 102 gwei，假设交易总计 300,000 gas，费用为 **0.0306 原生测试 MON**；300,000 仅为说明成本的假设，未进行具体套利交易的 estimateGas。不能用 demo WMON 的数量直接抵扣原生 MON 的费用。

## 获取测试币

1. **原生测试 MON**：[Monad 官方水龙头](https://faucet.monad.xyz/)。原生 MON 才能付 gas。现有项目钱包已经有上述余额。
2. **Circle 测试 USDC**：[Circle 水龙头](https://faucet.circle.com/)，选择 Monad Testnet 并填收款地址。页面公布每地址、每条链每 2 小时可领 20 USDC；本次未实际提交领取。[Circle 官方地址清单](https://developers.circle.com/stablecoins/usdc-contract-addresses)确认合约为 `0x534b2f3A21130d7a60830c2Df862319e593943A3`，6 位小数。
3. **demo 交易练习币**：[FableSwap](https://fableswap.vercel.app/) 的 faucet；[代币源码](https://github.com/portdeveloper/fableswap/blob/main/contracts/src/MockERC20.sol)中 `faucet()` 每次给调用者 1,000 个币，`mint(address,uint256)` 亦公开。仅适合其 demo 池和合约流程测试，无法由此获得原生 gas 币。

   demo USDC：`0x0AC0B00Cf3E7aB4B287a99c907C93f74bd73e737`；demo WMON：`0x6d427a189325473725c89Ed13Cb734cFAcbD75A6`。[部署配置来源](https://github.com/portdeveloper/fableswap/blob/main/web/lib/deployments.ts)。

## 覆盖范围和复查

本次不是全链扫描。核对 6 个工厂候选；代币候选包括官方当前清单、早期测试网地址、Uniswap 示例地址及 FableSwap demo 地址。旧地址无代码只能说明这些地址当前不可用，不能推断整条测试网没有其他 DEX。

公共 RPC 将 eth_getLogs 限制为每次 100 区块。V4 实际完成区间为官方示例初始化附近 **62279000–62279999** 和快照末尾 **71747145–71748144**，共 2,000 个区块；不覆盖两段之间的其他池创建。备用 RPC 的大区间查询/历史状态也受限，未以失败查询推断池不存在。FableSwap 交叉 getPair 查询出现 3 个 RPC 错误，但 allPairs 枚举成功且总池数为 1，该池身份与储备读取成功。

原始证据：`snapshot.json`（71747921）、`v4-snapshot.json`（71748144）、`analysis.json`。各池状态固定在各自快照区块，两个快照不混为同区块套利报价。

```bash
node research/testnet-arbitrage/scan.mjs
node research/testnet-arbitrage/v4.mjs
node research/testnet-arbitrage/analyze.mjs
```

这些脚本只读；均检查 chainId==10143。后续发现同一地址资产的第二个有效池，再开展同区块双腿报价、gas 估计及原子交易模拟。现有证据不足以通过套利增加原生测试 MON。
