# Monad AI · 60 秒自动交易演示

入口：**`/#ai`**（本地 `http://127.0.0.1:5173/#ai`）。

这是一个链上小模型驱动的 Monad Testnet 演示：钱包签一次 `Start`，Keeper 每 10 秒推进一轮 BUY / HOLD / SELL，六轮结束。模型推理发生在合约内，测试 USDC/WMON 的兑换通过 PuddleSwap 完成，兑换所得直接回到连接的钱包。

## 现在使用的合约

| 项目 | 地址 |
| --- | --- |
| 六轮会话合约（当前） | [0xfb2f3fe4a40f66e77f5ddd5b25de7cdd6ad534dd](https://testnet.monadscan.com/address/0xfb2f3fe4a40f66e77f5ddd5b25de7cdd6ad534dd) |
| 131 参数 AI 模型 | [0x5dd31af6d70b7633fd5da3e30c03feb5fde00618](https://testnet.monadscan.com/address/0x5dd31af6d70b7633fd5da3e30c03feb5fde00618) |
| PuddleSwap Router | `0x430c23895c8D44883526e3E0B09327dAD8766660` |
| Circle 测试 USDC | `0x534b2f3A21130d7a60830c2Df862319e593943A3` |
| Router 使用的 WMON | `0x97B3070F9Da6C002343862b35E68Bd8e22608943` |

部署交易：[0x4643f3f5…8697b8c](https://testnet.monadscan.com/tx/0x4643f3f594220acf78d37cb5aa46c6d335053a31b97c46460d8b17b058697b8c)，区块 71769504，费用 0.170955 测试 MON。Chain ID 固定为 10143。部署清单：`session-deployment.json`。

## 运行

首次编译与检查：

```bash
forge test --root research/ai/evm --match-contract TinyMonadSessionTraderTest -vv
node research/ai/showcase/deploy-session.mjs
```

启动 Keeper（本地后台运行）：

```bash
node research/ai/showcase/keeper.mjs --daemon
```

前端（已有本地服务可直接打开页面）：

```bash
npm --prefix frontend run dev
```

访问 `http://127.0.0.1:5173/#ai`。状态栏显示 Keeper ONLINE 后即可连接 MetaMask。

**钱包准备：** 有测试 MON 付授权与 Start 的 Gas、有测试 USDC 用于 BUY。已有测试钱包先前使用 10 测试 MON 换得 36.969158 测试 USDC，记录在 `archive/purchase-10-mon-usdc.json`。

首次点击「授权本次演示额度」分别授权 **0.8 USDC 和 0.2 WMON** 给会话合约，两笔 ERC-20 授权需要钱包确认。初始 WMON 余额可以是零：第 1 笔 BUY 得到的 WMON 用于第 3 笔 SELL。

之后点击 **START 60s AUTO TRADING**，在 MetaMask 签署一次 Start；后六笔由 Keeper 自动发送，不再弹出钱包确认。每笔 BUY 使用 0.4 USDC、SELL 使用 0.1 WMON、HOLD 只提交链上决策事件。Keeper 代付六笔交易的 Gas。六轮完成后再次启动，需要重新补足已消耗的 ERC-20 授权。

会话可以随时 Cancel，110 秒自动过期。六次的最早执行点是 Start 后约 0、10、20、30、40、50 秒，链上确认与 RPC 延迟可能让完成时间超过 60 秒。当前使用的是**真实历史行情特征回放**，不是每 10 秒更新一次的实时价格预测；模型权重和推理都是真实上链的。

## 目录

```text
research/ai/
  showcase/
    README.md                此操作入口
    deploy-session.mjs       当前会话合约部署和复核
    session-deployment.json  当前部署交易及地址
    keeper.mjs               自动执行/重试控制
    archive/                 前几次单笔签名和预存资金原型及原交易记录
  evm/
    src/TinyMonadSessionTrader.sol
    src/TinyMonadPolicy.sol
    test/TinyMonadSessionTrader.t.sol
  monad-small/               模型训练、数据、模型参数
  monad-small-v2/            其他实验
  model-study/               原先较大的 PPO actor 实验

frontend/
  src/ai/
    AiShowcase.jsx           当前演示界面
    aiShowcase.css
    research/                过去的回测/模型研究前端，已从菜单移除
  src/generated/monad-ai.json
  public/ai/session-config.json
```

Keeper 的日志、PID、状态缓存被 .gitignore 排除。前端使用的 `keeper-status.json` 是本地 Keeper 定期生成的状态文件，不保存密钥。钱包私钥只在测试网本地 Keeper 进程中使用，不进前端。

## 验证

```bash
forge test --root research/ai/evm --match-contract TinyMonadSessionTraderTest -vv
node research/ai/showcase/keeper.mjs --once
npm --prefix frontend run build
node frontend/tests/ai-wallet-showcase-browser.mjs
node --test frontend/tests/monad-ai.test.mjs
```

Foundry 已验证：六轮自动完成、无法绕过 Start、Keeper 不能伪造输入、无法提前执行、钱包取消后立即停止、到期和旧会话不能重放。浏览器已验证桌面、手机和模拟钱包连接。真正的六笔测试网交易将在用户点击 Start 之后才会发生。
