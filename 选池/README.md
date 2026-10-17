# Monad 选池算法

当前 UI 为报价优先，不展示合约市场。主网最优路径兑换与远程 example、后端启动说明见 [操作指南](../docs/最优报价与主网交易操作指南.md)。Buy/Rent/Own 合约及旧市场组件保留供后续使用。

先读 AUDIT_20261017.md。本目录分成两个独立实现：

- monad_dcar_scalable.py：同交易对、独立 V2 池的 DCAR 拆单优化，浮点研究算法。
- run_v4_selection.mjs：真实 Monad MON/USDC V4 池的同区块精确报价，K=1 单池择优。

不需要训练模型，不依赖此前的股票 PPO。

## 本地算法与审计回归

    python3 -m unittest discover -s 选池 -p 'test_*.py' -v
    python3 选池/monad_dcar_scalable.py --benchmark
    python3 选池/monad_dcar_verify.py
    python3 选池/monad_dcar_scalable.py --config 选池/monad_pools_example.json

示例 JSON 和大规模测试是合成数据，不是实时主网池。

## 真实 V4 选择

在项目根目录执行，需要已运行的本地 RPC，且默认只读取 Monad 主网：

    node 选池/run_v4_selection.mjs

当前候选目录在 results/mon-usdc-v4-catalog.json。
脚本重算完整 PoolKey 哈希，不匹配的池跳过，所有报价使用同一区块。
结果在 results/live-v4-selection.json。该脚本不会签名或兑换。

## 测试网基线

    cd 选池/contracts
    forge test -vv

合约只演示对固定候选报价的整数择优，没有报价真实性验证、资金托管或兑换。
已经部署并完成 6 组只读重放，见 results/testnet-deployment.json。
在项目根目录可生成部署预览：

    node 选池/deploy_testnet_selector.mjs

首次显式部署使用 --deploy，脚本只接受链 ID 10143。
当前已有部署记录，脚本会拒绝重复广播。
私钥只在本机进程内读取，不要写入结果文件或版本库。

## 已保存结果

- results/original-*：修复前复现与基准。
- results/audit-tests.txt：新增回归测试。
- results/verification.json：修复后原验证脚本结果。
- results/live-v4-selection.json：6 个有效真实池、6 组主网固定区块报价。
- results/selector-contract-tests.txt：8 项 Solidity 测试。
- results/testnet-deployment.json：测试网部署和链上重放记录。

## 网页与 Solidity 移植（2026-10-17）

网页入口 `/#selection`，或侧栏“选池策略”。

- MON/USDC 页为兑换卡片，默认 0.1 MON，支持切换 MON/USDC 方向。点击后通过只读 RPC 对 6 个 V4 池获取同区块最新报价，本地择优并可免费调用已部署测试网合约核对；输入变化清除旧结果。报告文件仍保留历史快照。获取的报价不保证稍后实际成交价格。
- DCAR V2 页使用明确标注的合成示例储备。`DcarV2Router.sol` 移植候选子集枚举、Q64 边际水位分配、容量约束、手续费、风险权重与固定成本；最多 6 个候选、3 路。不是千池预筛或对偶证书的完整移植，也不保证整数取整目标的全局最优。
- 调用方提供储备/报价，合约不认证数据、不托管、不执行兑换。真实 V4 池必须使用 V4 报价路径，不能填入虚构 V2 储备。
- 网页支持本地运行、只读链上核对、下载重放 JSON、钱包部署副本、部署超时后恢复确认。签名仅在用户钱包完成。测试网 RPC 代理拒绝发送交易方法。
- DCAR 测试网合约：`0xf4c5146a6535c7f58052e4ec185b3cbdfce4d2ff`；部署和三个实际链上重放见 `results/dcar-testnet-deployment.json`。
- 共 17 项 Solidity 测试通过。六候选压力样例约 16,354,248 gas，部署实际 1,033,056 gas；计算量随候选、拆单数、数值变化，不能以该样例作为任意输入的 gas 上界。

生成浏览器可用的公开 ABI、字节码与快照：

    node 选池/build_web_assets.mjs
    node --test frontend/tests/pool-selection.test.mjs
    node frontend/tests/pool-selection-browser.mjs

修改合约后先 `forge build --root 选池/contracts` 再重新生成网页资产。
`deploy_dcar_testnet.mjs` 只部署到链 10143；已有记录时复用并验证，提交后立即记录交易哈希以防重复部署。

## 实时与候选扩展

当前网页已切换到 14 个经过身份核验的 MON/WMON-USDC 直接池，含 V2、V3、V4 与 Pancake。每秒自动调度报价，慢请求不重叠，错误和旧结果有过期状态。页面只保留输入、输出、池子比较与钱包控件，说明收进“策略与合约”。

详细审计见 `results/selection-live-audit.md`。旧 V4 六池快照和旧合约保留作历史证据，不是目前网页的完整候选范围。

    python3 选池/audit_pool_coverage.py
    node 选池/verify_route_catalog.mjs
    node 选池/build_web_assets.mjs
    node --test frontend/tests/pool-selection.test.mjs frontend/tests/selection-quotes.test.mjs
    node frontend/tests/selection-realtime-browser.mjs

候选身份变化需要重新部署固定白名单的测试网核对合约，再生成网页资产。`deploy_route_selector.mjs` 已有部署记录时复用，不重复广播。

## 原生 MON 服务费演示

`PaidMonUsdcRouteSelector.sol` 新增付费入口 `selectAndPay(context, quotes)`。
创建者固定为部署交易的 `msg.sender`；每次调用必须附带恰好 `serviceFee` 的原生 MON。
报价比较成功后立即把费用转给创建者并发出 `SelectionPaid` 事件；报价无效或收款失败整笔回滚。
金额不足或多付均拒绝，不保留用户余额。手续费不可修改，直接向合约转账不受支持。

这是付费选池重放演示，报价由调用方提供，不认证市场价格，不兑换、不套利。
`eth_call` 模拟和底层 selector 的只读查询仍免费，无法阻止用户自行计算或绕过收费查询。
服务费只在成功上链的付费入口调用中结算；交易 gas 另付。

测试网部署与验证（仅链 10143，固定演示费 0.01 MON）：

    node 选池/deploy_paid_selector.mjs

结果保存在 `results/paid-testnet-deployment.json`，提交哈希立即保存，重跑复用交易。
演示使用合成报价，部署者自己调用：0.01 MON 返回同一钱包，实际净支出为 gas。
生产收费率和收款钱包需要在正式部署前确定；该脚本不会向主网部署。

## Contract Market：Buy / Rent / Own

网页 `/#selection` 现在优先展示合约市场，地址放入“合约详情”，下方保留实时报价试用。
`StrategyMarket.sol` 提供三种独立交易（以下均为测试网演示价）：

- `buy()`：1 MON，当前钱包永久使用授权，不转移所有权。
- `rent()`：0.1 MON / 30 天。到期手动续租，提前续租在现有期限上顺延，不自动扣款。
- `buyOwnership()`：首次挂牌 10 MON，转移 `owner` 和未来授权收入，原有授权不变，卖家保留永久使用权。成交后自动下架。
- `listOwnership(price)`：只有当前 owner 可重新挂牌，0 为下架。买断/租金/租期在部署时固定。
- `select(context, quotes)`：限有效授权或 owner 调用，无额外服务费，仍付 gas。

使用权针对这个合约入口，不构成公开算法/报价的独占权；不认证外部报价、不执行实际兑换。
所有价款成功后原生 MON 直接转给当时的 owner。失败整体回滚。
前端核对运行时代码哈希、读取当前价格/owner/当前钱包授权；钱包提交，保留交易哈希，确认事件后刷新授权。
DCAR V2 仍为合成示例，仅展示未上架，不出售。

    node 选池/deploy_strategy_market.mjs
    node 选池/build_web_assets.mjs
    node frontend/tests/strategy-market-browser.mjs

部署结果在 `results/market-testnet-deployment.json`。脚本只允许链 10143，保留已提交哈希避免重复部署。
三种购买/租用/转让的资金与权限转换通过 Foundry 测试；浏览器验证真实合约价格、三种购买意图和取消签名流程。
