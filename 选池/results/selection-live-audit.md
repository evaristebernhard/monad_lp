# 实时报价与候选覆盖审计 — 2026-10-17

## 发现与修复

1. 原网页只有点击刷新，没有定时任务。现在默认每秒调度，前台可暂停或继续。请求未完成时跳过本轮，不叠加请求。金额、方向、页面状态变化会取消旧请求，旧响应不能写回。页面后台停止轮询，恢复可见时立即刷新。
2. 原候选集合只含 6 个无 hook 的原生 MON/USDC V4 池。按代币地址核对本地库，Monad 共 1,574 池，MON/WMON 与 USDC 直接池共 15 个：V4 7、Uni V3 4、Pancake V3 3、Uni V2 1。核验工厂、代币、费率后接入 14 个，另 1 个 V4 的 PoolKey 未验证。目录不是完整全链索引。
3. 当前目标是这些候选的兑换输出最大，手续费由池报价计入。没有扣除完整交易 gas、WMON 包装费用，也没有跨币对或多跳拆单。页面去掉全局最优暗示，显示范围。
4. 单池失败曾令整轮失败。现在失败保留为零报价占位，其他候选继续比较，并显示覆盖不足。所有失败时保留上次结果但标记过期。
5. 同输出按数值 pool ID 升序选择，与合约保持一致。比较原始整数，不比较格式化字符串。0.1 MON 的 USDC 输出因 6 位精度可能连续多个区块不变，不能用数字不动判断没有刷新。
6. 每轮通过 Multicall3 合并报价。如果个别子调用失败，会在同一区块单独重试该池，避免聚合调用 gas 限制错误排除有效池。V2 用真实 reserves 与 0.3% 标准手续费计算。V3 使用各协议 QuoterV2，WMON 与 MON 按 1:1 归一。

## 验证证据

- `pool-coverage.json` / `route-catalog.json`：实际候选和排除项。地址匹配不依赖 symbol 文本。
- `live-route-audit.json`：固定主网区块 114087043，MON→USDC 与 USDC→MON 各 14 份批量报价均与逐池调用逐整数一致。测试网合约的池 ID、输出与重放哈希均一致。
- 此次样例 0.1 MON 的最高输出来自新增 Uni V3 低费率池。以前仅比较 V4 会漏掉它。这不是未来成交保证。
- 样例批量耗时 503 / 685 ms，逐池路径 1158 / 1637 ms。1 秒为调度目标，慢 RPC 时不会伪造新报价。页面显示实际区块与报价年龄，5 秒以上或刷新失败标记过期。
- 新 14 池核对合约：`0x0b50d15b3547f7f6f9424192497a1723a005a70a`，只在 Monad 测试网 10143 部署。见 `route-testnet-deployment.json`。不托管资金、不认证调用方报价、不执行兑换。
- Node 11 项、Foundry 17 项通过。浏览器实际链上核对通过。模拟 RPC 验证 1s 调度、不重叠、区块及输出变化、暂停恢复、过期输入丢弃、上游故障状态。钱包连接、账户和网络切换、拒绝签名与手机布局通过。

## 地址来源

- Uniswap V3 QuoterV2：https://developers.uniswap.org/docs/protocols/v3/deployments/v3-monad-deployments
- Pancake QuoterV2 已验证源码：https://monadscan.com/address/0xb048bbc1ee6b733fffcfb9e9cef7375518e25997#code
- Multicall3 地址采用项目已安装 viem 的 Monad 链配置。
- 两类 V3 Quoter 的 factory() 与候选池 factory()、getPool() 链上核对一致。


## 最终报价服务与公网核验

后端现在提供参数化 `GET/POST /api/selection/quote` 与 `GET/POST /api/selection/quote-stream`。只传金额和币对，后端负责固定目录、ABI 编码与解析、RPC、整数排序和重放哈希。前端不再提交报价 calldata。
公网 JSON 双向报价与独立逐池读取、测试网核对全部一致，见 `quote-api-verification.json`。公网 SSE 间隔实测 1031 / 995 / 1046 ms。14 个后端单元测试、11 个 Node 测试通过。
20 pool 通过 `/api/monad/prices-stream` 推送。RPC 快速价格任务按同一区块并行读取批次；批次不受支持时并行回退。公网检查确认 20 个池来源区块和来源时间全部推进，样例数据年龄 2–3 秒，见 `output/monad-live-demo-check.json`（项目根目录）。
详细调用方式见 `docs/quote-api.md`。成交量/TVL 30 秒采集与小时波动率仍保留真实来源周期。
