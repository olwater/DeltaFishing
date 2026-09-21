//! 钓鱼状态机与后台引擎线程。
//!
//! 流程（台钓 · 鼠标左键）：
//! 抛竿 → 等待入水 → 听咬钩声(立体声三层判据+右键聚焦) → 刺鱼(左键) → 跳过收竿动画/切刀重置 → 重复。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio::{self, DetectionResult, Detector, LoopbackCapture, StereoFrame, Template};
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
    pub last_level_db: f32,
    pub last_pan_db: f32,
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
            last_level_db: -99.0,
            last_pan_db: 0.0,
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
    /// 本轮等待咬钩期间的相似度峰值。
    peak_sim: f32,
    /// 上次输出「游戏不在前台」提示的时间（避免刷屏）。
    last_unfocused_log: Option<Instant>,
    /// 拦截日志冷却时间戳（避免短时间内刷屏报同一个拦截）。
    last_reject_log: Option<Instant>,
    /// 本次收竿是否已经执行过跳过动画点击。
    anim_skipped: bool,
}

impl Machine {
    fn new() -> Self {
        Self {
            state: State::Idle,
            stats: Stats::default(),
            deadline: None,
            peak_sim: 0.0,
            last_unfocused_log: None,
            last_reject_log: None,
            anim_skipped: false,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    /// 执行一拍状态机。
    fn step(
        &mut self,
        focused: bool,
        det: DetectionResult,
        cfg: &Config,
        right: &mut RightHold,
        logs: &mut Vec<(Level, String)>,
    ) {
        if !focused {
            right.ensure_up();
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
                if !self.do_cast(cfg, logs) {
                    self.state = State::Stopped;
                    return;
                }
                self.state = State::Casting;
                self.deadline = Some(now + Duration::from_secs_f64(cfg.cast_delay));
            }
            State::Casting => {
                if self.deadline_is_due() {
                    self.state = State::WaitingBite;
                    self.deadline = Some(Instant::now() + Duration::from_secs_f64(cfg.round_timeout));
                    self.peak_sim = 0.0;
                    self.anim_skipped = false;
                    logs.push((Level::Info, "等待咬钩…".to_string()));
                }
            }
            State::WaitingBite => {
                if cfg.hold_rmb {
                    right.ensure_down();
                } else {
                    right.ensure_up();
                }
                self.peak_sim = self.peak_sim.max(det.sim);

                let thr = cfg.bite_threshold / 100.0;
                let sim_ok = det.sim >= thr;
                let level_ok = det.level_db >= cfg.level_min_db
                    && (cfg.level_max_db == 0.0 || det.level_db <= cfg.level_max_db);
                let pan_ok = det.pan_db.abs() <= cfg.pan_max_db;

                if sim_ok {
                    if !level_ok || !pan_ok {
                        // 命中咬钩特征但未通过归属判据，拦截并防刷屏记录
                        let can_log = self
                            .last_reject_log
                            .map(|t| now.duration_since(t) >= Duration::from_millis(800))
                            .unwrap_or(true);
                        if can_log {
                            self.last_reject_log = Some(now);
                            if !level_ok {
                                if det.level_db < cfg.level_min_db {
                                    logs.push((
                                        Level::Warn,
                                        format!(
                                            "拦截远处咬钩: 电平 {:.1}dB < 门限 {:.1}dB (相似度 {:.0}%)",
                                            det.level_db, cfg.level_min_db, det.sim * 100.0
                                        ),
                                    ));
                                } else {
                                    logs.push((
                                        Level::Warn,
                                        format!(
                                            "拦截爆音/枪炮: 电平 {:.1}dB > 门限 {:.1}dB (相似度 {:.0}%)",
                                            det.level_db, cfg.level_max_db, det.sim * 100.0
                                        ),
                                    ));
                                }
                            } else if !pan_ok {
                                let side = if det.pan_db > 0.0 { "左侧" } else { "右侧" };
                                logs.push((
                                    Level::Warn,
                                    format!(
                                        "拦截他人咬钩({}): 声像差 {:+.1}dB 偏离正前方 (门限 ±{:.1}dB · 相似度 {:.0}%)",
                                        side, det.pan_db, cfg.pan_max_db, det.sim * 100.0
                                    ),
                                ));
                            }
                        }
                    } else {
                        // 三道闸门全部通过：确认为自己的鱼！
                        right.ensure_up();
                        self.stats.bites += 1;
                        logs.push((
                            Level::Ok,
                            format!(
                                "检测到咬钩！(相似度 {:.0}% · 电平 {:.1}dB · 声像 {:+.1}dB)",
                                det.sim * 100.0,
                                det.level_db,
                                det.pan_db
                            ),
                        ));
                        self.state = State::Striking;
                        self.deadline = Some(now + Duration::from_secs_f64(cfg.strike_delay));
                    }
                } else if self.deadline_is_due() {
                    // 单轮超时未咬钩
                    right.ensure_up();
                    self.stats.misses += 1;
                    logs.push((
                        Level::Warn,
                        format!(
                            "单轮 {:.0}s 未咬钩，收竿重抛 (第 {} 次) · 峰值相似度 {:.0}%",
                            cfg.round_timeout,
                            self.stats.misses,
                            self.peak_sim * 100.0
                        ),
                    ));

                    if cfg.reset_on_timeout {
                        logs.push((Level::Info, "执行 3→6 切刀切竿强制重置状态".to_string()));
                        input::reset_fishing_stance();
                        self.state = State::Idle;
                    } else {
                        if !do_click(cfg, logs) {
                            self.state = State::Stopped;
                            return;
                        }
                        self.state = State::Reeling;
                        self.anim_skipped = true; // 超时收竿没有展示鱼动画
                        self.deadline = Some(now + Duration::from_secs_f64(self.random_reel_wait(cfg)));
                    }
                }
            }
            State::Striking => {
                if self.deadline_is_due() {
                    if !do_click(cfg, logs) {
                        self.state = State::Stopped;
                        return;
                    }
                    self.stats.catches += 1;
                    logs.push((
                        Level::Ok,
                        format!("已发送刺鱼点击，累计 {} 次", self.stats.catches),
                    ));
                    self.state = State::Reeling;
                    if cfg.skip_anim {
                        self.anim_skipped = false;
                        self.deadline = Some(now + Duration::from_secs_f64(cfg.skip_anim_delay));
                    } else {
                        self.anim_skipped = true;
                        self.deadline = Some(now + Duration::from_secs_f64(self.random_reel_wait(cfg)));
                    }
                }
            }
            State::Reeling => {
                if self.deadline_is_due() {
                    if cfg.skip_anim && !self.anim_skipped {
                        // 刺鱼后第一次到期：轻点左键打断展示鱼动画
                        self.anim_skipped = true;
                        if !do_click(cfg, logs) {
                            self.state = State::Stopped;
                            return;
                        }
                        logs.push((Level::Info, "已点击打断展示鱼动画，提速收竿".to_string()));
                        let wait = self.random_post_skip_wait(cfg);
                        self.deadline = Some(now + Duration::from_secs_f64(wait));
                    } else {
                        // 收竿动画全部结束，进入下一轮
                        self.state = State::Idle;
                    }
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

    fn random_post_skip_wait(&self, cfg: &Config) -> f64 {
        let lo = cfg.post_skip_wait_min;
        let hi = cfg.post_skip_wait_max.max(lo);
        lo + fastrand::f64() * (hi - lo)
    }

    fn do_cast(&mut self, cfg: &Config, logs: &mut Vec<(Level, String)>) -> bool {
        if !do_click(cfg, logs) {
            return false;
        }
        if cfg.double_cast && !do_click(cfg, logs) {
            return false;
        }
        self.stats.casts += 1;
        logs.push((Level::Info, format!("抛竿 #{}", self.stats.casts)));
        true
    }
}

fn do_click(cfg: &Config, _logs: &mut Vec<(Level, String)>) -> bool {
    input::left_click(cfg.click_hold);
    true
}

fn rms_stereo(samples: &[StereoFrame]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mean_sq: f32 = samples
        .iter()
        .map(|s| {
            let m = s.mono();
            m * m
        })
        .sum::<f32>()
        / samples.len() as f32;
    mean_sq.sqrt()
}

/// 启动后台引擎线程。
pub fn spawn(shared: Arc<Mutex<Shared>>) {
    std::thread::Builder::new()
        .name("fishing-engine".into())
        .spawn(move || worker_loop(shared))
        .expect("无法创建引擎线程");
}

fn worker_loop(shared: Arc<Mutex<Shared>>) {
    let mut hotkeys = Hotkeys::default();
    let mut right = RightHold::default();
    let audio_buffer: Arc<Mutex<VecDeque<StereoFrame>>> = Arc::new(Mutex::new(VecDeque::new()));
    let mut capture: Option<LoopbackCapture> = None;
    let mut detector: Option<Detector> = None;
    let mut machine = Machine::new();

    // 固定使用内嵌咬钩音模板（0.35s 瞬态核心）。
    let template = Template::from_bytes(BITE_WAV, audio::TEMPLATE_LEN)
        .expect("内嵌咬钩模板加载失败");

    if let Ok(mut s) = shared.lock() {
        let device_count = s.devices.len();
        s.log.info(format!(
            "三角洲行动 · 自动钓鱼  共发现 {device_count} 个音频设备 · 立体声声学三层判据已就绪"
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
                        s.last_level_db = -99.0;
                        s.last_pan_db = 0.0;
                        s.state = State::Idle;
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

        // ---- 采集立体声音频并喂给检测器 ----
        let mut samples: Vec<StereoFrame> = Vec::new();
        if let Ok(mut q) = audio_buffer.lock() {
            samples.extend(q.drain(..));
        }
        let level = rms_stereo(&samples);
        let det_res = match detector.as_mut() {
            Some(det) if !samples.is_empty() => det.push(&samples),
            Some(det) => det.last_result(),
            None => DetectionResult::default(),
        };

        // ---- 前台检测 ----
        let focused = is_game_in_foreground(&cfg.game_exe);
        let fg_exe = crate::game::foreground_exe().unwrap_or_default();

        // ---- 状态机 ----
        let mut logs: Vec<(Level, String)> = Vec::new();
        machine.step(focused, det_res, &cfg, &mut right, &mut logs);

        // ---- 写回共享状态 ----
        if let Ok(mut s) = shared.lock() {
            s.last_sim = det_res.sim;
            s.last_level_db = det_res.level_db;
            s.last_pan_db = det_res.pan_db;
            s.audio_level = level;
            s.game_focused = focused;
            s.foreground_exe = fg_exe;
            s.state = if s.paused { State::Stopped } else { machine.state };
            s.stats = machine.stats;
            if machine.state == State::Stopped {
                s.running = false;
            }
            for (lvl, txt) in logs {
                s.log.push(lvl, txt);
            }
        }

        std::thread::sleep(Duration::from_millis(15));
    }
}
