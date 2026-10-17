# Monad 报价 API

公网入口：`https://lp.chayu123.me`。本机后端：`http://127.0.0.1:8100`。
后端负责候选池目录、RPC 读取、ABI 编解码、报价比较及重放哈希。调用方不需要提交合约 calldata。

## 单次报价

GET `/api/selection/quote`，也支持相同路径 POST JSON。

```bash
curl 'https://lp.chayu123.me/api/selection/quote?amount=0.1&token_in=MON&token_out=USDC'
```

```bash
curl 'https://lp.chayu123.me/api/selection/quote' \
  -H 'Content-Type: application/json' \
  -d '{"amount":"0.1","token_in":"USDC","token_out":"MON"}'
```

参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| amount | 字符串 | 正数，MON 最多 18 位小数，USDC 最多 6 位。禁止科学计数法。原始数量不超过 `2^112 - 1`。 |
| token_in | 字符串 | MON 或 USDC，默认 MON。 |
| token_out | 字符串 | 可省略，由输入币推导。必须是同币对另一侧。 |

响应关键字段：

| 字段 | 说明 |
| --- | --- |
| selected | 后端选出的池 ID、原始输出数量、格式化输出和协议。 |
| scenarios[0].quotes | 各候选池报价，含身份、协议、状态、`amountOutRaw`、`amountOut`。 |
| sourceChainId | 报价来源链，固定 Monad 主网 143。 |
| sourceBlock / sourceBlockHash | 所有报价共用的区块和哈希。 |
| sourceTimestamp | 区块 Unix 时间，秒。 |
| startedAtMs / capturedAtMs | 后端开始与完成本轮采集的 Unix 时间，毫秒。 |
| fresh / ageMs | 返回时，区块年龄是否在 5 秒内，以及年龄毫秒数。客户端还需计入接收延迟。 |
| available / candidateCount | 有效报价数量 / 已接入候选数量。 |
| algorithm / gasIncluded | 当前为 `max_gross_output`，`gasIncluded=false`。 |
| replayHash | 包含候选集合、输入、区块和原始报价的哈希，与链上核对合约一致。 |

原始整数始终用字符串返回。不要用浮点数代替 `amountOutRaw` 比较报价。
所有候选失败时返回 502，不用历史报价冒充新报价。无效参数返回 400 或 JSON 解码错误 422。
响应设置 `Cache-Control: no-store`。

## 实时报价

GET `/api/selection/quote-stream`，参数同上。也支持 POST JSON。
响应为 SSE。服务器每秒调度一轮，慢轮次不重叠。客户端断开后停止采集。

```bash
curl -N 'https://lp.chayu123.me/api/selection/quote-stream?amount=0.1&token_in=MON&token_out=USDC'
```

```js
const stream = new EventSource(
  '/api/selection/quote-stream?amount=0.1&token_in=MON&token_out=USDC'
)
stream.onmessage = ({ data }) => {
  const quote = JSON.parse(data)
  if (quote.error) return console.error(quote.error)
  console.log(quote.selected, quote.sourceBlock, quote.fresh)
}
// 停止订阅：stream.close()
```

正常 SSE 帧与单次报价结构一致。出错帧为 `{"error":"报价刷新失败"}`，消费者应将上次结果标记过期。

## 候选池与 20 pool

- GET `/api/selection/pools`：已核验候选、排除项、本地目录覆盖数量。当前接入 14 个 MON/WMON-USDC 直接池。
- GET `/api/monad/prices`：20 pool 最新 RPC 价格，含 `state_block`、`as_of` 和 `status`。
- GET `/api/monad/prices-stream`：持续推送相同价格结构。
- GET `/api/monad/demo`：20 pool 价格、成交量、APR 和波动率。价格由 RPC 更新。成交量/TVL 按 30 秒抓取索引快照，供应商可能有缓存。波动率使用已完成小时历史，不伪造秒级变化。

## 计算范围

当前比较已接入的 14 个直接池，覆盖 Uniswap V2/V3/V4、Pancake V3。另一个 V4 的 PoolKey 待核验。
V3/V2 使用 WMON，按 1:1 与原生 MON 归一。报价计入池手续费，不扣兑换交易 gas 或包装费用，不含多跳和跨池拆单。
所有读数固定在同一区块，并再次检查区块哈希。当前 API 只提供报价，不签名、不兑换、不收费。
