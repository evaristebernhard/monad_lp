//! ShredStream 配置

/// ShredStream 客户端配置
#[derive(Debug, Clone)]
pub struct ShredStreamConfig {
    /// 连接超时（毫秒）
    pub connection_timeout_ms: u64,
    /// 请求超时（毫秒）
    pub request_timeout_ms: u64,
    /// 最大解码消息大小（字节）
    pub max_decoding_message_size: usize,
    /// 自动重连延迟（毫秒）
    pub reconnect_delay_ms: u64,
    /// 最大重连次数（0 表示无限重连）
    pub max_reconnect_attempts: u32,
}

impl Default for ShredStreamConfig {
    fn default() -> Self {
        Self {
            connection_timeout_ms: 8000,
            request_timeout_ms: 15000,
            max_decoding_message_size: 1024 * 1024 * 100, // 100MB
            reconnect_delay_ms: 1000,
            max_reconnect_attempts: 3,
        }
    }
}

impl ShredStreamConfig {
    /// 低延迟配置 - 最小化处理延迟
    pub fn low_latency() -> Self {
        Self {
            connection_timeout_ms: 5000,
            request_timeout_ms: 10000,
            max_decoding_message_size: 1024 * 1024 * 50,
            reconnect_delay_ms: 100,
            max_reconnect_attempts: 1,
        }
    }

    /// 高吞吐配置 - 优化批量处理
    pub fn high_throughput() -> Self {
        Self {
            connection_timeout_ms: 10000,
            request_timeout_ms: 30000,
            max_decoding_message_size: 1024 * 1024 * 200,
            reconnect_delay_ms: 2000,
            max_reconnect_attempts: 5,
        }
    }
}
