# 数据目录

## 主数据库

`data/lp_screener.db` 是 catalog/current-state SQLite 数据库；`data/lp_price_history.db` 单独保存 token/pool 原始价格历史。Rust API 会自动 attach 价格库，路径可用 `LP_PRICE_DB` 覆盖。两个文件都是 frontend 和 Rust API 的运行时数据边界，不放进普通 Git 历史，因为 GitHub 对单个文件有 100 MB 限制。

下载当前快照：

```bash
gh release download lp-screener-db-2026-10-07 \
  --repo evaristebernhard/arbitage \
  --pattern lp_screener.db \
  --dir data
```

下载后的 catalog 文件路径应为 `data/lp_screener.db`；价格历史文件应放在 `data/lp_price_history.db`。GitHub 的代码 checkout 不会自动包含运行时数据库。

不要把池数据重新散落到新的 JSON/CSV 文件中。临时导出只能放在被忽略的临时目录，不能作为运行时输入。

## 数据写入方向

```text
rpc / workers              -> catalog 中的 RPC 状态，价格库中的 token/pool 历史
backend                    -> 均价、波动率和研究派生表
frontend                   <- Rust API <- SQLite
```

当前最小运行形态不需要 ClickHouse；本地 SQLite 就是池目录和派生指标的事实来源。

## 表的所有权

- `pools`、`priority_pools`：池目录导入/维护流程。
- `pool_rpc_state`：`rpc/workers/lp-refresh`。
- `token_metadata`：catalog 中的 `rpc` 数据层元数据。
- `token_price_history`、`pool_price_history`、`token_price_fetch_state`：价格库中的原始历史与抓取状态。
- `token_volatility`、`pool_volatility`：`backend/lp-metrics`。
- `option_snapshots`、`option_quotes`：`rpc/option`。
- `hedge_analysis`：`backend/lp-options`，属于实验性派生结果。

池目录重建时不能从旧分析 JSON 恢复 `hedge_analysis`，否则会把过期或不匹配的方案再次显示到网页。

token USD 锚点按 token 维护，pool 可执行价格按 pool 维护。RPC 当前状态每小时采样一次，最多保留 30 天；DefiLlama 只作为稀疏 token 历史补充，缺失 token 的重试状态写入 `token_price_fetch_state`。pool 波动率只读取对应 pool 的原始历史，backend 负责计算均价和波动率。
