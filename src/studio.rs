//! 声学标定工坊：长音频录制、多录音档案库、多轮钓鱼切片、双端点区间标注与换饵时序标定。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32};
use serde::{Deserialize, Serialize};

use crate::audio::{Detector, StereoFrame, Template, INTERNAL_SR};
use crate::theme;

/// 钓鱼模式划分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FishingMode {
    Float, // 台钓
    Lure,  // 路亚
}

impl FishingMode {
    pub fn label(&self) -> &'static str {
        match self {
            FishingMode::Float => "台钓模式",
            FishingMode::Lure => "路亚模式",
        }
    }
}

/// 标注事件类型定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    Cast,       // 抛竿入水 / 抛竿落水
    Bite,       // 咬钩切水 (台钓)
    Hit,        // 咬饵顿口 (路亚)
    Retrieve,   // 收线搜寻 (路亚)
    Fight,      // 摆杆搏鱼 (路亚)
    Catch,      // 刺鱼钓获 / 起鱼钓获
    Miss,       // 脱钩空竿
    Snag,       // 挂底卡障 (路亚)
    Rebait,     // 换饵 / 上饵动作
    CastFailed, // 抛竿失败 / 无法抛竿
    Noise,      // 干扰杂音 / 旁人咬钩
    Custom(String),
}

impl EventType {
    pub fn display_name(&self, mode: FishingMode) -> &'static str {
        match (self, mode) {
            (EventType::Cast, FishingMode::Float) => "抛竿入水",
            (EventType::Cast, FishingMode::Lure) => "抛竿落水",
            (EventType::Bite, _) => "咬钩切水",
            (EventType::Hit, _) => "咬饵顿口",
            (EventType::Retrieve, _) => "收线搜寻",
            (EventType::Fight, _) => "摆杆搏鱼",
            (EventType::Catch, FishingMode::Float) => "刺鱼钓获",
            (EventType::Catch, FishingMode::Lure) => "起鱼钓获",
            (EventType::Miss, _) => "脱钩空竿",
            (EventType::Snag, _) => "挂底卡障",
            (EventType::Rebait, _) => "换饵/上饵",
            (EventType::CastFailed, _) => "抛竿失败",
            (EventType::Noise, _) => "干扰杂音",
            (EventType::Custom(_), _) => "自定义标记",
        }
    }

    pub fn color(&self) -> Color32 {
        match self {
            EventType::Cast => theme::CYAN,
            EventType::Bite | EventType::Hit => theme::MINT,
            EventType::Retrieve => theme::BLUE,
            EventType::Fight => Color32::from_rgb(0xa8, 0x55, 0xf7),
            EventType::Catch => Color32::from_rgb(0x34, 0xd3, 0x99),
            EventType::Miss | EventType::Snag => theme::RED,
            EventType::Rebait => Color32::from_rgb(0xfb, 0x92, 0x3c), // 亮橙色
            EventType::CastFailed => Color32::from_rgb(0xf4, 0x3f, 0x5e), // 警示红
            EventType::Noise => theme::AMBER,
            EventType::Custom(_) => Color32::from_rgb(0x94, 0xa3, 0xb8),
        }
    }
}

/// 时间线上的双端点标记区间。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMarker {
    pub id: u64,
    pub start_sec: f32,
    pub end_sec: f32,
    pub event_type: EventType,
    pub auto_detected: bool,
    pub confidence: f32,
    pub comment: String,
}

impl EventMarker {
    pub fn duration(&self) -> f32 {
        (self.end_sec - self.start_sec).max(0.0)
    }
}

/// 鼠标在时间线上拖拽手柄的目标状态。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragHandle {
    None,
    #[allow(dead_code)]
    Cursor,
    MarkerStart(u64),               // 拖拽调整起始点
    MarkerEnd(u64),                 // 拖拽调整结束点
    MarkerBody { id: u64, offset_sec: f32 }, // 整体平移
}

/// 单轮钓鱼切片（第 N 竿）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSlice {
    pub id: u64,
    pub title: String,
    pub start_sec: f32,
    pub end_sec: f32,
    pub has_rebait: bool,
}

impl AudioSlice {
    pub fn duration(&self) -> f32 {
        (self.end_sec - self.start_sec).max(0.0)
    }
}

/// 单次录音会话元数据（与 session_*.wav 配对保存为 session_*.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub mode: FishingMode,
    pub wav_file: String,
    pub sample_rate: u32,
    #[allow(dead_code)]
    pub duration_sec: f32,
    pub markers: Vec<EventMarker>,
    pub slices: Vec<AudioSlice>,
}

/// 历史录音档案条目（用于列表显示与切换）。
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub filename: String,
    pub wav_path: PathBuf,
    #[allow(dead_code)]
    pub json_path: PathBuf,
    pub timestamp_display: String,
    #[allow(dead_code)]
    pub duration_sec: f32,
    pub marker_count: usize,
    pub mode: FishingMode,
}

/// 内存音频录制与波形缓存。
#[derive(Clone, Default)]
pub struct AudioRecording {
    pub file_path: Option<PathBuf>,
    pub sample_rate: u32,
    #[allow(dead_code)]
    pub duration_sec: f32,
    pub frames: Vec<StereoFrame>,
    pub peaks: Vec<(f32, f32)>,
}

impl AudioRecording {
    pub fn from_frames(frames: Vec<StereoFrame>, sr: u32) -> Self {
        let dur = frames.len() as f32 / sr as f32;
        let chunk_sz = (sr / 100).max(1) as usize;
        let mut peaks = Vec::with_capacity(frames.len() / chunk_sz + 1);

        for chunk in frames.chunks(chunk_sz) {
            let mut mi = 0.0f32;
            let mut ma = 0.0f32;
            for f in chunk {
                let m = f.mono();
                if m < mi { mi = m; }
                if m > ma { ma = m; }
            }
            peaks.push((mi, ma));
        }

        Self {
            file_path: None,
            sample_rate: sr,
            duration_sec: dur,
            frames,
            peaks,
        }
    }

    pub fn from_wav_file(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败: {e}"))?;
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("解析 WAV 失败: {e}"))?;
        let spec = reader.spec();
        let sr = spec.sample_rate;
        let channels = spec.channels as usize;

        let raw_samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
            hound::SampleFormat::Int => match spec.bits_per_sample {
                16 => reader.samples::<i16>().filter_map(|s| s.ok()).map(|s| s as f32 / 32768.0).collect(),
                24 => reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / 8_388_608.0).collect(),
                _ => reader.samples::<i32>().filter_map(|s| s.ok()).map(|s| s as f32 / 2_147_483_648.0).collect(),
            },
        };

        let mut frames = Vec::with_capacity(raw_samples.len() / channels.max(1));
        if channels <= 1 {
            for &s in &raw_samples {
                frames.push(StereoFrame::new(s, s));
            }
        } else {
            for chunk in raw_samples.chunks(channels) {
                let l = chunk[0];
                let r = if chunk.len() > 1 { chunk[1] } else { l };
                frames.push(StereoFrame::new(l, r));
            }
        }

        let mut rec = Self::from_frames(frames, sr);
        rec.file_path = Some(path.to_path_buf());
        Ok(rec)
    }

    pub fn save_to_wav(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec)
            .map_err(|e| format!("创建 WAV 失败: {e}"))?;

        for f in &self.frames {
            let l = (f.l.clamp(-1.0, 1.0) * 32767.0) as i16;
            let r = (f.r.clamp(-1.0, 1.0) * 32767.0) as i16;
            writer.write_sample(l).map_err(|e| e.to_string())?;
            writer.write_sample(r).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| format!("写入 WAV 失败: {e}"))?;
        Ok(())
    }

    pub fn slice_to_wav_bytes(&self, start_sec: f32, duration_sec: f32) -> Result<Vec<u8>, String> {
        let sr = self.sample_rate as usize;
        let start_idx = ((start_sec * sr as f32) as usize).min(self.frames.len());
        let len = ((duration_sec * sr as f32) as usize).min(self.frames.len().saturating_sub(start_idx));
        let slice = &self.frames[start_idx..start_idx + len];

        let mut buf = std::io::Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut buf, spec)
            .map_err(|e| e.to_string())?;

        for f in slice {
            let l = (f.l.clamp(-1.0, 1.0) * 32767.0) as i16;
            let r = (f.r.clamp(-1.0, 1.0) * 32767.0) as i16;
            writer.write_sample(l).map_err(|e| e.to_string())?;
            writer.write_sample(r).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
        Ok(buf.into_inner())
    }
}

/// Windows 原生异步音频发声器与停止接口。
#[cfg(windows)]
pub fn play_sound_raw(bytes: &[u8]) {
    unsafe extern "system" {
        fn PlaySoundA(pszSound: *const u8, hmod: usize, fdwSound: u32) -> i32;
    }
    const SND_ASYNC: u32 = 0x0001;
    const SND_MEMORY: u32 = 0x0004;
    const SND_NODEFAULT: u32 = 0x0002;
    unsafe {
        PlaySoundA(bytes.as_ptr(), 0, SND_ASYNC | SND_MEMORY | SND_NODEFAULT);
    }
}

#[cfg(windows)]
pub fn stop_sound_raw() {
    unsafe extern "system" {
        fn PlaySoundA(pszSound: *const u8, hmod: usize, fdwSound: u32) -> i32;
    }
    unsafe {
        PlaySoundA(std::ptr::null(), 0, 0);
    }
}

#[cfg(not(windows))]
pub fn play_sound_raw(_bytes: &[u8]) {}
#[cfg(not(windows))]
pub fn stop_sound_raw() {}

/// 打开系统资源管理器文件夹。
pub fn open_folder(dir: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(dir.as_os_str())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
    }
}

/// 仿真评测报告。
#[derive(Debug, Clone, Default)]
pub struct SimReport {
    #[allow(dead_code)]
    pub total_hits: usize,
    pub bite_label_recalled: usize,
    pub total_bites_labeled: usize,
    pub noise_blocked: usize,
    pub total_noise_labeled: usize,
    pub recommended_threshold: f32,
    pub recommended_level_min: f32,
    pub recommended_pan_max: f32,
}

/// 声学工坊全局状态。
pub struct StudioState {
    pub mode: FishingMode,
    pub recording: bool,
    pub record_start: Option<Instant>,
    pub live_buffer: Arc<Mutex<Vec<StereoFrame>>>,
    pub current_recording: Option<AudioRecording>,
    pub current_session_id: String,
    pub markers: Vec<EventMarker>,
    pub next_marker_id: u64,
    /// 选中的标记 ID
    pub selected_marker_id: Option<u64>,
    /// 手柄拖拽状态
    pub drag_handle: DragHandle,
    /// 多轮钓鱼切片集合
    pub slices: Vec<AudioSlice>,
    pub next_slice_id: u64,
    pub active_slice_id: Option<u64>,
    /// 历史录音档案库
    pub available_sessions: Vec<SessionEntry>,
    /// 时间线游标位置（秒）
    pub cursor_sec: f32,
    /// 选区起始点与结束点
    pub selection_start_sec: Option<f32>,
    pub selection_end_sec: Option<f32>,
    /// 时间线视口（起始秒，显示时长秒）
    pub view_start_sec: f32,
    pub view_duration_sec: f32,
    /// 播放控制器状态
    pub is_playing: bool,
    pub play_start_cursor: f32,
    pub play_end_cursor: f32,
    pub play_start_time: Option<Instant>,
    pub playing_audio_bytes: Option<Arc<Vec<u8>>>,
    pub loop_playback: bool,
    pub sim_report: Option<SimReport>,
    pub status_msg: String,
}

impl Default for StudioState {
    fn default() -> Self {
        let mut s = Self {
            mode: FishingMode::Float,
            recording: false,
            record_start: None,
            live_buffer: Arc::new(Mutex::new(Vec::new())),
            current_recording: None,
            current_session_id: String::new(),
            markers: Vec::new(),
            next_marker_id: 1,
            selected_marker_id: None,
            drag_handle: DragHandle::None,
            slices: Vec::new(),
            next_slice_id: 1,
            active_slice_id: None,
            available_sessions: Vec::new(),
            cursor_sec: 0.0,
            selection_start_sec: None,
            selection_end_sec: None,
            view_start_sec: 0.0,
            view_duration_sec: 30.0,
            is_playing: false,
            play_start_cursor: 0.0,
            play_end_cursor: 0.0,
            play_start_time: None,
            playing_audio_bytes: None,
            loop_playback: false,
            sim_report: None,
            status_msg: "就绪。按 [Space 空格] 播放/暂停，按 [← / →] 微调游标，按 [1-6] 快速打标。".into(),
        };
        s.refresh_available_recordings();
        s
    }
}

impl StudioState {
    pub fn dataset_dir() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("DeltaFishing").join("dataset")
    }

    /// 刷新历史录音档案库。
    pub fn refresh_available_recordings(&mut self) {
        let dir = Self::dataset_dir();
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }

        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("wav") {
                    let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let json_path = path.with_extension("json");

                    // 尝试读取元数据
                    let mut dur = 0.0f32;
                    let mut cnt = 0usize;
                    let mut mode = FishingMode::Float;

                    if let Ok(text) = std::fs::read_to_string(&json_path) {
                        if let Ok(meta) = serde_json::from_str::<SessionMetadata>(&text) {
                            dur = meta.duration_sec;
                            cnt = meta.markers.len();
                            mode = meta.mode;
                        }
                    } else if let Ok(rec) = AudioRecording::from_wav_file(&path) {
                        dur = rec.duration_sec;
                    }

                    // 格式化时间戳
                    let ts_display = if fname.starts_with("session_") && fname.len() >= 23 {
                        let y = &fname[8..12];
                        let m = &fname[12..14];
                        let d = &fname[14..16];
                        let hh = &fname[17..19];
                        let mm = &fname[19..21];
                        format!("{y}-{m}-{d} {hh}:{mm}")
                    } else {
                        fname.clone()
                    };

                    entries.push(SessionEntry {
                        filename: fname,
                        wav_path: path,
                        json_path,
                        timestamp_display: ts_display,
                        duration_sec: dur,
                        marker_count: cnt,
                        mode,
                    });
                }
            }
        }

        // 按文件名降序（最新在上）
        entries.sort_by(|a, b| b.filename.cmp(&a.filename));
        self.available_sessions = entries;

        // 如果当前没有载入且存在录音，自动载入最新的那条
        if self.current_recording.is_none() && !self.available_sessions.is_empty() {
            let latest_path = self.available_sessions[0].wav_path.clone();
            self.load_session(&latest_path);
        }
    }

    /// 载入指定的历史录音与标记。
    pub fn load_session(&mut self, wav_path: &Path) {
        self.stop_playback();
        match AudioRecording::from_wav_file(wav_path) {
            Ok(rec) => {
                let json_path = wav_path.with_extension("json");
                let mut markers = Vec::new();
                let mut slices = Vec::new();
                let mut mode = self.mode;
                let fname = wav_path.file_name().unwrap_or_default().to_string_lossy().to_string();

                if let Ok(text) = std::fs::read_to_string(&json_path) {
                    if let Ok(meta) = serde_json::from_str::<SessionMetadata>(&text) {
                        markers = meta.markers;
                        slices = meta.slices;
                        mode = meta.mode;
                    }
                }

                let max_id = markers.iter().map(|m| m.id).max().unwrap_or(0);
                self.next_marker_id = max_id + 1;
                let max_sl_id = slices.iter().map(|s| s.id).max().unwrap_or(0);
                self.next_slice_id = max_sl_id + 1;

                let dur = rec.duration_sec;
                self.current_recording = Some(rec);
                self.current_session_id = fname.clone();
                self.mode = mode;
                self.markers = markers;
                self.slices = slices;
                self.active_slice_id = None;
                self.selected_marker_id = None;
                self.cursor_sec = 0.0;
                self.view_start_sec = 0.0;
                self.view_duration_sec = dur.min(45.0).max(10.0);
                self.status_msg = format!("已载入历史录音：{fname}（时长 {:.1}s · {}个标记）", dur, self.markers.len());

                // 若暂无切片，自动分切
                if self.slices.is_empty() {
                    self.auto_slice_rounds();
                }
            }
            Err(e) => {
                self.status_msg = format!("载入录音失败: {e}");
            }
        }
    }

    /// 将当前会话元数据（标记与切片）保存至配套的 JSON 文件中。
    pub fn save_current_session(&self) {
        if let Some(rec) = &self.current_recording {
            if let Some(wav_path) = &rec.file_path {
                let json_path = wav_path.with_extension("json");
                let meta = SessionMetadata {
                    session_id: self.current_session_id.clone(),
                    mode: self.mode,
                    wav_file: wav_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    sample_rate: rec.sample_rate,
                    duration_sec: rec.duration_sec,
                    markers: self.markers.clone(),
                    slices: self.slices.clone(),
                };
                if let Ok(text) = serde_json::to_string_pretty(&meta) {
                    let _ = std::fs::write(json_path, text);
                }
            }
        }
    }

    /// 删除指定会话。
    pub fn delete_session(&mut self, wav_path: &Path) {
        self.stop_playback();
        let _ = std::fs::remove_file(wav_path);
        let _ = std::fs::remove_file(wav_path.with_extension("json"));
        self.current_recording = None;
        self.markers.clear();
        self.slices.clear();
        self.refresh_available_recordings();
    }

    pub fn start_playback(&mut self) {
        let rec = match &self.current_recording {
            Some(r) if !r.frames.is_empty() => r,
            _ => return,
        };

        let (s, e) = match (self.selection_start_sec, self.selection_end_sec) {
            (Some(a), Some(b)) if (b - a).abs() >= 0.05 => (a.min(b), a.max(b)),
            _ => (self.cursor_sec, rec.duration_sec),
        };

        if e <= s {
            return;
        }

        let dur = e - s;
        if let Ok(bytes) = rec.slice_to_wav_bytes(s, dur) {
            let arc_bytes = Arc::new(bytes);
            play_sound_raw(&arc_bytes);
            self.playing_audio_bytes = Some(arc_bytes);
            self.is_playing = true;
            self.cursor_sec = s;
            self.play_start_cursor = s;
            self.play_end_cursor = e;
            self.play_start_time = Some(Instant::now());
        }
    }

    pub fn stop_playback(&mut self) {
        stop_sound_raw();
        self.is_playing = false;
        self.play_start_time = None;
        self.playing_audio_bytes = None;
    }

    pub fn toggle_playback(&mut self) {
        if self.is_playing {
            self.stop_playback();
        } else {
            self.start_playback();
        }
    }

    pub fn start_recording(&mut self, device_name: &str) -> Result<(), String> {
        if self.recording {
            return Ok(());
        }
        self.stop_playback();
        self.live_buffer.lock().unwrap().clear();
        let buf_arc = self.live_buffer.clone();
        let dev = device_name.to_string();

        std::thread::Builder::new()
            .name("studio-recorder".into())
            .spawn(move || {
                let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
                if let Ok(capture) = crate::audio::LoopbackCapture::start(&dev, queue.clone()) {
                    while let Ok(q) = queue.lock() {
                        if !buf_arc.is_poisoned() {
                            let mut main_buf = buf_arc.lock().unwrap();
                            drop(q);
                            if let Ok(mut q_drain) = queue.lock() {
                                main_buf.extend(q_drain.drain(..));
                            }
                        }
                        std::thread::sleep(Duration::from_millis(25));
                        let _ = &capture;
                    }
                }
            })
            .map_err(|e| format!("启动录制失败: {e}"))?;

        self.recording = true;
        self.record_start = Some(Instant::now());
        self.status_msg = "正在录音... 请在游戏中钓多竿，录完可切片、试听并标定换饵时序。".into();
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<(), String> {
        if !self.recording {
            return Ok(());
        }
        self.recording = false;
        let elapsed = self.record_start.take().map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);

        let frames = {
            let mut buf = self.live_buffer.lock().unwrap();
            let data = buf.clone();
            buf.clear();
            data
        };

        if frames.is_empty() {
            self.status_msg = "未采集到声音数据，请检查音频回环设备。".into();
            return Err("数据为空".into());
        }

        let recording = AudioRecording::from_frames(frames, INTERNAL_SR);
        let dir = Self::dataset_dir();
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let wav_name = format!("session_{ts}.wav");
        let wav_path = dir.join(&wav_name);
        let _ = recording.save_to_wav(&wav_path);

        self.current_recording = Some(recording);
        self.current_session_id = wav_name.clone();
        self.cursor_sec = 0.0;
        self.view_start_sec = 0.0;
        self.view_duration_sec = elapsed.min(45.0).max(10.0);
        self.selection_start_sec = None;
        self.selection_end_sec = None;
        self.slices.clear();
        self.active_slice_id = None;
        self.selected_marker_id = None;

        self.auto_prelabel();
        self.auto_slice_rounds();
        self.save_current_session();
        self.refresh_available_recordings();

        self.status_msg = format!("录音完成（时长 {:.1}s），已切出 {} 轮钓鱼切片，按 [空格键] 可试听播放！", elapsed, self.slices.len());
        Ok(())
    }

    pub fn auto_slice_rounds(&mut self) {
        let rec = match &self.current_recording {
            Some(r) if !r.frames.is_empty() => r,
            _ => return,
        };

        let total_dur = rec.duration_sec;
        let mut slices = Vec::new();

        let mut split_points: Vec<f32> = self.markers.iter()
            .filter(|m| matches!(m.event_type, EventType::Cast))
            .map(|m| m.start_sec)
            .collect();

        if split_points.is_empty() {
            split_points = self.markers.iter()
                .filter(|m| matches!(m.event_type, EventType::Bite | EventType::Hit))
                .map(|m| (m.start_sec - 8.0).max(0.0))
                .collect();
        }

        split_points.sort_by(|a, b| a.partial_cmp(b).unwrap());
        split_points.dedup_by(|a, b| (*a - *b).abs() < 10.0);

        if split_points.len() <= 1 {
            slices.push(AudioSlice {
                id: 1,
                title: format!("完整录音 (00:00 ~ {:02.0}:{:02.0})", (total_dur / 60.0).floor(), total_dur % 60.0),
                start_sec: 0.0,
                end_sec: total_dur,
                has_rebait: false,
            });
        } else {
            for i in 0..split_points.len() {
                let s = split_points[i];
                let e = if i + 1 < split_points.len() { split_points[i + 1] } else { total_dur };
                let dur = e - s;
                if dur >= 6.0 {
                    let has_rebait = self.markers.iter().any(|m| m.event_type == EventType::Rebait && m.start_sec >= s && m.end_sec <= e);
                    let tag_suffix = if has_rebait { " [含换饵]" } else { "" };
                    slices.push(AudioSlice {
                        id: (i + 1) as u64,
                        title: format!("第 {} 竿 (00:{:02.0} ~ 00:{:02.0}){}", i + 1, s, e, tag_suffix),
                        start_sec: s,
                        end_sec: e,
                        has_rebait,
                    });
                }
            }
        }

        self.slices = slices;
        self.next_slice_id = self.slices.len() as u64 + 1;
        self.save_current_session();
    }

    pub fn slice_current_selection(&mut self) {
        if let (Some(a), Some(b)) = (self.selection_start_sec, self.selection_end_sec) {
            let (s, e) = (a.min(b), a.max(b));
            if (e - s) >= 3.0 {
                let id = self.next_slice_id;
                self.next_slice_id += 1;
                let has_rebait = self.markers.iter().any(|m| m.event_type == EventType::Rebait && m.start_sec >= s && m.end_sec <= e);
                let title = format!("切片 #{} (00:{:02.0} ~ 00:{:02.0})", id, s, e);

                self.slices.push(AudioSlice {
                    id,
                    title,
                    start_sec: s,
                    end_sec: e,
                    has_rebait,
                });
                self.active_slice_id = Some(id);
                self.status_msg = format!("已将选区切片保存为独立切片片段（时长 {:.1}s）！", e - s);
                self.save_current_session();
            }
        }
    }

    pub fn focus_slice(&mut self, slice_id: u64) {
        if let Some(sl) = self.slices.iter().find(|s| s.id == slice_id) {
            self.active_slice_id = Some(slice_id);
            self.view_start_sec = sl.start_sec;
            self.view_duration_sec = sl.duration().max(5.0);
            self.cursor_sec = sl.start_sec;
            self.status_msg = format!("已聚焦视口到切片：{}", sl.title);
        }
    }

    pub fn auto_prelabel(&mut self) {
        let rec = match &self.current_recording {
            Some(r) if !r.frames.is_empty() => r,
            _ => return,
        };

        let template_bytes = include_bytes!("../assets/bite.wav");
        let template = match Template::from_bytes(template_bytes, crate::audio::TEMPLATE_LEN) {
            Ok(t) => t,
            Err(_) => return,
        };

        let mut detector = Detector::new(template);
        let mut auto_markers = Vec::new();
        let step_frames = (INTERNAL_SR as f32 * 0.02) as usize;
        let mut t_idx = 0usize;

        let mut in_burst = false;
        let mut burst_start = 0.0f32;
        let mut peak_sim = 0.0f32;
        let mut peak_pan = 0.0f32;

        while t_idx + step_frames < rec.frames.len() {
            let slice = &rec.frames[t_idx..t_idx + step_frames];
            let det = detector.push(slice);
            let cur_time = t_idx as f32 / INTERNAL_SR as f32;

            if det.sim >= 0.50 {
                if !in_burst {
                    in_burst = true;
                    burst_start = (cur_time - 0.05).max(0.0);
                    peak_sim = det.sim;
                    peak_pan = det.pan_db;
                } else if det.sim > peak_sim {
                    peak_sim = det.sim;
                    peak_pan = det.pan_db;
                }
            } else if in_burst {
                in_burst = false;
                let burst_end = (cur_time + 0.05).min(rec.duration_sec);
                let dur = burst_end - burst_start;

                if dur >= 0.15 && dur <= 0.60 {
                    let (evt, comment) = if peak_pan.abs() <= 5.0 && peak_sim >= 0.60 {
                        let e = match self.mode {
                            FishingMode::Float => EventType::Bite,
                            FishingMode::Lure => EventType::Hit,
                        };
                        (e, format!("自动初筛：正前咬钩 ({:.0}% · {:+.1}dB)", peak_sim * 100.0, peak_pan))
                    } else {
                        (EventType::Noise, format!("自动初筛：疑似他人/杂音 ({:+.1}dB)", peak_pan))
                    };

                    auto_markers.push(EventMarker {
                        id: self.next_marker_id,
                        start_sec: burst_start,
                        end_sec: burst_end,
                        event_type: evt,
                        auto_detected: true,
                        confidence: peak_sim,
                        comment,
                    });
                    self.next_marker_id += 1;
                }
            }

            t_idx += step_frames;
        }

        self.markers = auto_markers;
        self.markers.sort_by(|a, b| a.start_sec.partial_cmp(&b.start_sec).unwrap());
        self.save_current_session();
    }

    pub fn add_or_update_interval_marker(&mut self, event_type: EventType) {
        let (s, e) = match (self.selection_start_sec, self.selection_end_sec) {
            (Some(a), Some(b)) if (b - a).abs() >= 0.05 => (a.min(b), a.max(b)),
            _ => {
                let dur = match event_type {
                    EventType::Bite | EventType::Hit => 0.30,
                    EventType::Cast => 0.40,
                    EventType::Catch => 0.50,
                    EventType::Rebait => 1.50,
                    EventType::CastFailed => 0.40,
                    EventType::Retrieve | EventType::Fight => 1.00,
                    _ => 0.25,
                };
                let s = self.cursor_sec;
                let max_dur = self.current_recording.as_ref().map(|r| r.duration_sec).unwrap_or(s + dur);
                (s, (s + dur).min(max_dur))
            }
        };

        let id = self.next_marker_id;
        self.next_marker_id += 1;

        self.markers.push(EventMarker {
            id,
            start_sec: s,
            end_sec: e,
            event_type,
            auto_detected: false,
            confidence: 1.0,
            comment: "人工标记".into(),
        });

        self.selected_marker_id = Some(id);
        self.selection_start_sec = None;
        self.selection_end_sec = None;
        self.markers.sort_by(|a, b| a.start_sec.partial_cmp(&b.start_sec).unwrap());
        self.save_current_session();
    }

    pub fn export_marker_as_template(&mut self, marker_id: u64) -> Result<PathBuf, String> {
        let marker = self.markers.iter().find(|m| m.id == marker_id)
            .ok_or_else(|| "未找到标记".to_string())?;

        let rec = self.current_recording.as_ref()
            .ok_or_else(|| "当前未加载录音".to_string())?;

        let dur = marker.duration();
        let bytes = rec.slice_to_wav_bytes(marker.start_sec, dur)?;

        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let tpl_path = base.join("DeltaFishing").join("custom_bite.wav");

        std::fs::write(&tpl_path, bytes).map_err(|e| format!("保存模板失败: {e}"))?;
        self.status_msg = format!("★ 已成功将标记切片（{:.0}ms）导出为专属模板！", dur * 1000.0);
        Ok(tpl_path)
    }

    pub fn run_simulation(&mut self, cfg: &crate::config::Config) {
        let rec = match &self.current_recording {
            Some(r) if !r.frames.is_empty() => r,
            _ => {
                self.status_msg = "请先录制或加载一段音频再运行仿真。".into();
                return;
            }
        };

        let template_bytes = include_bytes!("../assets/bite.wav");
        let template = match Template::from_bytes(template_bytes, crate::audio::TEMPLATE_LEN) {
            Ok(t) => t,
            Err(e) => {
                self.status_msg = format!("加载模板失败: {e}");
                return;
            }
        };

        let mut detector = Detector::new(template);
        let mut detections_count = 0;
        let step_frames = (INTERNAL_SR as f32 * 0.02) as usize;

        let thr = cfg.bite_threshold / 100.0;
        let mut t_idx = 0usize;
        let mut hit_times = Vec::new();

        while t_idx + step_frames < rec.frames.len() {
            let slice = &rec.frames[t_idx..t_idx + step_frames];
            let det = detector.push(slice);
            let time_sec = t_idx as f32 / INTERNAL_SR as f32;

            if det.sim >= thr {
                let level_ok = det.level_db >= cfg.level_min_db
                    && (cfg.level_max_db == 0.0 || det.level_db <= cfg.level_max_db);
                let pan_ok = det.pan_db.abs() <= cfg.pan_max_db;
                if level_ok && pan_ok {
                    detections_count += 1;
                    hit_times.push(time_sec);
                }
            }
            t_idx += step_frames;
        }

        let bite_markers: Vec<&EventMarker> = self.markers.iter()
            .filter(|m| matches!(m.event_type, EventType::Bite | EventType::Hit))
            .collect();
        let noise_markers: Vec<&EventMarker> = self.markers.iter()
            .filter(|m| matches!(m.event_type, EventType::Noise))
            .collect();

        let mut bites_hit = 0;
        for bm in &bite_markers {
            if hit_times.iter().any(|&t| t >= (bm.start_sec - 0.25) && t <= (bm.end_sec + 0.25)) {
                bites_hit += 1;
            }
        }

        let mut noise_blocked = 0;
        for nm in &noise_markers {
            if !hit_times.iter().any(|&t| t >= (nm.start_sec - 0.2) && t <= (nm.end_sec + 0.2)) {
                noise_blocked += 1;
            }
        }

        let report = SimReport {
            total_hits: detections_count,
            bite_label_recalled: bites_hit,
            total_bites_labeled: bite_markers.len(),
            noise_blocked,
            total_noise_labeled: noise_markers.len(),
            recommended_threshold: 65.0,
            recommended_level_min: -26.0,
            recommended_pan_max: 4.5,
        };

        self.status_msg = format!(
            "全切片评测完成！咬钩召回: {}/{}，杂音/他人拦截: {}/{}",
            report.bite_label_recalled, report.total_bites_labeled,
            report.noise_blocked, report.total_noise_labeled
        );
        self.sim_report = Some(report);
    }
}

/// 渲染波形时间线画布，支持播放时游标平滑走动与手柄拖拽微调。
pub fn render_timeline(ui: &mut egui::Ui, state: &mut StudioState) {
    if state.current_recording.is_none() {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 160.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, egui::CornerRadius::same(8), Color32::from_black_alpha(60));
        ui.painter().rect_stroke(rect, egui::CornerRadius::same(8), egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Middle);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "暂无录音数据 · 点击上方「开始录音」开始实况采集",
            egui::FontId::proportional(13.0),
            theme::TEXT_MUTED,
        );
        return;
    }

    let mut should_loop = false;
    let mut should_stop = false;
    if state.is_playing {
        if let Some(start_t) = state.play_start_time {
            let elapsed = start_t.elapsed().as_secs_f32();
            state.cursor_sec = state.play_start_cursor + elapsed;
            ui.ctx().request_repaint();

            if state.cursor_sec >= state.play_end_cursor {
                if state.loop_playback {
                    should_loop = true;
                } else {
                    should_stop = true;
                }
            }
        }
    }
    if should_loop {
        state.start_playback();
    } else if should_stop {
        state.stop_playback();
    }

    let rec = state.current_recording.as_ref().unwrap();
    let total_dur = rec.duration_sec.max(1.0);
    let view_start = state.view_start_sec.clamp(0.0, total_dur);
    let view_dur = state.view_duration_sec.clamp(2.0, total_dur);
    let view_end = (view_start + view_dur).min(total_dur);

    let height = 150.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::click_and_drag());
    let painter = ui.painter();

    painter.rect_filled(rect, egui::CornerRadius::same(8), Color32::from_black_alpha(80));
    painter.rect_stroke(rect, egui::CornerRadius::same(8), egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Middle);

    let ruler_h = 20.0;
    let wave_top = rect.top() + ruler_h;
    let wave_h = rect.height() - ruler_h;
    let cy = wave_top + wave_h / 2.0;

    // 时间刻度标尺
    painter.line_segment(
        [egui::pos2(rect.left(), wave_top), egui::pos2(rect.right(), wave_top)],
        egui::Stroke::new(1.0, Color32::from_white_alpha(25)),
    );

    let step_sec = if view_dur <= 5.0 { 0.5 } else if view_dur <= 15.0 { 1.0 } else if view_dur <= 40.0 { 2.0 } else { 5.0 };
    let mut t_mark = (view_start / step_sec).floor() * step_sec;
    while t_mark <= view_end {
        if t_mark >= view_start {
            let x = rect.left() + (t_mark - view_start) / (view_end - view_start) * rect.width();
            painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, wave_top)], egui::Stroke::new(1.0, Color32::from_white_alpha(35)));
            let txt = format!("{:02.0}:{:04.1}", (t_mark / 60.0).floor(), t_mark % 60.0);
            painter.text(egui::pos2(x + 3.0, rect.top() + 3.0), egui::Align2::LEFT_TOP, txt, egui::FontId::monospace(9.5), theme::TEXT_MUTED);
        }
        t_mark += step_sec;
    }

    // 绘制波形能量包络
    let n_peaks = rec.peaks.len();
    if n_peaks > 0 {
        let p_start_idx = ((view_start / total_dur) * n_peaks as f32) as usize;
        let p_end_idx = (((view_end / total_dur) * n_peaks as f32) as usize).min(n_peaks);
        let n_visible = (p_end_idx.saturating_sub(p_start_idx)).max(1);
        let w = rect.width();
        let amp_scale = wave_h * 0.45;

        for (i, p) in rec.peaks[p_start_idx..p_end_idx].iter().enumerate() {
            let x = rect.left() + (i as f32 / n_visible as f32) * w;
            let y_top = cy - p.1.abs().clamp(0.0, 1.0) * amp_scale;
            let y_bot = cy + p.0.abs().clamp(0.0, 1.0) * amp_scale;
            painter.line_segment(
                [egui::pos2(x, y_top), egui::pos2(x, y_bot.max(y_top + 1.0))],
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(56, 189, 248, 140)),
            );
        }
    }

    // 绘制选区 (Selection)
    if let (Some(sa), Some(sb)) = (state.selection_start_sec, state.selection_end_sec) {
        let (s_min, s_max) = (sa.min(sb), sa.max(sb));
        if s_max >= view_start && s_min <= view_end {
            let sx1 = rect.left() + (s_min.max(view_start) - view_start) / (view_end - view_start) * rect.width();
            let sx2 = rect.left() + (s_max.min(view_end) - view_start) / (view_end - view_start) * rect.width();
            let sel_rect = egui::Rect::from_min_max(egui::pos2(sx1, wave_top), egui::pos2(sx2, rect.bottom()));
            painter.rect_filled(sel_rect, egui::CornerRadius::ZERO, Color32::from_rgba_unmultiplied(251, 191, 36, 40));
            painter.rect_stroke(sel_rect, egui::CornerRadius::ZERO, egui::Stroke::new(1.0, theme::AMBER), egui::StrokeKind::Middle);
        }
    }

    let mouse_pos = ui.input(|i| i.pointer.hover_pos());
    let mut hover_cursor = None;

    // 绘制双端点标记区间 (带起始手柄与结束手柄)
    for m in &state.markers {
        if m.end_sec >= view_start && m.start_sec <= view_end {
            let sx = rect.left() + (m.start_sec.max(view_start) - view_start) / (view_end - view_start) * rect.width();
            let ex = rect.left() + (m.end_sec.min(view_end) - view_start) / (view_end - view_start) * rect.width();
            let is_selected = state.selected_marker_id == Some(m.id);
            let col = m.event_type.color();

            // 区间填充
            let span_rect = egui::Rect::from_min_max(egui::pos2(sx, wave_top), egui::pos2(ex.max(sx + 4.0), rect.bottom()));
            let alpha = if is_selected { 70 } else { 35 };
            painter.rect_filled(span_rect, egui::CornerRadius::ZERO, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha));
            if is_selected {
                painter.rect_stroke(span_rect, egui::CornerRadius::ZERO, egui::Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Middle);
            }

            // 左边缘手柄
            let left_handle_rect = egui::Rect::from_min_max(egui::pos2(sx - 4.0, wave_top), egui::pos2(sx + 4.0, rect.bottom()));
            let is_hover_left = mouse_pos.map(|p| left_handle_rect.contains(p)).unwrap_or(false);
            let left_col = if is_hover_left { Color32::WHITE } else { col };
            painter.line_segment([egui::pos2(sx, wave_top), egui::pos2(sx, rect.bottom())], egui::Stroke::new(if is_hover_left { 2.5 } else { 1.5 }, left_col));
            painter.circle_filled(egui::pos2(sx, wave_top + 4.0), 3.0, left_col);

            // 右边缘手柄
            let right_handle_rect = egui::Rect::from_min_max(egui::pos2(ex - 4.0, wave_top), egui::pos2(ex + 4.0, rect.bottom()));
            let is_hover_right = mouse_pos.map(|p| right_handle_rect.contains(p)).unwrap_or(false);
            let right_col = if is_hover_right { Color32::WHITE } else { col };
            painter.line_segment([egui::pos2(ex, wave_top), egui::pos2(ex, rect.bottom())], egui::Stroke::new(if is_hover_right { 2.5 } else { 1.5 }, right_col));
            painter.circle_filled(egui::pos2(ex, wave_top + 4.0), 3.0, right_col);

            if is_hover_left || is_hover_right {
                hover_cursor = Some(egui::CursorIcon::ResizeHorizontal);
            }

            // 顶部事件标签胶囊 (带持续毫秒数)
            let dur_ms = m.duration() * 1000.0;
            let badge_txt = format!("{} ({:.0}ms)", m.event_type.display_name(state.mode), dur_ms);
            let flag_w = (badge_txt.chars().count() as f32 * 11.0 + 10.0).max(45.0);
            let flag_rect = egui::Rect::from_min_size(egui::pos2(sx, rect.top() + 1.0), egui::vec2(flag_w, 16.0));
            painter.rect_filled(flag_rect, egui::CornerRadius::same(3), Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 220));
            painter.text(flag_rect.center(), egui::Align2::CENTER_CENTER, badge_txt, egui::FontId::proportional(10.0), Color32::BLACK);
        }
    }

    if let Some(c) = hover_cursor {
        ui.ctx().set_cursor_icon(c);
    }

    let to_time = |x: f32| -> f32 {
        let ratio = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        view_start + ratio * (view_end - view_start)
    };

    // 鼠标手柄交互
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let mut hit_handle = DragHandle::None;

            for m in state.markers.iter().rev() {
                let sx = rect.left() + (m.start_sec - view_start) / (view_end - view_start) * rect.width();
                let ex = rect.left() + (m.end_sec - view_start) / (view_end - view_start) * rect.width();

                if (pos.x - sx).abs() <= 6.0 {
                    hit_handle = DragHandle::MarkerStart(m.id);
                    state.selected_marker_id = Some(m.id);
                    break;
                } else if (pos.x - ex).abs() <= 6.0 {
                    hit_handle = DragHandle::MarkerEnd(m.id);
                    state.selected_marker_id = Some(m.id);
                    break;
                } else if pos.x > sx && pos.x < ex && pos.y > wave_top {
                    let cur_t = to_time(pos.x);
                    hit_handle = DragHandle::MarkerBody { id: m.id, offset_sec: cur_t - m.start_sec };
                    state.selected_marker_id = Some(m.id);
                    break;
                }
            }

            if hit_handle == DragHandle::None {
                let t = to_time(pos.x);
                state.selection_start_sec = Some(t);
                state.selection_end_sec = Some(t);
                state.cursor_sec = t;
                state.selected_marker_id = None;
            }
            state.drag_handle = hit_handle;
        }
    } else if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = to_time(pos.x);
            match state.drag_handle {
                DragHandle::MarkerStart(id) => {
                    if let Some(m) = state.markers.iter_mut().find(|m| m.id == id) {
                        m.start_sec = t.clamp(0.0, m.end_sec - 0.02);
                    }
                }
                DragHandle::MarkerEnd(id) => {
                    if let Some(m) = state.markers.iter_mut().find(|m| m.id == id) {
                        m.end_sec = t.clamp(m.start_sec + 0.02, total_dur);
                    }
                }
                DragHandle::MarkerBody { id, offset_sec } => {
                    if let Some(m) = state.markers.iter_mut().find(|m| m.id == id) {
                        let dur = m.duration();
                        let new_start = (t - offset_sec).clamp(0.0, total_dur - dur);
                        m.start_sec = new_start;
                        m.end_sec = new_start + dur;
                    }
                }
                _ => {
                    state.selection_end_sec = Some(t);
                    state.cursor_sec = t;
                }
            }
        }
    } else if response.drag_stopped() {
        state.drag_handle = DragHandle::None;
        state.markers.sort_by(|a, b| a.start_sec.partial_cmp(&b.start_sec).unwrap());
        state.save_current_session();
    } else if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = to_time(pos.x);
            state.cursor_sec = t;
            let mut clicked_marker = None;
            for m in state.markers.iter().rev() {
                if t >= m.start_sec && t <= m.end_sec {
                    clicked_marker = Some(m.id);
                    break;
                }
            }
            state.selected_marker_id = clicked_marker;
            state.selection_start_sec = None;
            state.selection_end_sec = None;
        }
    }

    // 绘制游标针
    if state.cursor_sec >= view_start && state.cursor_sec <= view_end {
        let cx = rect.left() + (state.cursor_sec - view_start) / (view_end - view_start) * rect.width();
        painter.line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())], egui::Stroke::new(2.0, theme::MINT));
        let pt1 = egui::pos2(cx - 5.0, rect.top());
        let pt2 = egui::pos2(cx + 5.0, rect.top());
        let pt3 = egui::pos2(cx, rect.top() + 7.0);
        painter.add(egui::Shape::convex_polygon(vec![pt1, pt2, pt3], theme::MINT, egui::Stroke::NONE));
    }
}

/// 渲染声学标定工坊主面板（包含多录音档案库、多轮切片、媒体播放器、双排打标、快捷键与选中控制台）。
pub fn render_studio_panel(
    ui: &mut egui::Ui,
    state: &mut StudioState,
    cfg: &mut crate::config::Config,
    cfg_changed: &mut bool,
    current_device: &str,
) {
    let rec_dur = state.current_recording.as_ref().map(|r| r.duration_sec);

    // ==========================================
    // 全局基础快捷键监听（空格播放、方向键微移、数字键打标）
    // ==========================================
    if !ui.ctx().egui_wants_keyboard_input() {
        if ui.input(|i| i.key_pressed(egui::Key::Space)) {
            state.toggle_playback();
        }

        let shift = ui.input(|i| i.modifiers.shift);
        let step = if shift { 0.50 } else { 0.05 };
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            state.cursor_sec = (state.cursor_sec - step).max(0.0);
            state.selection_start_sec = None;
            state.selection_end_sec = None;
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            let max_t = rec_dur.unwrap_or(0.0);
            state.cursor_sec = (state.cursor_sec + step).min(max_t);
            state.selection_start_sec = None;
            state.selection_end_sec = None;
        }

        if ui.input(|i| i.key_pressed(egui::Key::Num1)) {
            state.add_or_update_interval_marker(EventType::Cast);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num2)) {
            let e = if state.mode == FishingMode::Float { EventType::Bite } else { EventType::Hit };
            state.add_or_update_interval_marker(e);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num3)) {
            state.add_or_update_interval_marker(EventType::Rebait);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num4)) {
            state.add_or_update_interval_marker(EventType::CastFailed);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num5)) {
            state.add_or_update_interval_marker(EventType::Catch);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num6)) {
            let e = if state.mode == FishingMode::Float { EventType::Miss } else { EventType::Snag };
            state.add_or_update_interval_marker(e);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Num0) || i.key_pressed(egui::Key::Num7)) {
            state.add_or_update_interval_marker(EventType::Noise);
        }

        if ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
            if let Some(sel_id) = state.selected_marker_id {
                state.markers.retain(|m| m.id != sel_id);
                state.selected_marker_id = None;
                state.save_current_session();
            }
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.selected_marker_id = None;
            state.selection_start_sec = None;
            state.selection_end_sec = None;
        }
    }

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    // ---- 1. 顶部模式与录音控制栏 ----
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("模式:").size(12.5).color(theme::TEXT_DIM));
                                let float_active = state.mode == FishingMode::Float;
                                if ui.selectable_label(float_active, egui::RichText::new("台钓模式").size(12.5).strong()).clicked() {
                                    state.mode = FishingMode::Float;
                                    state.save_current_session();
                                }
                                let lure_active = state.mode == FishingMode::Lure;
                                if ui.selectable_label(lure_active, egui::RichText::new("路亚模式").size(12.5).strong()).clicked() {
                                    state.mode = FishingMode::Lure;
                                    state.save_current_session();
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if state.recording {
                                        let elapsed = state.record_start.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                                        let btn = egui::Button::new(
                                            egui::RichText::new(format!("■ 停止录音 ({:.1}s)", elapsed))
                                                .size(13.0)
                                                .strong()
                                                .color(theme::RED),
                                        )
                                        .fill(Color32::from_rgba_unmultiplied(65, 24, 24, 230))
                                        .stroke(egui::Stroke::new(1.0, theme::RED))
                                        .corner_radius(egui::CornerRadius::same(6));
                                        if ui.add(btn).clicked() {
                                            let _ = state.stop_recording();
                                        }
                                    } else {
                                        let btn = egui::Button::new(
                                            egui::RichText::new("● 开始实机录音")
                                                .size(13.0)
                                                .strong()
                                                .color(Color32::from_rgb(10, 30, 16)),
                                        )
                                        .fill(theme::MINT)
                                        .corner_radius(egui::CornerRadius::same(6));
                                        if ui.add(btn).clicked() {
                                            let _ = state.start_recording(current_device);
                                        }
                                    }

                                    if state.current_recording.is_some() {
                                        if ui.button(egui::RichText::new("🤖 自动初筛预标").size(12.0).color(theme::CYAN)).clicked() {
                                            state.auto_prelabel();
                                        }
                                    }
                                });
                            });
                        });

                    ui.add_space(8.0);

                    // ---- 2. 历史录音档案库选择栏 (多录音管理) ----
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("📂 录音档案库:").size(12.0).color(theme::TEXT_DIM));

                                let current_title = if state.current_session_id.is_empty() {
                                    "（暂未选择录音）".to_string()
                                } else {
                                    format!("当前: {} ({:.1}s)", state.current_session_id, rec_dur.unwrap_or(0.0))
                                };

                                let mut to_load = None;
                                egui::ComboBox::from_id_salt("history-recordings-combo")
                                    .width(280.0)
                                    .selected_text(current_title)
                                    .show_ui(ui, |ui| {
                                        for entry in &state.available_sessions {
                                            let is_sel = state.current_session_id == entry.filename;
                                            let item_text = format!("{} ({} · {}个标记)", entry.timestamp_display, entry.mode.label(), entry.marker_count);
                                            if ui.selectable_label(is_sel, item_text).clicked() {
                                                to_load = Some(entry.wav_path.clone());
                                            }
                                        }
                                    });

                                if let Some(p) = to_load {
                                    state.load_session(&p);
                                }

                                if ui.small_button("📂 打开目录").on_hover_text("在资源管理器中查看录音与标注文件").clicked() {
                                    open_folder(&StudioState::dataset_dir());
                                }
                                if ui.small_button("🔄 刷新").clicked() {
                                    state.refresh_available_recordings();
                                }
                                if let Some(rec) = &state.current_recording {
                                    if let Some(p) = rec.file_path.clone() {
                                        if ui.small_button("🗑 删除当前").clicked() {
                                            state.delete_session(&p);
                                        }
                                    }
                                }
                            });
                        });

                    ui.add_space(8.0);

                    // ---- 3. 长录音钓鱼轮次切片栏 ----
                    if let Some(total_dur) = rec_dur {
                        egui::Frame::new()
                            .fill(theme::CARD)
                            .stroke(egui::Stroke::new(1.0, theme::LINE))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(egui::RichText::new("轮次切片:").size(12.0).color(theme::TEXT_DIM));

                                    let all_active = state.active_slice_id.is_none();
                                    if ui.selectable_label(all_active, format!("完整录音 ({:.1}s)", total_dur)).clicked() {
                                        state.active_slice_id = None;
                                        state.view_start_sec = 0.0;
                                        state.view_duration_sec = total_dur;
                                    }

                                    let mut to_focus = None;
                                    for sl in &state.slices {
                                        let is_sel = state.active_slice_id == Some(sl.id);
                                        if ui.selectable_label(is_sel, &sl.title).clicked() {
                                            to_focus = Some(sl.id);
                                        }
                                    }
                                    if let Some(id) = to_focus {
                                        state.focus_slice(id);
                                    }

                                    if state.selection_start_sec.is_some() {
                                        if ui.button(egui::RichText::new("✂ 选区存为切片").size(11.5).color(theme::AMBER)).clicked() {
                                            state.slice_current_selection();
                                        }
                                    }
                                    if ui.button(egui::RichText::new("⚡ 智能分切").size(11.5).color(theme::TEXT_MUTED)).clicked() {
                                        state.auto_slice_rounds();
                                    }
                                });
                            });
                        ui.add_space(6.0);
                    }

                    // ---- 4. 媒体播放控制工具栏 (Transport Bar) ----
                    if let Some(total_dur) = rec_dur {
                        egui::Frame::new()
                            .fill(theme::CARD)
                            .stroke(egui::Stroke::new(1.0, theme::LINE))
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui.button(egui::RichText::new("⏮ 归零").size(12.0)).on_hover_text("回到开头 (00:00.0)").clicked() {
                                        state.cursor_sec = 0.0;
                                        if state.is_playing {
                                            state.stop_playback();
                                        }
                                    }

                                    let play_btn_txt = if state.is_playing { "⏸ 暂停 (Space)" } else { "▶ 播放时间线 (Space)" };
                                    let play_btn_col = if state.is_playing { theme::AMBER } else { theme::MINT };
                                    let btn = egui::Button::new(egui::RichText::new(play_btn_txt).size(12.5).strong().color(play_btn_col))
                                        .fill(Color32::from_white_alpha(12))
                                        .corner_radius(egui::CornerRadius::same(6));
                                    if ui.add(btn).clicked() {
                                        state.toggle_playback();
                                    }

                                    if ui.button(egui::RichText::new("⏹ 停止").size(12.0)).clicked() {
                                        state.stop_playback();
                                    }

                                    ui.checkbox(&mut state.loop_playback, "循环");

                                    let cur_str = format!("{:02.0}:{:04.2}", (state.cursor_sec / 60.0).floor(), state.cursor_sec % 60.0);
                                    let tot_str = format!("{:02.0}:{:04.2}", (total_dur / 60.0).floor(), total_dur % 60.0);
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(format!("{cur_str} / {tot_str}")).monospace().size(13.0).strong().color(theme::CYAN));
                                    });
                                });
                            });
                        ui.add_space(4.0);
                    }

                    // ---- 5. 波形时间线画布 ----
                    render_timeline(ui, state);

                    // ---- 6. 视口缩放与平移微调条 ----
                    if let Some(total_dur) = rec_dur {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("时间线缩放:").size(11.5).color(theme::TEXT_MUTED));
                            ui.add(
                                egui::Slider::new(&mut state.view_duration_sec, 2.0..=total_dur.max(5.0))
                                    .custom_formatter(|val, _| format!("{:.2} s", val))
                                    .text("持续")
                            );
                            ui.add_space(10.0);
                            ui.label(egui::RichText::new("平移视口:").size(11.5).color(theme::TEXT_MUTED));
                            let max_pan = (total_dur - state.view_duration_sec).max(0.0);
                            ui.add(
                                egui::Slider::new(&mut state.view_start_sec, 0.0..=max_pan)
                                    .custom_formatter(|val, _| format!("{:.2} s", val))
                                    .text("起始")
                            );
                        });
                    }

                    ui.add_space(10.0);

                    // ---- 7. 当前选中标记专属控制卡 ----
                    let mut insp_delete = false;
                    let mut insp_preview = false;
                    let mut insp_export = false;

                    if let Some(sel_id) = state.selected_marker_id {
                        if let Some(idx) = state.markers.iter().position(|m| m.id == sel_id) {
                            let mut start_val = state.markers[idx].start_sec;
                            let mut end_val = state.markers[idx].end_sec;
                            let evt = state.markers[idx].event_type.clone();
                            let dur_ms = (end_val - start_val).max(0.0) * 1000.0;
                            let max_t = rec_dur.unwrap_or(end_val + 10.0);

                            egui::Frame::new()
                                .fill(Color32::from_rgba_unmultiplied(74, 222, 128, 16))
                                .stroke(egui::Stroke::new(1.0, theme::MINT))
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::same(12))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("🎯 选中标记:").strong().size(13.0).color(theme::MINT));
                                        ui.label(egui::RichText::new(evt.display_name(state.mode)).size(13.0).strong().color(evt.color()));
                                        ui.label(egui::RichText::new(format!("(区间: {:.0} ms)", dur_ms)).monospace().size(12.0).color(theme::CYAN));

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.button(egui::RichText::new("🗑 删除 [Del]").size(11.5).color(theme::RED)).clicked() {
                                                insp_delete = true;
                                            }
                                            if matches!(evt, EventType::Bite | EventType::Hit) {
                                                if ui.button(egui::RichText::new("★ 设为模板").size(11.5).color(theme::MINT)).clicked() {
                                                    insp_export = true;
                                                }
                                            }
                                            if ui.button(egui::RichText::new("▶ 播放此段").size(11.5).color(theme::TEXT)).clicked() {
                                                insp_preview = true;
                                            }
                                        });
                                    });

                                    ui.add_space(6.0);

                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("起点:").size(12.0).color(theme::TEXT_DIM));
                                        if ui.add(egui::DragValue::new(&mut start_val).speed(0.01).range(0.0..=end_val - 0.02).suffix(" s")).changed() {
                                            state.markers[idx].start_sec = start_val;
                                        }

                                        ui.add_space(10.0);
                                        ui.label(egui::RichText::new("终点:").size(12.0).color(theme::TEXT_DIM));
                                        if ui.add(egui::DragValue::new(&mut end_val).speed(0.01).range(start_val + 0.02..=max_t).suffix(" s")).changed() {
                                            state.markers[idx].end_sec = end_val;
                                        }

                                        ui.add_space(14.0);
                                        ui.label(egui::RichText::new("提示: 可在上方波形图直接拖拽左右手柄微调边界").size(11.0).color(theme::TEXT_MUTED));
                                    });
                                });
                            ui.add_space(8.0);
                        }
                    }

                    if insp_delete {
                        if let Some(sel_id) = state.selected_marker_id {
                            state.markers.retain(|m| m.id != sel_id);
                            state.selected_marker_id = None;
                            state.save_current_session();
                        }
                    }
                    if insp_preview {
                        state.start_playback();
                    }
                    if insp_export {
                        if let Some(sel_id) = state.selected_marker_id {
                            let _ = state.export_marker_as_template(sel_id);
                        }
                    }

                    // ---- 8. 双排区间打标工具栏 ----
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            let span_desc = match (state.selection_start_sec, state.selection_end_sec) {
                                (Some(a), Some(b)) if (b - a).abs() >= 0.05 => {
                                    format!("当前选区: {:02.0}:{:04.2} ~ {:02.0}:{:04.2} (持续 {:.0} ms)",
                                        (a.min(b) / 60.0).floor(), a.min(b) % 60.0,
                                        (a.max(b) / 60.0).floor(), a.max(b) % 60.0,
                                        (b - a).abs() * 1000.0)
                                }
                                _ => {
                                    format!("当前游标: {:02.0}:{:04.2} (按数字键 1-6 或点击按钮打标)",
                                        (state.cursor_sec / 60.0).floor(), state.cursor_sec % 60.0)
                                }
                            };

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("添加标记:").size(12.5).color(theme::TEXT_DIM));
                                ui.label(egui::RichText::new(span_desc).size(12.0).color(theme::AMBER));
                            });
                            ui.add_space(8.0);

                            // 第一排：常规垂钓流程
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new("常规动作:").size(12.0).strong().color(theme::TEXT));
                                match state.mode {
                                    FishingMode::Float => {
                                        if ui.button(egui::RichText::new("🎣 抛竿入水 [1]").size(12.0).color(theme::CYAN)).clicked() {
                                            state.add_or_update_interval_marker(EventType::Cast);
                                        }
                                        if ui.button(egui::RichText::new("⚡ 咬钩切水 [2]").size(12.0).color(theme::MINT).strong()).clicked() {
                                            state.add_or_update_interval_marker(EventType::Bite);
                                        }
                                        if ui.button(egui::RichText::new("🐟 刺鱼钓获 [5]").size(12.0).color(Color32::from_rgb(0x34, 0xd3, 0x99))).clicked() {
                                            state.add_or_update_interval_marker(EventType::Catch);
                                        }
                                        if ui.button(egui::RichText::new("💨 脱钩空竿 [6]").size(12.0).color(theme::RED)).clicked() {
                                            state.add_or_update_interval_marker(EventType::Miss);
                                        }
                                    }
                                    FishingMode::Lure => {
                                        if ui.button(egui::RichText::new("🎣 抛竿落水 [1]").size(12.0).color(theme::CYAN)).clicked() {
                                            state.add_or_update_interval_marker(EventType::Cast);
                                        }
                                        if ui.button(egui::RichText::new("🔄 收线搜寻").size(12.0).color(theme::BLUE)).clicked() {
                                            state.add_or_update_interval_marker(EventType::Retrieve);
                                        }
                                        if ui.button(egui::RichText::new("⚡ 咬饵顿口 [2]").size(12.0).color(theme::MINT).strong()).clicked() {
                                            state.add_or_update_interval_marker(EventType::Hit);
                                        }
                                        if ui.button(egui::RichText::new("🎣 摆杆搏鱼").size(12.0).color(Color32::from_rgb(0xa8, 0x55, 0xf7))).clicked() {
                                            state.add_or_update_interval_marker(EventType::Fight);
                                        }
                                        if ui.button(egui::RichText::new("🐟 起鱼钓获 [5]").size(12.0).color(Color32::from_rgb(0x34, 0xd3, 0x99))).clicked() {
                                            state.add_or_update_interval_marker(EventType::Catch);
                                        }
                                    }
                                }
                            });

                            ui.add_space(6.0);

                            // 第二排：换饵与异常排查
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new("换饵异常:").size(12.0).strong().color(Color32::from_rgb(0xfb, 0x92, 0x3c)));

                                let btn_rebait = egui::Button::new(
                                    egui::RichText::new("🪱 换饵/上饵 [3]").size(12.5).strong().color(Color32::from_rgb(0xfb, 0x92, 0x3c))
                                )
                                .fill(Color32::from_rgba_unmultiplied(251, 146, 60, 30))
                                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0xfb, 0x92, 0x3c)))
                                .corner_radius(egui::CornerRadius::same(6));
                                if ui.add(btn_rebait).on_hover_text("饵用光后双手穿饵动作区间（约 1.5s），用于测算精准换饵耗时").clicked() {
                                    state.add_or_update_interval_marker(EventType::Rebait);
                                }

                                let btn_fail = egui::Button::new(
                                    egui::RichText::new("⚠️ 抛竿失败 [4]").size(12.5).strong().color(Color32::from_rgb(0xf4, 0x3f, 0x5e))
                                )
                                .fill(Color32::from_rgba_unmultiplied(244, 63, 94, 30))
                                .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0xf4, 0x3f, 0x5e)))
                                .corner_radius(egui::CornerRadius::same(6));
                                if ui.add(btn_fail).on_hover_text("未换好饵或无饵时抛竿报错音，标定后系统可自动补抛重试").clicked() {
                                    state.add_or_update_interval_marker(EventType::CastFailed);
                                }

                                if state.mode == FishingMode::Lure {
                                    if ui.button(egui::RichText::new("🪨 挂底脱钩 [6]").size(12.0).color(theme::RED)).clicked() {
                                        state.add_or_update_interval_marker(EventType::Snag);
                                    }
                                }

                                if ui.button(egui::RichText::new("💥 干扰杂音 [0]").size(12.0).color(theme::AMBER)).clicked() {
                                    state.add_or_update_interval_marker(EventType::Noise);
                                }
                            });
                        });

                    ui.add_space(8.0);

                    // ---- 9. 双端点标记区间列表 ----
                    egui::Frame::new()
                        .fill(theme::CARD)
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("标记区间列表 ({})", state.markers.len())).strong().size(13.5).color(theme::TEXT));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if !state.markers.is_empty() && ui.small_button("清空所有标记").clicked() {
                                        state.markers.clear();
                                        state.selected_marker_id = None;
                                        state.save_current_session();
                                    }
                                    if state.current_recording.is_some() && ui.button(egui::RichText::new("🔍 全切片联合仿真跑分").size(12.0).color(theme::CYAN)).clicked() {
                                        state.run_simulation(cfg);
                                    }
                                });
                            });
                            ui.add_space(6.0);

                            if state.markers.is_empty() {
                                ui.label(egui::RichText::new("暂无标记。可在波形上拖拽鼠标框选声音，或按快捷键 1-6 添加标记并在时间线上拖动手柄微调。").size(11.5).color(theme::TEXT_MUTED));
                            } else {
                                let mut to_delete = None;
                                let mut to_export = None;
                                let mut to_select = None;

                                egui::ScrollArea::vertical()
                                    .id_salt("markers-interval-list")
                                    .max_height(170.0)
                                    .show(ui, |ui| {
                                        for m in &mut state.markers {
                                            let s_txt = format!("{:02.0}:{:04.2}", (m.start_sec / 60.0).floor(), m.start_sec % 60.0);
                                            let e_txt = format!("{:02.0}:{:04.2}", (m.end_sec / 60.0).floor(), m.end_sec % 60.0);
                                            let dur_ms = m.duration() * 1000.0;
                                            let col = m.event_type.color();
                                            let is_sel = state.selected_marker_id == Some(m.id);

                                            ui.horizontal(|ui| {
                                                let tag_prefix = if m.auto_detected { "[自动] " } else { "[手动] " };
                                                let num_txt = format!("{tag_prefix}{s_txt} ~ {e_txt} ({dur_ms:.0}ms)");
                                                if ui.selectable_label(is_sel, egui::RichText::new(num_txt).monospace().size(11.5)).clicked() {
                                                    to_select = Some(m.id);
                                                }
                                                ui.label(egui::RichText::new(m.event_type.display_name(state.mode)).size(12.0).color(col).strong());

                                                if ui.small_button("◀").on_hover_text("起点向前微调 20ms").clicked() {
                                                    m.start_sec = (m.start_sec - 0.02).max(0.0);
                                                }
                                                if ui.small_button("▶").on_hover_text("终点向后微调 20ms").clicked() {
                                                    m.end_sec += 0.02;
                                                }

                                                if matches!(m.event_type, EventType::Bite | EventType::Hit) {
                                                    if ui.small_button("★ 设为模板").clicked() {
                                                        to_export = Some(m.id);
                                                    }
                                                }

                                                if ui.small_button("▶ 播放").clicked() {
                                                    if let Some(rec) = &state.current_recording {
                                                        if let Ok(wav_bytes) = rec.slice_to_wav_bytes(m.start_sec, m.duration()) {
                                                            let arc_bytes = Arc::new(wav_bytes);
                                                            play_sound_raw(&arc_bytes);
                                                            state.playing_audio_bytes = Some(arc_bytes);
                                                        }
                                                    }
                                                }

                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.small_button("删除").clicked() {
                                                        to_delete = Some(m.id);
                                                    }
                                                });
                                            });
                                        }
                                    });

                                if let Some(id) = to_select {
                                    state.selected_marker_id = Some(id);
                                    if let Some(m) = state.markers.iter().find(|m| m.id == id) {
                                        state.cursor_sec = m.start_sec;
                                    }
                                }
                                if let Some(id) = to_delete {
                                    state.markers.retain(|m| m.id != id);
                                    if state.selected_marker_id == Some(id) {
                                        state.selected_marker_id = None;
                                    }
                                    state.save_current_session();
                                }
                                if let Some(id) = to_export {
                                    let _ = state.export_marker_as_template(id);
                                }
                            }

                            // 仿真报告
                            if let Some(rep) = &state.sim_report {
                                ui.add_space(8.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.label(egui::RichText::new("📊 算法跨切片离线仿真评测报告").strong().size(13.0).color(theme::MINT));
                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    ui.label(format!("真实咬钩召回率: {}/{}", rep.bite_label_recalled, rep.total_bites_labeled));
                                    ui.label(format!("杂音/他人拦截率: {}/{}", rep.noise_blocked, rep.total_noise_labeled));
                                });
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!(
                                        "推荐最优门限: 相似度≥{:.0}% · 电平≥{:.0}dB · 声像≤±{:.1}dB",
                                        rep.recommended_threshold, rep.recommended_level_min, rep.recommended_pan_max
                                    )).size(11.5).color(theme::CYAN));

                                    if ui.button(egui::RichText::new("一键应用到设置").size(11.5).color(theme::MINT)).clicked() {
                                        cfg.bite_threshold = rep.recommended_threshold;
                                        cfg.level_min_db = rep.recommended_level_min;
                                        cfg.pan_max_db = rep.recommended_pan_max;
                                        *cfg_changed = true;
                                        state.status_msg = "已成功将数据驱动算出的推荐门限应用到系统！".into();
                                    }
                                });
                            }
                        });

                    ui.add_space(8.0);

                    // ---- 10. 底部快捷键指南栏 ----
                    egui::Frame::new()
                        .fill(Color32::from_black_alpha(60))
                        .stroke(egui::Stroke::new(1.0, theme::LINE))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 7))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("快捷键:").strong().size(11.5).color(theme::TEXT));
                                ui.label(egui::RichText::new("[Space 空格] 播放/暂停 · [← / →] 微移游标 · [1-6] 快速打标 · [Delete] 删除选中").size(11.0).color(theme::TEXT_MUTED));
                            });
                            ui.add_space(2.0);
                            ui.label(egui::RichText::new(&state.status_msg).size(11.5).color(theme::TEXT_DIM));
                        });
                });
        });
}

