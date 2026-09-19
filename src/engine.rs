//! 钓鱼状态机与后台引擎线程。
//!
//! 流程（台钓 · 鼠标左键）：抛竿 → 等待入水 → 听咬钩声 → 刺鱼(左键) → 收竿动画 → 重复。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::{self, Detector, LoopbackCapture, Template};
use crate::config::Config;
use crate::game::is_game_in_foreground;
use crate::input::{self, Hotkey, Hotkeys, RightHold};
use crate::log::{Level, LogBuffer};

/// 内嵌的默认咬钩音模板（源自 Delta-force-s11-auto-fishing 的「咬钩声音.wav」）。
const BITE_WAV: &[u8] = include_bytes!("../assets/bite.wav");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Stopped,
    Idle,
    Casting,
    WaitingBite,
    Striking,
    Reeling,
}

impl State {
    pub fn label(&self) -> &'static str {
        match self {
            State::Stopped => "已停止",
            State::Idle => "等待抛竿",
            State::Casting => "抛竿等待",
            State::WaitingBite => "等待咬钩",
            State::Striking => "刺鱼",
            State::Reeling => "收竿动画",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub casts: u64,
    pub bites: u64,
    pub catches: u64,
    pub misses: u64,
}

/// UI 与引擎共享的状态。
pub struct Shared {
    pub running: bool,
    pub paused: bool,
    pub state: State,
    pub stats: Stats,
    pub last_sim: f32,
    pub audio_level: f32,
    pub game_focused: bool,
    pub foreground_exe: String,
    pub devices: Vec<String>,
    pub config: Config,
    pub log: LogBuffer,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            running: false,
            paused: false,
            state: State::Stopped,
            stats: Stats::default(),
            last_sim: 0.0,
            audio_level: 0.0,
            game_focused: false,
            foreground_exe: String::new(),
            devices: audio::list_loopback_devices(),
            config: Config::load(),
            log: LogBuffer::default(),
        }
    }
}

struct Machine {
    state: State,
    stats: Stats,
    deadline: Option<Instant>,
    /// 本轮等待咬钩期间的相似度峰值（超时日志用，帮助判断阈值）。
    peak_sim: f32,
    /// 上次输出「游戏不在前台」提示的时间（避免刷屏）。
    last_unfocused_log: Option<Instant>,
}

impl Machine {
    fn new() -> Self {
        Self {
            state: State::Idle,
            stats: Stats::default(),
            deadline: None,
            peak_sim: 0.0,
            last_unfocused_log: None,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// 执行一拍状态机（不阻塞，阻塞仅发生在点击的极短 hold 内）。
    fn step(
        &mut self,
        focused: bool,
        sim: f32,
        cfg: &Config,
        right: &mut RightHold,
        logs: &mut Vec<(Level, String)>,
    ) {
        if !focused {
            right.ensure_up();
            // 游戏不在前台：静默不动会让用户以为死了，每 3 秒提示一次原因。
            let now = Instant::now();
            let should_log = self
                .last_unfocused_log
                .map(|t| now.duration_since(t) >= Duration::from_secs(3))
                .unwrap_or(true);
            if should_log {
                self.last_unfocused_log = Some(now);
                logs.push((
                    Level::Warn,
                    "游戏不在前台，暂停动作（切到游戏窗口即恢复）".to_string(),
                ));
            }
            return;
        }
        self.last_unfocused_log = None;

        let now = Instant::now();
        match self.state {
            State::Idle => {
                if !self.do_cast(cfg, logs) { self.state = State::Stopped; return; }
                self.state = State::Casting;
                self.deadline = Some(now + Duration::from_secs_f64(cfg.cast_delay));
            }
            State::Casting => {
                if self.deadline_is_due() {
                    self.state = State::WaitingBite;
                    // 进入等待咬钩阶段才启动本阶段的超时计时（此前是抛竿等待）。
                    self.deadline = Some(Instant::now() + Duration::from_secs_f64(cfg.round_timeout));
                    self.peak_sim = 0.0;
                    logs.push((Level::Info, "等待咬钩…".to_string()));
                }
            }
            State::WaitingBite => { if cfg.hold_rmb {
                    right.ensure_down();
                } else {
                    right.ensure_up();
                }
                self.peak_sim = self.peak_sim.max(sim);
                let thr = cfg.bite_threshold / 100.0;
                if sim >= thr {
                    right.ensure_up();
                    self.stats.bites += 1;
                    logs.push((
                        Level::Ok,
                        format!("检测到咬钩 (相似度 {:.0}%)", sim * 100.0),
                    ));
                    self.state = State::Striking;
                    self.deadline = Some(now + Duration::from_secs_f64(cfg.strike_delay));
                } else if self.deadline_is_due() {
                    right.ensure_up();
                    self.stats.misses += 1;
                    logs.push((
                        Level::Warn,
                        format!(
                            "单轮 {:.0}s 未咬钩，收竿重抛 (第 {} 次) · 本轮相似度峰值 {:.0}%（阈值 {:.0}%）",
                            cfg.round_timeout,
                            self.stats.misses,
                            self.peak_sim * 100.0,
                            cfg.bite_threshold
                        ),
                    ));
                    if !do_click(cfg, logs) { self.state = State::Stopped; return; }
                    self.state = State::Reeling;
                    self.deadline = Some(now + Duration::from_secs_f64(self.random_reel_wait(cfg)));
                }
            }
            State::Striking => {
                if self.deadline_is_due() {
                    if !do_click(cfg, logs) { self.state = State::Stopped; return; }
                    self.stats.catches += 1;
                    logs.push((Level::Ok, format!("已发送刺鱼点击，累计 {} 次（实际结果以游戏为准）", self.stats.catches)));
                    self.state = State::Reeling;
                    self.deadline = Some(now + Duration::from_secs_f64(self.random_reel_wait(cfg)));
                }
            }
            State::Reeling => {
                if self.deadline_is_due() {
                    self.state = State::Idle;
                }
            }
            State::Stopped => {}
        }
    }

    fn deadline_is_due(&self) -> bool {
        self.deadline.map(|d| Instant::now() >= d).unwrap_or(true)
    }

    fn random_reel_wait(&self, cfg: &Config) -> f64 {
        let lo = cfg.reel_wait_min;
        let hi = cfg.reel_wait_max.max(lo);
        lo + fastrand::f64() * (hi - lo)
    }

    fn do_cast(&mut self, cfg: &Config, logs: &mut Vec<(Level, String)>) -> bool {
        if !do_click(cfg, logs) { return false; }
        if cfg.double_cast && !do_click(cfg, logs) { return false; }
        self.stats.casts += 1;
        logs.push((Level::Info, format!("抛竿 #{}", self.stats.casts)));
        true
    }
}

fn do_click(cfg: &Config, _logs: &mut Vec<(Level, String)>) -> bool {
    input::left_click(cfg.click_hold);
    true
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_sq = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    mean_sq.sqrt()
}

/// 启动后台引擎线程。该线程常驻，仅在 `running` 为真时驱动状态机。
pub fn spawn(shared: Arc<Mutex<Shared>>) {
    std::thread::Builder::new()
        .name("fishing-engine".into())
        .spawn(move || worker_loop(shared))
        .expect("无法创建引擎线程");
}

fn worker_loop(shared: Arc<Mutex<Shared>>) {
    let mut hotkeys = Hotkeys::default();
    let mut right = RightHold::default();
    let audio_buffer: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut capture: Option<LoopbackCapture> = None;
    let mut detector: Option<Detector> = None;
    let mut machine = Machine::new();

    // 固定使用内嵌咬钩音模板（与游戏内音效同源，用户实测可靠）。
    let template = Template::from_bytes(BITE_WAV, audio::TEMPLATE_LEN)
        .expect("内嵌咬钩模板加载失败");

    // 首次启动日志
    if let Ok(mut s) = shared.lock() {
        let device_count = s.devices.len();
        s.log.info(format!(
            "三角洲行动 · 自动钓鱼  共发现 {device_count} 个音频设备 · 内置咬钩音模板"
        ));
    }

    loop {
        let (running, paused, cfg) = {
            let s = shared.lock().unwrap();
            (s.running, s.paused, s.config.clone())
        };

        // ---- 启停捕获与检测器 ----
        if running && capture.is_none() {
            match LoopbackCapture::start(&cfg.device_name, audio_buffer.clone()) {
                Ok(c) => {
                    capture = Some(c);
                    detector = Some(Detector::new(template.clone()));
                    if let Ok(mut s) = shared.lock() {
                        s.stats = Stats::default();
                        s.last_sim = 0.0;
                        s.state = State::Idle;
                        let dev = if cfg.device_name.is_empty() {
                            "默认输出".to_string()
                        } else {
                            cfg.device_name.clone()
                        };
                        if false {
                            s.log.ok(format!("开始运行，音频设备：{dev}"));
                        }
                    }
                }
                Err(e) => {
                    if let Ok(mut s) = shared.lock() {
                        s.log.error(e);
                        s.running = false;
                    }
                }
            }
        }
        if !running && capture.is_some() {
            capture = None;
            detector = None;
            right.ensure_up();
            machine.reset();
            if let Ok(mut s) = shared.lock() {
                s.state = State::Stopped;
                s.log.info("已停止");
            }
        }

        if !running {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }

        // ---- 热键 ----
        match hotkeys.poll() {
            Hotkey::Quit => {
                if let Ok(mut s) = shared.lock() {
                    s.running = false;
                    s.log.info("收到 F9，退出运行");
                }
                continue;
            }
            Hotkey::Pause => {
                if let Ok(mut s) = shared.lock() {
                    s.paused = !s.paused;
                    let paused_now = s.paused;
                    s.log.info(if paused_now { "已暂停 (F8 继续)" } else { "继续" });
                }
                continue;
            }
            Hotkey::None => {}
        }

        if paused {
            right.ensure_up();
            if let Ok(mut s) = shared.lock() {
                s.audio_level = 0.0;
            }
            std::thread::sleep(Duration::from_millis(40));
            continue;
        }

        // ---- 采集音频并喂给检测器 ----
        let mut samples: Vec<f32> = Vec::new();
        if let Ok(mut q) = audio_buffer.lock() {
            samples.extend(q.drain(..));
        }
        let level = rms(&samples);
        let sim = match detector.as_mut() {
            Some(det) if !samples.is_empty() => det.push(&samples),
            Some(det) => det.last_sim(),
            None => 0.0,
        };

        // ---- 前台检测（后台模式跳过：点击通过窗口消息投递，与前台无关） ----
        let focused = is_game_in_foreground(&cfg.game_exe);
        let fg_exe = crate::game::foreground_exe().unwrap_or_default();

        // ---- 状态机 ----
        let mut logs: Vec<(Level, String)> = Vec::new();
        machine.step(focused, sim, &cfg, &mut right, &mut logs);

        // ---- 写回共享状态 ----
        if let Ok(mut s) = shared.lock() {
            s.last_sim = sim;
            s.audio_level = level;
            s.game_focused = focused;
            s.foreground_exe = fg_exe;
            s.state = if s.paused { State::Stopped } else { machine.state };
            s.stats = machine.stats;
            if machine.state == State::Stopped { s.running = false; }
            for (lvl, txt) in logs {
                s.log.push(lvl, txt);
            }
        }

        std::thread::sleep(Duration::from_millis(15));
    }
}


