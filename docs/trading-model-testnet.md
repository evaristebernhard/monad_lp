# 交易模型的测试网部署

本次固定 Monad 测试网，链 ID 为 10143。
部署对象是 Adilbai PPO actor 的 int16/Q16 版本。
所有 actor 权重、偏置和 Tanh 表都在链上。
推理在 Solidity 合约中完成，不调用外部 AI API。

部署和验证已完成，记录状态为 `verified`。
模型地址：[0x3f63f7586c696b5d9ff52774b4895c51bc280906](https://testnet.monadscan.com/address/0x3f63f7586c696b5d9ff52774b4895c51bc280906)。
决策记录器：`0x6903f12a39135374f9ced70178466d2dba3e7794`。
六组链上输出与本地整数输出一致。
持有、买入和卖出各有一笔成功的决策交易。

| 项目 | 实付测试 MON |
| --- | ---: |
| 工厂、全部数据块、actor 和记录器的部署 | 9.9668582 |
| 持有决策交易 | 1.7929982 |
| 买入决策交易 | 1.8049202 |
| 卖出决策交易 | 1.8050522 |
| 合计 | 15.3698288 |

本次有效 gas 单价为 100 MON-gwei。
总费用与钱包余额减少量一致。
本次模型不适合未经压缩的频繁交易。
后续可比较缩小输入、模型压缩和 Neurom 蒸馏。
这些方法会改变模型，必须重新验证收益和决策。

这不是 Neurom 阈值网络格式。
部署流程复用 Neurom 的代码分块方法。
原模型结构为 3008 → 64 → 64 → 2。
原模型用于股票，尚未验证 MON 交易收益。

## 合约

- `ModelChunkFactory` 部署内容寻址的权重块。
- 四个权重块共存储 393,992 字节。
- 一个表块存储 8,196 字节的 Tanh 查表数据。
- `ExperimentalPpoActor` 校验数据哈希并计算决策。
- `PpoDecisionRecorder` 调用 actor 并记录决策事件。

代码块以 STOP 开头，数据无法通过调用改写。
actor 没有权重更新接口。
recorder 没有兑换或资金托管功能。
输入由调用者提供，不认证行情真实性。
本次输入是六组固定数值样例，不是真实行情。

## 成本和验证

部署记录位于 `research/ai/model-study/testnet-deployment.json`。
只有记录状态为 `verified` 才表示全部验证完成。
记录包含每笔交易哈希、gas、费用和代码哈希。
记录还包含六组本地与链上输出的比较。
持有、买入和卖出各发送一笔实际推理交易。

Monad 按 gas limit 计费。
脚本分别记录 gasEstimate、gasLimit 和收据 gasUsed。
`chargedFeeWei` 按 gasLimit × effectiveGasPrice 计算。
钱包余额变化单独记录，便于核对总费用。
余额变化可能包含同一期间的其他钱包操作。

只读 `eth_call` 不支付链上手续费。
它仍占用 RPC 计算资源，受服务商限制。
自动交易需要发交易，模型推理会增加交易费用。
本次不把 AI API token 当作链上手续费。

## 复现

本地设置 `TESTNET_DEPLOYER_PRIVATE_KEY`。
脚本也兼容项目已有的本地部署钱包配置。
不要将私钥写入部署记录或命令行参数。

先编译 `research/ai/evm` 中的 Solidity 合约。
查看只读预检：

```sh
node research/ai/deploy_testnet.mjs
```

部署并验证：

```sh
node research/ai/deploy_testnet.mjs --deploy
```

脚本只接受链 10143，总费用上限为 20 测试 MON。
脚本在广播前保存交易哈希，重跑复用该哈希。
如广播中断且哈希长期查不到，先检查钱包 nonce。
不要删除记录后重复部署。

本地测试覆盖完整 actor 和真实大块创建。
已有 actor 测试覆盖动作、仓位和无效输入。
大块测试实际创建 112 KiB 数据块，不使用 `vm.etch`。

## 后续范围

链上模型不自动产生行情，也不自动触发交易。
还需要行情输入协议、触发服务和兑换执行器。
这些组件需要单独验证，然后才能运行交易策略。
完整背景见 `docs/pretrained-model-onchain-study.md`。
