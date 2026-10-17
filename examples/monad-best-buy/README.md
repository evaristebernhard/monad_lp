# 从最优 API 交易（Node.js ≥22.9）

**独立单文件版本：** [`buy-standalone.mjs`](buy-standalone.mjs) 已包含候选池目录和所需库。另一台电脑只需 Node.js（本次实测 24.18.0）和按 [`standalone.env.example`](standalone.env.example) 填写的 `.env`，不用 npm 安装或复制项目其他文件：

```bash
# 私钥配置在本机 .env；先预览
node --use-env-proxy --env-file=.env buy-standalone.mjs 0.1
# 真实购买 0.1 MON，另付 gas
node --use-env-proxy --env-file=.env buy-standalone.mjs 0.1 --send --record buy-001.json
```

完整说明及 0.1 MON 真实购买记录见 [独立单文件最优报价购买](../../docs/独立单文件最优报价购买.md)。后文 `buy.mjs` 是保留的模块化源码版本。

只想跑最优报价、采集数据，使用同目录的 `quotes.py`，不要先运行交易预览。仅需 Python 3，不需要安装依赖、钱包、私钥、独立 RPC、`.env` 或本地后端：

```bash
# 在此目录执行；Windows 可以将 python3 换成 py -3
python3 quotes.py --amount 0.1
# 每轮结束后等 2 秒，持续保存全部候选报价；Ctrl+C 停止
python3 quotes.py --amount 0.1 --count 0 --interval 2 --output monad-quotes.jsonl
# 反向报价
python3 quotes.py --amount 1 --token-in USDC
```

默认访问 `https://api.chayu123.me`，可用 `--api-url https://lp.chayu123.me` 切换。只填域名，不要再加 `/api`。JSONL 每行是完整响应，包含 `selected`、`scenarios[0].quotes` 全部候选、来源区块和时间。脚本额外记录客户端接收时间、报价年龄和 `clientFresh`，不会把过期数据标成新报价。失败输出到 stderr，不写入伪造的数据行；有限次采集有失败时返回非零退出码。

完整跨电脑调用说明见 [`docs/另一台电脑采集最优报价.md`](../../docs/另一台电脑采集最优报价.md)。

以下是需要钱包与额外 RPC 核验的**交易示例**：

```bash
npm ci
# 将提供的 standalone.env.example 复制成 .env，在本机填写下方变量
node --env-file-if-exists=.env buy.mjs 0.1 --wallet 0x你的地址  # 只预览
node --env-file=.env buy.mjs 0.1 --send                      # 主网真实兑换
```

`.env` 不随仓库上传；按需在签名电脑创建，内容格式如下：

```dotenv
MONAD_API_URL=https://api.chayu123.me
MONAD_RPC_URL=https://rpc.monad.xyz
PRIVATE_KEY=只在签名电脑填写你自己的私钥
```

默认取本站 14 个已接入池中的最高输出；本地核验、构造路由、本地签名。只支持 MON → USDC。
默认滑点 0.5%，gas 另付；支持 `--min-usdc 0.002`、`--record 文件.json`。
`last-buy.json` 先保存广播哈希，再发交易；出现超时先查哈希，不要重复发。

可选 `--protocol 'Uniswap V2'` / `'Uniswap V3'` / `'PancakeSwap V3'` / `'Uniswap V4'` 仅用于限制协议的路径测试，不是跨协议最优。

独立测试网租金：`node --env-file=.env rent.mjs --send`，支付 0.01 测试 MON 调用报价核对，不兑换。

完整文档：项目 `docs/最优报价与主网交易操作指南.md`。私钥不要发给 API，也不要复制其他目录里已有的密钥示例。
