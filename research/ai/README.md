# 已训练交易模型

已下载 Adilbai/stock-trading-rl-agent 的 final_model.zip。
这是已有的 PPO 检查点，不需要先训练才能取得权重。
原模型用于股票交易。已完成权重拆解、合成输入的 PyTorch 对照与本地 EVM 整数推理测试。尚未运行真实 MON 输入或部署公共链合约。

- 原始仓库：https://huggingface.co/Adilbai/stock-trading-rl-agent
- 下载路径：third_party/stock-trading-rl-agent
- 固定版本：a317fec1939eda44d04088b47b48ca3ee158bc1f
- 权重：final_model.zip，4,875,406 字节，检查点记录训练步数 501,760。
- 标准化参数：scaler.pkl，2,423 字节。
- 同步保留模型卡、配置、环境实现、特征处理实现和训练记录。
- 模型卡声明许可证为 MIT，模型仓库未附单独的 LICENSE 文件。后续再分发时核对授权文本。

Hugging Face 直连超时，所以使用 hf-mirror.com 下载。
本地提交与模型元数据中的版本一致。
权重和标准化参数的 SHA-256 与该提交的 Git LFS OID 一致。
其余图表及 TensorBoard 大文件保留 LFS 指针，不影响模型权重的完整性。

运行验证：

    python3 research/ai/verify_pretrained_model.py

校验结果已保存到 pretrained-model-manifest.json。
验证器只读取 JSON、ZIP 和哈希，不执行仓库代码或反序列化模型对象。

## 实际接口

输入为 60 步 x 50 个行情特征，再加 8 个账户特征，共 3,008 个浮点数。
scaler.pkl 与 dataprocessor.py 定义了原始特征处理。
移植时必须固定特征顺序、时间窗口、标准化和账户归一化规则。

输出为两个连续数值。第一个经裁剪和取整得到动作：
0 为持有，1 为买入，2 为卖出。第二个经裁剪得到 0 到 1 的仓位比例。
它不是三个动作的概率或 logits。不能直接对输出做三分类 argmax。
动作协议以 enviromentcreator.py 的 step 实现为准。

## 链上适配

该模型与 Neurom 的整数权重、位输入和阈值/LIF 激活不直接兼容。
下一步先运行原始检查点，导出 actor 参数和输入输出测试样本。
如果保留原始网络，需要定点推理引擎、激活函数实现及误差报告。
如果蒸馏成 Neurom 网络，需要明确记录这是新模型，并分别报告：
教师与学生决策一致率，以及学生本地和链上整数推理的一致性。

目前没有执行 MON/USDC 交易，也没有把样例输出当作 Monad 市场信号。


## 上链研究结果

研究报告：../../docs/pretrained-model-onchain-study.md。
已经导出原 actor 的浮点参数和实验用 int16 权重。
model-study/ 包含参数布局、标准化参数、数值对照和 gas 记录。
evm/ 是独立研究原型，使用自定义 Tanh 引擎，不是 Neurom 的原有引擎。
它没有资金托管或兑换功能，不连接前端，不改变当前交易行为。

原始检查点的 statically verified manifest 不代表新研究实验的完成状态。
新实验分别以 analysis.json、pytorch-parity.json 和 evm-benchmark.json 记录。
