# LP Screener / DEX 研究平台

这是一个面向多链 DEX 流动性池的研究和数据平台，不是一个直接下单的交易机器人。
项目当前要回答的是：

- 某条链、某个协议有哪些 LP 池；
- 池子的 token、价格、TVL、成交量、手续费和 RPC 状态是否可信；
- 不同 AMM 类型应该使用什么解码器和刷新策略；
- 在数据质量足够时，进一步研究手续费收益、波动率和期权对冲。

## 本地数据边界

运行时拆成两个 SQLite 数据库：

```text
data/lp_screener.db       catalog、RPC 状态和业务/研究表
data/lp_price_history.db  token/pool 原始价格历史
```

前端和 Rust API 通过 `LP_SQLITE_PATH` 与 `LP_PRICE_DB` 读取这两个库；不能直接读取工作区里的 JSON、CSV 或上游 RPC。价格库默认是主库同目录下的 `lp_price_history.db`。

由于该 SQLite 文件超过 GitHub 普通 Git 文件的 100 MB 限制，它不进入代码历史；当前完整快照作为 [GitHub Release asset](https://github.com/evaristebernhard/arbitage/releases/tag/lp-screener-db-2026-10-07) 发布。新环境可执行：

```bash
gh release download lp-screener-db-2026-10-07 \
  --repo evaristebernhard/arbitage \
  --pattern lp_screener.db \
  --dir data
```

主要表的职责如下：

| 表 | 职责 |
| --- | --- |
| `pools` | 多链池目录、协议、pair、TVL、volume、fee、AMM family 和基础质量信息 |
| `pool_rpc_state` | RPC 检查的区块、解码器、池状态、储备和现货价格 |
| `token_metadata` | token symbol、name、decimals 和元数据问题 |
| `token_price_history`（价格库） | RPC/外部源得到的短期 token USD 锚点历史 |
| `pool_price_history`（价格库） | 每个 pool 独立的相对价格及 token0/token1 USD 历史；无可执行价格时记录 `unavailable` 状态而不填伪价格 |
| `token_price_fetch_state`（价格库） | 外部价格源的重试、成功和错误状态，避免重复请求 |
| `token_volatility` / `pool_volatility` | backend 计算的均价、波动率和 pair 指标 |
| `priority_pools` | 固定的重点池研究集合，当前为 1,000 个 |
| `hedge_analysis` | 可选的实验性期权对冲计算结果，不是池目录的来源 |

`hedge_analysis` 不应由池目录导入脚本填充。它只有在期权报价 snapshot 存在、并显式运行 `backend/lp-options` 后才有意义。

## 三层结构

```text
frontend/                 React/Vite，只负责展示
    │ HTTP
backend/                  Rust API、均价/波动率、研究计算
    │ SQLite attach
data/lp_screener.db       catalog/current state
data/lp_price_history.db  raw price history
    ▲
rpc/                      RPC 转发、池状态、价格历史、期权报价入库
```

- `rpc/`：只负责读取外部链上/RPC/公共市场数据并写入数据库。
- `rpc/workers/lp-refresh/`：按重点池和刷新策略获取池状态，属于 RPC 数据层内部 worker。
- `backend/`：只读取两个数据库，计算均价、波动率、APR 及研究指标。
- `frontend/`：只调用 Rust API，不访问数据库或外部 RPC。
- `backend/lp-options/` 和 `rpc/option/`：期权研究的两个独立部分；前者计算，后者采集报价。
- `docs/`：协议和工程边界说明；运行时不依赖 Python 脚本。

## 启动

```bash
cargo run --manifest-path backend/lp-api/Cargo.toml
```

另一个终端：

```bash
npm --prefix frontend run dev
```

访问 `http://127.0.0.1:5173`。Vite 将 `/api` 转发到 `127.0.0.1:8100`。

## 期权研究的当前边界

期权模块目前只是研究能力，不代表所有协议、所有 token 都已经具备可执行的对冲方案：

```text
rpc/option                 抓取公共期权报价 -> option_snapshots / option_quotes
backend/lp-options         读取报价和池数据 -> hedge_analysis
backend/lp-api             读取结果 -> API
```

没有最新报价 snapshot 时，网页必须显示“未加载”，不能拿历史 `hedge_analysis` 冒充当前结果。

## 检查数据库

```bash
sqlite3 -header -column data/lp_screener.db \
  "SELECT chain, protocol, pool_type, count(*) AS pools FROM pools GROUP BY chain, protocol, pool_type ORDER BY pools DESC;"
```

编译检查：

```bash
cargo check --manifest-path rpc/Cargo.toml --workspace
cargo check --manifest-path backend/Cargo.toml --workspace
cargo check --manifest-path rpc/workers/lp-refresh/Cargo.toml
npm --prefix frontend run build
```

更多边界说明见 [`ARCHITECTURE.md`](ARCHITECTURE.md)、[`data/README.md`](data/README.md) 和
[`docs/all-dex-option-hedge.md`](docs/all-dex-option-hedge.md)。

## 最优报价与远程实盘 example

见 [操作指南](docs/最优报价与主网交易操作指南.md)：20 池后端启动、报价页、另一台机器通过最优 API 兑换、测试网合约租金。独立示例在 `examples/monad-best-buy/`。
