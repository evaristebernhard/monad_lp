export const ACTIONS = ['持有', '买入', '卖出']
export const FEATURE_LABELS = ['1 小时收益', '3 小时收益', '6 小时收益', '12 小时收益', '24 小时收益', '振幅 / 收盘价', '实体涨跌', '偏离 6 小时均价', '偏离 24 小时均价', '6 小时波动率', '24 小时波动率', '相对成交量（对数）']

// Integer reference for the deployed model. Division truncates toward zero.
export function predictTiny(model, values) {
  if (values.length !== 12 || values.some(x => !Number.isSafeInteger(x) || Math.abs(x) > model.input_clip * model.input_q)) throw Error('模型输入必须为 12 个有效的 Q12 整数')
  let x = values.map(BigInt)
  model.layers.forEach((layer, i) => {
    x = layer.weights.map((row, r) => {
      const total = row.reduce((s, w, c) => s + BigInt(w) * x[c], BigInt(layer.bias[r])) / BigInt(model.weight_q)
      return i < model.layers.length - 1 && total < 0n ? 0n : total
    })
  })
  let action = 0
  x.forEach((score, i) => {if (score > x[action]) action = i})
  return {scores: x.map(String), action}
}
