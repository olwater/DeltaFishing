//! 配置结构、默认值与持久化（JSON 保存到 %LOCALAPPDATA%）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const GAME_EXE_DEFAULT: &str = "DeltaForceClient-Win64-Shipping.exe";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 游戏进程名（用于判定前台窗口）
    pub game_exe: String,
    /// 音频回环设备名（空 = 自动选择默认输出设备）
    pub device_name: String,
    /// 咬钩相似度阈值，0..=100（百分比）
    pub bite_threshold: f32,
    /// 最小电平门限，dBFS（低于此值判为远处他人咬钩）
    pub level_min_db: f32,
    /// 最大电平门限，dBFS（高于此值判为贴脸枪声/爆炸，0.0 表示不限制）
    pub level_max_db: f32,
    /// 最大允许声像差绝对值，dB（超过说明偏离正前方，为旁边他人咬钩）
    pub pan_max_db: f32,
    /// 抛竿后等待（此期间忽略咬钩声），秒
    pub cast_delay: f64,
    /// 检测到咬钩后再等多久点击收竿，秒
    pub strike_delay: f64,
    /// 收竿后动画等待（随机范围下限/上限），秒（未开启跳过动画时生效）
    pub reel_wait_min: f64,
    pub reel_wait_max: f64,
    /// 刺鱼后是否打断跳过展示鱼动画（提高挂机效率）
    pub skip_anim: bool,
    /// 刺鱼后等待多久点击跳过动画，秒
    pub skip_anim_delay: f64,
    /// 跳过动画后的等待时间（随机范围下限/上限），秒
    pub post_skip_wait_min: f64,
    pub post_skip_wait_max: f64,
    /// 等待咬钩阶段的超时秒数，超时兜底收竿重抛
    pub round_timeout: f64,
    /// 超时收竿时是否执行 3->6 切刀切竿强制重置状态
    pub reset_on_timeout: bool,
    /// 鼠标点击按住时长，秒
    pub click_hold: f64,
    /// 抛竿是否连点两次
    pub double_cast: bool,
    /// 等待咬钩时按住右键（缩放视角，利用游戏引擎屏蔽他人声音与杂音）
    pub hold_rmb: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            game_exe: GAME_EXE_DEFAULT.to_string(),
            device_name: String::new(),
            bite_threshold: 60.0,
            level_min_db: -28.0,
            level_max_db: -6.0,
            pan_max_db: 5.0,
            cast_delay: 4.5,
            strike_delay: 0.2,
            reel_wait_min: 7.0,
            reel_wait_max: 9.0,
            skip_anim: true,
            skip_anim_delay: 1.2,
            post_skip_wait_min: 2.0,
            post_skip_wait_max: 2.8,
            round_timeout: 20.0,
            reset_on_timeout: true,
            click_hold: 0.10,
            double_cast: true,
            hold_rmb: true,
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("DeltaFishing").join("config.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Config::default(),
        };
        // 修正旧版本异常或不合法值
        if !cfg.bite_threshold.is_finite() || cfg.bite_threshold <= 0.0 {
            cfg.bite_threshold = Self::default().bite_threshold;
        }
        cfg.bite_threshold = cfg.bite_threshold.clamp(1.0, 100.0);
        if !cfg.level_min_db.is_finite() || cfg.level_min_db >= 0.0 {
            cfg.level_min_db = -28.0;
        }
        if !cfg.pan_max_db.is_finite() || cfg.pan_max_db <= 0.0 {
            cfg.pan_max_db = 5.0;
        }
        cfg.cast_delay = cfg.cast_delay.max(0.1);
        cfg.round_timeout = cfg.round_timeout.max(1.0);
        cfg
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
    }
}
