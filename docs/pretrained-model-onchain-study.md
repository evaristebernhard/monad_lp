# 预训练交易模型上链研究

研究日期：2026-10-17。第一版固定 Monad 上已有的 Uniswap V4 MON/USDC 池。
本次没有部署到公共链，没有交易真实资产，没有改交易网页。

## 结论

可以保留这份 PPO 检查点的 actor 结构和已训练权重，制作定点推理版本。
无需为证明部署能力先训练另一个模型，也无需把 critic 和优化器部署到链上。

已经完成权重拆解、PyTorch 核对、整数参考实现、Solidity 研究原型和本地测试。
原型在标准 EVM 中运行成功。Monad 定价下的部署、推理和完整兑换仍需验证。
不能把本地 EVM 实验写成“已在 Monad 上部署”。

推荐先走“保留原网络的 int16 定点 actor”，把 Neurom 蒸馏路线作为备选。
如果 Monad 上实测无法满足完整交易的 gas 预算，再评估结构压缩或蒸馏。
压缩、蒸馏和原网络量化是不同实验，不能混用模型身份。

## 1. 原始模型的实际结构

来源：https://huggingface.co/Adilbai/stock-trading-rl-agent
固定提交：a317fec1939eda44d04088b47b48ca3ee158bc1f。
检查点 final_model.zip 的 SHA-256：
d714969844c671a04cecbe64c428a23919dbcae35bfcc3078e32d0f0b8fdf366。

检查点记录 501,760 个训练步，Stable-Baselines3 版本为 2.1.0。
权重文件的张量形状与下面的结构一致：

    3008 输入 → Linear(64) → Tanh → Linear(64) → Tanh → Linear(2)

| 项目 | 核对结果 |
| --- | --- |
| 完整检查点参数 | 393,669 |
| 部署所需 actor 参数 | 196,866 |
| 每次 actor 推理的乘加项 | 196,736 |
| actor 浮点参数 | 787,464 字节 |
| int16 权重＋int32 偏置 | 393,992 字节 |
| Q16 Tanh 正半轴查表 | 8,196 字节 |
| 确定性推理不需要 | critic、优化器、log_std |

检查点 policy_kwargs 为空。SB3 2.1.0 的默认 ActorCriticPolicy 使用 Tanh。
use_sde 为 false。确定性预测取高斯分布的均值，不需要链上随机采样。
源码依据：
https://github.com/DLR-RM/stable-baselines3/blob/v2.1.0/stable_baselines3/common/policies.py

## 2. 输入不能用当前价格直接代替

3,008 个输入由两部分组成：

- 60 个时间点，每个时间点 50 个按原顺序排列并标准化的行情特征。
- 8 个归一化账户特征，包括现金、持仓价值、净值、回报、交易次数、费用、回撤和回报波动率。

原模型使用日频股票数据。特征包括 OHLCV、SMA、EMA、RSI、MACD、布林带、
价格变化、成交量比以及滞后项。已从 scaler.pkl 提取 50 个特征的精确顺序、
均值和尺度，保存到 research/ai/model-study/scaler.json。

本地 MON/USDC 最近 120 小时价格连续完整，但这仍是小时池内价格比。
它不能代替 60 天的股票 OHLCV，也不能凭空提供所有技术指标。
当前网页提供的成交量单位是 USD。原模型的股票 Volume 单位不同，必须明确适配。

第一版必须写一个版本化的 MON/USDC 输入适配器，明确：

1. 使用日线还是把时间点改为小时线。
2. 每个特征的算法、排列、单位和标准化方式。
3. 使用已完成的观察窗口，拒绝过期、断档和非有限数值。
4. 使用策略账户的真实 MON/USDC 余额。
5. 把账户历史、原模型模拟账户和实际链上状态的差异记录下来。

更换时间频率或标准化规则会改变输入分布。即使权重不变，也不能声称保留原股票策略的交易含义。

上游环境还有一个具体问题：_extract_prices() 从 states 的 Close 列读取价格，
而 dataprocessor.py 在生成 states 前已经标准化该列。
不能直接复制这段代码来给 Monad 持仓估值。实际估值必须使用未标准化的池价格。
这影响原环境语义，不影响本次对权重矩阵本身的推理核对。

## 3. 动作协议

网络输出两个连续数值，原环境的解码是：

    action = int(clip(raw_action[0], 0, 2))
    position_fraction = clip(raw_action[1], 0, 1)

| 第一维的原始值 | 动作 |
| --- | --- |
| 小于 1 | HOLD |
| 大于等于 1 且小于 2 | BUY |
| 大于等于 2 | SELL |

第二维决定仓位比例。它不是模型置信度。
不能把输出改成三分类 argmax，也不能把截断改成四舍五入。

MON/USDC 执行映射：

- BUY：用策略账户的 USDC 买 MON。
- SELL：用策略账户的 MON 买 USDC。
- HOLD：记录决策，不调用 DEX。
- 仓位比例为 0 或余额不足：保留原动作，另行记录未执行原因。

## 4. 为什么不能直接使用现有 Neurom 引擎

本地 Neurom 的 ModuleFactory.MAX_PORT 为 256，原模型有 3,008 个输入。
EngineBase._input() 要求输入字节为 0 或 1。
DensePolicyEngine 的普通层使用阈值或 LIF 激活，不是 Tanh。
这些是计算语义差异，不是换文件格式就能解决的问题。

可复用 Neurom 的权重分块思路、内容哈希与本地/链上核对方法。
不直接把原模型送入 Neurom 当前的 deploy 接口。
若长期统一到 Neurom，需要设计新的引擎协议及部署入口，不能随意放宽原引擎的限制。

## 5. 已完成的数值验证

原始张量用 PyTorch weights_only 模式在 CPU 上读取，没有运行模型仓库的 Python 代码。
另有标准库读取器，只允许检查点使用的张量重建元数据和 scaler 类型。
没有调用 PPO.load() 来反序列化检查点内的可执行对象。

64 组输入是固定种子的归一化数值测试，包含零输入、初始账户、
正负输入和不同幅度的扰动。它们不是 MON 行情或交易建议。

| 检查 | 结果 |
| --- | --- |
| 原始 float32 权重的参考计算与 PyTorch | 动作 64/64 一致 |
| 浮点参考与 PyTorch 的最大输出误差 | 约 0.00000208 |
| int8 与 PyTorch | 1/64 动作不同 |
| int16 与 PyTorch | 0/64 动作不同 |
| int16 最大原始输出误差 | 约 0.001107 |
| int16 最大仓位比例误差（对浮点参考） | 约 0.0001545 |

int16 原型使用每层的二次幂权重尺度、Q16 偏置与输入。
Tanh 使用 [-8,8] 饱和范围、间隔 1/256 的查表和整数线性插值。
输入 [-8,8] 是原型新增的前置条件，越界拒绝，不是原始模型的输入限制。
查表和整数化会引入误差，有限测试不能证明对所有输入都保持动作一致。
尤其原始动作接近 1 或 2 时，需要额外测试决策边界。

## 6. Solidity 原型与 gas

代码：research/ai/evm/src/ExperimentalPpoActor.sol。
编译配置：Solidity 0.8.28，viaIR，optimizer_runs=1000。
测试后端：Foundry 1.6.0-rc1 的标准 Ethereum Cancun EVM，不是 Monad 定价。

8 项测试通过：

- 零输入、初始账户、正输入、负输入、买入和卖出共 6 个数值样例。
- 3,008 维输入长度检查。
- 超出归一化输入范围的拒绝检查。

六个样例的 Solidity 原始整数输出、解码动作及仓位均与 Python 整数参考完全一致。
测试中的权重合约由 vm.etch 安装，没有测量真实权重部署流程。

| 本地 EVM 实验 | 单次 predict 调用 gas |
| --- | --- |
| 初版循环 | 约 2,575 万～2,579 万 |
| 每次循环处理四个权重 | 约 1,719 万～1,723 万 |

这里的 gas 是测试中外部 predict 调用的 gasleft 差值。
它不包括外层交易固有 gas、完整策略执行器或兑换费用。
独立交易的冷访问、calldata 和 Monad 特有计价均需要复测。
本次只在研究用循环做优化，没有更改既有交易代码。

Monad 官方文档当前列出单笔交易 gas 上限 30M，合约代码上限 128KB。
Monad 按交易 gas limit 收费。上述 Ethereum EVM 数字不能直接作为 Monad 报价。
来源：
https://docs.monad.xyz/developer-essentials/gas-pricing
https://docs.monad.xyz/developer-essentials/differences
https://docs.monad.xyz/developer-essentials/opcode-pricing

## 7. 实际部署方式

    内容寻址的权重块（4 块，每块最多 112 KiB）
        ＋ 独立 Tanh 表
        ↓
    PpoFixedPointPolicy：读取固定权重，计算原始连续动作与仓位
        ↓
    MonUsdcStrategy：检查输入与资金限制，映射买卖方向
        ↓
    已固定的 Uniswap V4 MON/USDC 路由与池

393,992 字节的权重和偏置不能放进一个 128KB 合约。
按 Neurom 现有的 112KiB 分块大小，需要 4 个权重块和 1 个 Tanh 表块。
创建代码块的流程可参考 ModuleFactory.createChunk 的小 initcode＋回调方法。
部署拆成多笔交易，每块分别测量 gas，不使用把全部参数写入普通 storage 的方式。

模型合约绑定权重哈希、Tanh 表哈希、输入协议版本和原始检查点哈希。
keeper 只能提交指定窗口的数据并触发，不能提交买卖方向。
第一版如果数据由签名提交者提供，要明确这个信任假设。
模型在链上计算，不等于行情来源已经无需信任。
后续逐步用固定池的链上采样记录替代外部特征提交。

MonUsdcStrategy 只能交易指定池。它独立限制资金、滑点、最小成交量和有效期。
模型推理、账户状态更新和兑换放在同一笔交易里。
DEX 兑换失败则整笔回滚。模型动作与资金限制拒绝原因分别记录。
事件保留模型身份、输入哈希、时间窗口、决策编号、原始输出、动作和兑换结果。
输入内容保留在可读取记录或交易 calldata 中，哈希本身不能用来重算输入。

## 8. 下一步与验收门槛

1. 完成 MON/USDC 特征适配，得到真实输入，不以虚构成交量补齐特征。
2. 在冻结的真实数据窗口和边界输入上扩展 PyTorch/整数误差测试。
3. 在支持 Monad 计价的本地节点或 Monad 测试网上测部署与推理 gas。
4. 在 Monad 主网 fork 接现有真实 V4 池，验证买、卖、持有与回滚。
5. 确认模型推理和兑换总 gas 低于单笔预算并留有余量。
6. 网页展示运行位置、模型版本、原始动作和重放结果。
7. 公共链部署再通过钱包签名，不从服务端默认调用用户私钥。

本研究支持“保留原模型结构的整数推理有可行路径”。
它还不支持“已在 Monad 主网完成 AI 自动交易”。

## 复现

标准库即可拆解模型、导出权重并重做整数测试：

    python3 research/ai/study_checkpoint.py

可选的 PyTorch 对照需要 CPU 版 PyTorch 2.9.1：

    python3 research/ai/check_torch_parity.py

安装 Foundry 和 Solidity 0.8.28 后运行：

    cd research/ai/evm
    forge test -vv

结果文件：
research/ai/model-study/analysis.json
research/ai/model-study/pytorch-parity.json
research/ai/model-study/evm-benchmark.json
research/ai/model-study/evm-test-results.txt
