# Monad Blitz 参赛方向审计：千级池子自适应路由 DCAR

- **日期**：2026-10-17。
- **任务定义**：Monad 同链 Swap，对某给定 A→B 输入交易，从约 1,000–2,000 个*全链池子*中寻找可用池 / 路径及资金分配；最多 K=3 份，考虑手续费、Gas 与执行有效性。
- **核心澄清**：1,000–2,000 是用户提供的工程规模假设，**未**核对为 Monad 主网在今日存在且可用的精确池数。整条链全部池数与某一 A/B 交易对的可达候选数不是同一概念。
- **审计裁决**：**BAMR 直接以“智能池选择+多池拆单”为原创点：NO GO**；新研究候选 **DCAR：增量对偶证书复用+失效触发自适应求解：PROTOTYPE VERIFIED / NOVELTY NOT CONFIRMED**。截至本日已检索到一些过往 Blitz 获奖项目，但未掌握 84 场过往活动的完整获奖源码名单，**不能保证历届无人做过同类算法**。

## 1. 历届 Monad Blitz 初步检索（必须区分获奖项目与参赛项目）

| 场次 | 可核验获奖作品 | 与本题关联 | 来源可信度与限制 |
|---|---|---|---|
| 2026-05-16 Çanakkale | Troia（支付编排）、RUNAD（运动身份）、Refaktör（RWA 发票融资） | 非池选择；Troia 属邻近的交易/支付编排 | 联合主办方 BuilderMare 的公开获奖通报 |
| 2026-06-27 Ankara V3 | WICK（随价格源逐块调整定价的 AI 做市商）、DegenSlide、BillBoard dApp | **WICK 与动态流动性定价邻近，但不是面向用户的跨池路由** | 联合主办方 BuilderMare 获奖通报 |
| 2026-04-25 Jogja | Pass Chick（第一名，链上游戏）；Last Nads Standing（第三名） | 非跨池交易 | 获奖者公开自述，未从完整官方名单验证每一名次 |
| 2026-08-22 Hyderabad V3 | Swarm（微任务分发与链上支付） | 非跨池交易 | GitHub 仓库自称获奖，未独立核验具体名次 |
| 2026 年 Blitz V3 | Proof-Of-Meet（现实社交互动与链上证明） | 非跨池交易 | 自述第一名，场次细节有待交叉核验 |
| 2026-09-19 Mumbai V4 | Find Hidden Money（钱包遗漏资产发现） | 非跨池最优路由 | 个人公开获奖自述，未独立核验榜单 |
| 深圳 2025 Blitz 提交仓库 | 有 MonadPay、Simple Pre-market、Torch 等提交 | 文件列表未显示显式“池子路由器”标题 | **仅提交列表，不能据此判断哪些获奖，更不能保证源码没有类似机制** |

**检索结论**：在以上**有限样本**中，未发现一个可确认的历届 Blitz 获奖作品与“2,000 池池选择+增量最优性证书”完全一致；但这个否定发现**仅限已检索公开样本，未达到全历届排除资格**。Monad 官方页面目前标注 84 场过往 Blitz/相关活动记录，并无一站式、完整、可逐项核验的获奖项目+源码名单；某些活动只有报名页或海报。

**特别提示**：2026-10-17 的 Monad Blitz V3 @上海，当日活动时间安排晚上颁奖，在活动尚未结束时不应将它纳入“历届获奖项目”的已核验名单。

原始公开证据：
- 官方历史活动：https://monad.xyz/developers/hackathons
- Blitz 社区档案（自称社区构建、会漏数据）：https://monadblitz.world/about
- Çanakkale 联合主办方通报：https://tr.linkedin.com/posts/buildermare_tr-monad-blitz-%C3%A7anakkale-hackathonu-ba%C5%9Far%C4%B1yla-activity-7470087408663908352-mUuQ
- Ankara V3 联合主办方通报：https://www.linkedin.com/posts/buildermare_32-projects-one-saturday-one-incredible-activity-7477398843211497472--kRn
- Jogja 获奖者消息：https://id.linkedin.com/in/axel-urwawuska-atarubby-730400329
- Hyderabad V3 项目仓库：https://github.com/divyanshkhurana06/swarm
- New Delhi/Blitz V3 项目自述：https://www.linkedin.com/posts/chandan-jha-71243b276_monad-hackathon-web3-activity-7445768116942520320-dnfM
- Mumbai V4 项目自述：https://www.sotwe.com/omoboi_dev?lang=en
- 深圳提交列表：https://github.com/monad-developers/monad-blitz-shenzhen
- 上海当日赛程：https://www.eventmatch.co/en/events/monad-blitz-v3-20261017

## 2. 算法 / 商业竞品查新比 Blitz 历届更重要

Monad 官方聚合器文档现列出 **0x、1inch、Dirol、Kuru Flow、KyberSwap、Monorail、Moose 等**。多池拆单、Gas 考虑、多引擎报价、订单簿并入都是已有工程能力。

已存在的**正式研究**：
1. Angeris 等，*Optimal Routing for Constant Function Market Makers*，ACM EC 2022，CFMM 凸路由以及考虑固定成本的 MICP。
2. Diamandis 等，*An Efficient Algorithm for Optimal Routing Through Constant Function Market Makers*，2023，分解式高效优化。
3. Escudero 等，*Optimal Routing across Constant Function Market Makers with Gas Fees*，2026-03 的 arXiv 预印本，考虑 Gas 激活门槛与近似误差界。
4. 1inch 2025/2026 Pathfinder 升级亦涉及路径合并、拆单、Gas 优化；不能把这些称作首次。

来源：
- https://docs.monad.xyz/tooling-and-infra/dex-aggregators
- https://stanford.edu/~boyd/papers/cfmm_routing.html
- https://arxiv.org/abs/2302.04938
- https://arxiv.org/abs/2603.02844
- https://1inch.com/blog/post/new-pathfinder-algorithm-better-swap-rates

## 3. 目标与新算法 DCAR

针对独立 V2 池的约束子模型：

\[
\max_{x_i,z_i}\ \sum_i w_i q_i(x_i)-\sum_i g_i z_i-G_0,
\quad \sum_i x_i=Q,\quad 0\le x_i\le c_i z_i,\quad\sum_i z_i\le K.
\]

其中 V2 的 `q_i(x)=Y_i a_i x/(X_i+a_i x)` 已含池手续费及价格冲击，`g_i` 是按输出代币估价的**增量**执行 Gas，`w_i` 是已给定的效用缩放，不属于真正统计置信担保。真实异构协议需各自报价器与执行模拟。

选择任意 `μ>0`，定义

\[
\psi_i(\mu)=\max_{0\le x\le c_i} \{w_i q_i(x)-\mu x\},\qquad
U(\mu)=\mu Q-G_0+\sum_{j=1}^{K} [\psi_{(j)}(\mu)-g_{(j)}]_+.
\]

`(j)` 是先按整个 `ψ_i-g_i` 排序后的前 K 项。则给定模型下**任何可行路由**净效用 `F` 满足

\[
F\le \mathrm{OPT}\le U(\mu).
\]

用固定 `μ` 得到有效上界，数值调节 `μ` 得到较紧上界；将后者和实际 shortlist 分配得到的**可行下界** `L` 比较。如果 `U−L ≤ ε|L|`，就有明确定量最优性差距，而不必枚举全 2,000 池。某些实例对偶间隙并不为 0，此时只能报告 Gap，不可宣称全局最优。

### 异步 / 连续区块下的真正差异化构想

在 `b` 区块求得的对偶乘子 `μ_b` 在 `b+1` 区块仍然可用于构造一个**合法的当前上界**，不需要是新块的最优乘子。采用可复用的 `ψ_i(μ_b)-g_i` 索引结构；链上变化集合 `Δ_b` 到来时，只更新受影响池的贡献，重估老路由的实际可行评分 `L_{b+1}`：

\[
U_{b+1}(\mu_b)-L_{b+1}\le \varepsilon |L_{b+1}|
\quad\Rightarrow\quad
\text{可以保留旧路由且有 Gap 证书。}
\]

若不满足则启动**增量失效触发重规划**。上界即使不紧也依然是合法证书；收紧需要再优化 `μ`。缓存堆懒删除维护 Top K，单次小批池更新**摊还**时间约 `O(|Δ_b| log N+K log N)`（本地内存，不含远程读取与链上执行）。

**创新资格**：对偶数学、TopK 索引和选择性重算分别都是既有工具；目前仍只是**组合机制候选**。想以算法创新参赛，应该把“跨区块证书可复用的触发判别 + 真实 Monad 端到端时延/收益优势”作为待验证差异。尚未确认这是首创，不能在 PPT 写“首次提出”。

## 4. 原型与实测（明确是合成数据）

文件：`monad_pool_selector.py`（上一轮基础 V2 引擎）；`monad_dcar_scalable.py`；`monad_dcar_verify.py`；`monad_dcar_verification_20261017.json`。

- 8 / 12 池：与原枚举目标值一致。
- 1,000 池固定示例：约 0.18–0.21 s 离线 CPU；2,000 池固定示例：约 0.23–0.27 s。时间会随机器环境、池分布变化。
- 12 个随机规模压力实例（分别 N=1,000 / 2,000，固定不同随机种子及输入规模）：本机纯 Python 的中位求解时间约 204 / 245 ms，最宽最优值上界缺口分别约 0.004615% / 0.004614%。这个数字是**上界证书的缺口**，不是相对 0x/1inch 的真实收益提升。
- 120 个小规模且存在容量限制的随机实例，和完整穷举比较：所有算出的上界均未被真实最优值突破，最差报告 Gap 约 **3.57787%**；因此算法不是“始终精确”，需在 Gap 过大时扩大搜索。
- 2,000 池、5 池/块的合成更新：缓存上界索引在同一 Python 进程内增量校验约万分之一秒量级；**不**等于真实 Monad 全流程时延。
- 将当前选择池的输出侧储备人工减少 2% 后，缓存路线的最优性缺口上界约 **1.8887%**，超过 0.1% 容差，于是触发重新求解。该测试是故意压力场景，不代表典型链上价格走势。

**覆盖缺口**：未接入实时 RPC、未统计真实池数量、没有 V3 tick / V4 Hook / Curve / 订单簿适配器，不处理路径共享池，没有合约执行端，不保证不受 MEV 或抢跑影响。优化器的“数学保证”仅覆盖受限模型；尚未评估与 1inch、0x、Dirol、Kuru Flow、Monorail 的同区块实盘报价差距。

## 5. 真正 1,000–2,000 池的两层架构

链上全局池子不是同一交易对的平行池。应构造**多重有向代币图** G=(V,E)：V 为代币，E 为池子的方向性可执行交易边。数量约 1k–2k 是**全图边/池规模的上层假设**，而不是直接运算 2k 个同一交易对池。

1. **索引与校验**：按 Factory 发现池子；核验部署地址、token 行为、费率、hook 安全、当前状态，并记录 `block_number`。
2. **路径预筛**：按输入输出 token 可达性、最多 2–3 跳、白名单和可用深度筛选；禁止循环，不应把静态最短路误认为全局最优有限额路径。
3. **统一报价**：V2 用解析曲线，V3 用集中流动性 tick 模型/Quoter，其他 AMM 与订单簿须真实报价；不支持的池一律不进入执行。
4. **分层优化**：先找可信候选路径，确认路径是否共享底层池；独立 V2 直达路径可使用本证书，复杂异构多跳应使用合法的保守上界或只标示“启发式”，不得挪用 V2 全局证书。
5. **执行前校验**：采用 Monad 官方建议的 RPC 批量、并发、索引器；对真实 Router 的交易原子模拟并校验 `minAmountOut`、Gas、nonce、过期状态、allowance 与 Router 安全。
6. **真实效果判定**：对相同交易、相同区块、相同池访问权限比较 1inch、0x、Dirol、Kuru Flow、Monorail，并报告 p50/p95 算时、数据获取时延、Gas、输出代币、交易失败率；第三方报价接口时效不同，公平性需控制。

Monad 官方 RPC 最佳实践：https://docs.monad.xyz/developer-essentials/best-practices

## 6. 参赛创新性闸门

| 机制 | 本轮结论 | 尚需完成 |
|---|---|---|
| 多池选择/拆单/Gas | **已有技术，非原创** | 作为基础功能使用 |
| V2 对偶证书 | **已有数学思想，受限实例验证** | 整数舍入与区间安全误差 |
| 2,000 池快速求解 | **离线合成实验成立** | 真实全链图数据/不同代币对 |
| 稀疏变动后的证书复用 | **原型成立，创新性未确认** | 与动态最短路/增量路由文献查重；同工况 AB 消融 |
| 全链多跳异构路由 | **尚未实现** | V3、Hook、订单簿、共享池路径 |
| Blitz 历届不重复 | **尚不能全局排除** | 逐活动获奖项目/源码核对，尤其 84 场清单 |
| 提升相对成熟聚合器的真实效益 | **无证据** | 相同区块同量同 Gas 基准与原子模拟 |

**建议**：提交为“带有可审计优化误差界的 Monad 千池动态交换路由实验系统”；重点演示旧路线遇到链上流动性改变如何无须重算或自动失效并切换，**不**使用“唯一/首次/已优于 1inch”等未经证明的表述。

### 直接复现

```bash
python monad_dcar_scalable.py --benchmark
python monad_dcar_scalable.py --config monad_pools_example.json
python monad_dcar_verify.py
```

所有代码只读本地 JSON / 合成数据，不发起真实链上交易。生产部署前需要合约、报价模型和执行器的独立安全审计。
