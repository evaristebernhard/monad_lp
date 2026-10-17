// 公用模块 - 简化的通用功能
pub mod constants;
pub mod metrics;
pub mod simd_utils;
pub mod subscription;

// 重新导出主要类型
pub use constants::*;
pub use metrics::*;
pub use simd_utils::*;
pub use subscription::*;

// 常用类型别名
pub type AnyResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
