# 从最优 API 交易（Node.js ≥22.9）

```bash
npm ci
cp .env.example .env
# 修改 .env：RPC 和你自己的本机 PRIVATE_KEY
node --env-file-if-exists=.env buy.mjs 0.1 --wallet 0x你的地址  # 只预览
node --env-file=.env buy.mjs 0.1 --send                      # 主网真实兑换
```

默认取本站 14 个已接入池中的最高输出；本地核验、构造路由、本地签名。只支持 MON → USDC。
默认滑点 0.5%，gas 另付；支持 `--min-usdc 0.002`、`--record 文件.json`。
`last-buy.json` 先保存广播哈希，再发交易；出现超时先查哈希，不要重复发。

可选 `--protocol 'Uniswap V2'` / `'Uniswap V3'` / `'PancakeSwap V3'` / `'Uniswap V4'` 仅用于限制协议的路径测试，不是跨协议最优。

独立测试网租金：`node --env-file=.env rent.mjs --send`，支付 0.01 测试 MON 调用报价核对，不兑换。

完整文档：项目 `docs/最优报价与主网交易操作指南.md`。私钥不要发给 API，也不要复制其他目录里已有的密钥示例。
