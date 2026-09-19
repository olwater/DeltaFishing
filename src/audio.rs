//! 音频回环采集 + 咬钩音模板匹配。
//!
//! 原理：WASAPI 回环（loopback）捕获系统当前输出（游戏的声音），下采样到 8kHz
//! 单声道后，与预先录制的咬钩音模板做归一化互相关（即 Pearson 相关系数）。
//! 相似度超过阈值即判定咬钩。

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 内部固定处理采样率。
pub const INTERNAL_SR: u32 = 8000;
/// 每个音频块时长（秒）。
pub const BLOCK_SECS: f32 = 0.05;
/// 内置咬钩模板的有效截取长度（秒）——取「叮」声最响的核心段。
/// 模板过长会稀释相关性（环境音占满窗口时分数被拉低），过短则特征不足。
pub const TEMPLATE_LEN: f32 = 0.6;

/// 列出可做回环采集的系统输出设备名。
pub fn list_loopback_devices() -> Vec<String> {
    let mut names = Vec::new();
    let host = cpal::default_host();
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            names.push(d.to_string());
        }
    }
    names
}

/// 按名称选择输出设备（回环采集的目标）；找不到则用默认输出设备。
fn select_device(host: &cpal::Host, name: &str) -> Result<cpal::Device, String> {
    if !name.is_empty() {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(d) = devices.find(|d| d.to_string() == name) {
                return Ok(d);
            }
        }
    }
    host.default_output_device()
        .ok_or_else(|| "未找到默认音频输出设备".to_string())
}

/// 持有活跃的回环采集流；drop 即停止。
pub struct LoopbackCapture {
    _stream: cpal::Stream,
}

impl LoopbackCapture {
    /// 启动回环采集，把 8kHz 单声道样本持续写入 `buffer`。
    pub fn start(device_name: &str, buffer: Arc<Mutex<VecDeque<f32>>>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = select_device(&host, device_name)?;

        let supported = device
            .default_output_config()
            .map_err(|e| format!("获取设备格式失败: {e}"))?;
        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.config();

        let mut resampler = Resampler::new(sample_rate, INTERNAL_SR);
        let mut mono_buf: Vec<f32> = Vec::new();

        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    // 降混为单声道
                    mono_buf.clear();
                    if channels == 1 {
                        mono_buf.extend_from_slice(data);
                    } else {
                        for frame in data.chunks(channels) {
                            mono_buf.push(frame.iter().sum::<f32>() / channels as f32);
                        }
                    }
                    // 重采样到内部固定采样率
                    let mut out: Vec<f32> =
                        Vec::with_capacity(mono_buf.len() * INTERNAL_SR as usize / sample_rate as usize + 16);
                    resampler.process(&mono_buf, &mut out);
                    if !out.is_empty() {
                        if let Ok(mut q) = buffer.lock() {
                            for s in out {
                                q.push_back(s);
                            }
                            // 队列上限 3 秒，防止处理不过来时延迟无限堆积。
                            let cap = (INTERNAL_SR * 3) as usize;
                            while q.len() > cap {
                                q.pop_front();
                            }
                        }
                    }
                },
                move |err: cpal::Error| {
                    eprintln!("音频流错误: {err}");
                },
                None,
            )
            .map_err(|e| format!("创建回环采集流失败: {e}"))?;

        stream.play().map_err(|e| format!("启动音频流失败: {e}"))?;
        Ok(Self { _stream: stream })
    }
}

/// 线性插值重采样器（输入任意采样率 -> 固定输出采样率）。
struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            step: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            last: 0.0,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        while self.pos < input.len() as f64 {
            let idx = self.pos.floor() as usize;
            let frac = (self.pos - idx as f64) as f32;
            let a = if idx == 0 { self.last } else { input[idx - 1] };
            let b = input[idx.min(input.len() - 1)];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        if !input.is_empty() {
            self.last = input[input.len() - 1];
        }
        self.pos -= input.len() as f64;
    }
}

/// 归一化后的音频模板：居中 + L2 单位范数（Pearson 相关系数）。
///
/// 同时保存两路特征：
/// - `tpl`：原始波形归一化，对与模板高度一致的声音给出高分；
/// - `env`：高通后的能量包络归一化，对相位/音调变化不敏感，
///   游戏里每次咬钩音略有差异时仍能匹配上。
#[derive(Clone)]
pub struct Template {
    tpl: Vec<f32>,
    env: Vec<f32>,
}

impl Template {
    /// 从内存中的 WAV 字节加载模板（内嵌的默认咬钩音）。
    pub fn from_bytes(bytes: &[u8], max_len_secs: f32) -> Result<Self, String> {
        let (samples, in_rate) = decode_wav(bytes)?;
        let samples = trim_silence(&samples);
        let samples = resample_once(&samples, in_rate, INTERNAL_SR);
        let max_len = (max_len_secs * INTERNAL_SR as f32) as usize;
        let samples = &samples[..samples.len().min(max_len)];
        if samples.len() < 32 {
            return Err("模板有效长度过短".to_string());
        }
        Ok(Self::from_samples(samples))
    }

    /// 居中并做 L2 归一化，使匹配结果落到 [-1, 1]（Pearson 相关系数）。
    fn from_samples(samples: &[f32]) -> Self {
        // 波形通道在「高通域」归一化：滤掉低频环境声，保留咬钩「叮」的高频特征。
        let tpl = l2_normalize(&center(&highpass(samples)));
        let env = l2_normalize(&center(&highpass_envelope(samples)));
        Self { tpl, env }
    }

    fn len(&self) -> usize {
        self.tpl.len()
    }
}

fn center(samples: &[f32]) -> Vec<f32> {
    let mean = samples.iter().sum::<f32>() / samples.len() as f32;
    samples.iter().map(|s| s - mean).collect()
}

fn l2_normalize(samples: &[f32]) -> Vec<f32> {
    let norm = samples.iter().map(|s| s * s).sum::<f32>().sqrt();
    if norm < 1e-9 {
        return samples.to_vec();
    }
    samples.iter().map(|s| s / norm).collect()
}

/// 一阶高通（8kHz 下截止约 130Hz）：去掉低频轰鸣，保留咬钩「叮」的高频成分。
fn highpass(samples: &[f32]) -> Vec<f32> {
    let mut prev = 0.0f32;
    samples
        .iter()
        .map(|&x| {
            let hp = x - 0.9f32 * prev;
            prev = x;
            hp
        })
        .collect()
}

/// 提取「高通 → 整流 → 低通平滑」能量包络。
///
/// 包络描述声音响度随时间的起伏形状——咬钩音即使音调/相位略有差异，
/// 包络形状仍高度相似。
fn highpass_envelope(samples: &[f32]) -> Vec<f32> {
    let alpha = 0.18f32; // 8kHz 下约 5ms 平滑，跟随短促咬钩音
    let mut env = 0.0f32;
    highpass(samples)
        .iter()
        .map(|&hp| {
            env += alpha * (hp.abs() - env);
            env
        })
        .collect()
}

/// 解码 WAV 字节为单声道 f32 样本与其原始采样率。
fn decode_wav(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(|e| format!("解析 WAV 失败: {e}"))?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;

    let mono: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => {
            let v: Vec<f32> = reader
                .samples::<f32>()
                .collect::<Result<_, _>>()
                .map_err(|e| format!("读取 WAV 失败: {e}"))?;
            downmix(&v, channels)
        }
        hound::SampleFormat::Int => match spec.bits_per_sample {
            16 => {
                let v: Vec<i16> = reader
                    .samples::<i16>()
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("读取 WAV 失败: {e}"))?;
                downmix(&v.iter().map(|&s| s as f32 / 32768.0).collect::<Vec<_>>(), channels)
            }
            24 => {
                let v: Vec<i32> = reader
                    .samples::<i32>()
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("读取 WAV 失败: {e}"))?;
                downmix(
                    &v.iter().map(|&s| s as f32 / 8_388_608.0).collect::<Vec<_>>(),
                    channels,
                )
            }
            8 => {
                let v: Vec<i8> = reader
                    .samples::<i8>()
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("读取 WAV 失败: {e}"))?;
                downmix(
                    &v.iter().map(|&s| s as f32 / 128.0).collect::<Vec<_>>(),
                    channels,
                )
            }
            _ => {
                let v: Vec<i32> = reader
                    .samples::<i32>()
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("读取 WAV 失败: {e}"))?;
                downmix(
                    &v.iter().map(|&s| s as f32 / 2_147_483_648.0).collect::<Vec<_>>(),
                    channels,
                )
            }
        },
    };
    Ok((mono, sample_rate))
}

fn downmix(v: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return v.to_vec();
    }
    v.chunks(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 去除首尾静音（以峰值 15% 为界，只保留咬钩音最响的核心段落），
/// 避免低能量的尾巴稀释相关性、模板长短随录音漂移。
fn trim_silence(samples: &[f32]) -> &[f32] {
    let peak = samples.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
    if peak < 1e-6 {
        return samples;
    }
    let thr = peak * 0.15;
    let start = samples.iter().position(|&s| s.abs() >= thr).unwrap_or(0);
    let end = samples
        .iter()
        .rposition(|&s| s.abs() >= thr)
        .map(|i| i + 1)
        .unwrap_or(samples.len());
    &samples[start.max(0)..end.min(samples.len())]
}

/// 一次性线性重采样（用于模板加载阶段）。
fn resample_once(samples: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate {
        return samples.to_vec();
    }
    let mut r = Resampler::new(in_rate, out_rate);
    let mut out = Vec::with_capacity(samples.len() * out_rate as usize / in_rate as usize + 16);
    r.process(samples, &mut out);
    out
}

/// 滚动缓冲上的实时咬钩检测器。
pub struct Detector {
    template: Template,
    buf: VecDeque<f32>,
    window_len: usize,
    last_sim: f32,
}

impl Detector {
    pub fn new(template: Template) -> Self {
        let n = template.len();
        let block = (BLOCK_SECS * INTERNAL_SR as f32) as usize;
        Self {
            template,
            buf: VecDeque::with_capacity(n + block),
            window_len: n + block,
            last_sim: 0.0,
        }
    }

    pub fn last_sim(&self) -> f32 {
        self.last_sim
    }

    /// 喂入新样本，返回当前最大的归一化相似度（-1..=1）。
    ///
    /// 取「高通波形相关」与「能量包络相关」两路的最高分：
    /// 前者精确、后者对游戏内每次音效的细微差异鲁棒。
    pub fn push(&mut self, samples: &[f32]) -> f32 {
        self.buf.extend(samples.iter().copied());
        while self.buf.len() > self.window_len {
            self.buf.pop_front();
        }
        let n = self.template.len();
        if self.buf.len() < n + 1 {
            return 0.0;
        }
        let x = self.buf.make_contiguous();
        let m = x.len();

        // 一次性对整个滚动缓冲做高通，两路匹配共用。
        let hp = highpass(x);
        let mut env = 0.0f32;
        let env: Vec<f32> = hp
            .iter()
            .map(|&h| {
                env += 0.18 * (h.abs() - env);
                env
            })
            .collect();

        // 通道 1：高通波形 ZNCC，对齐步长 4 个样本（0.5ms）。
        let mut best = 0.0f32;
        for k in (0..=(m - n)).step_by(4) {
            best = best.max(zncc(&hp[k..k + n], &self.template.tpl));
        }

        // 通道 2：能量包络 ZNCC，包络变化慢，步长 16（2ms）即可。
        // 跳过开头避开滤波器起振瞬态。
        for k in (40..=(m - n)).step_by(16) {
            best = best.max(zncc(&env[k..k + n], &self.template.env));
        }

        // 检测值必须反映当前音频；显示峰值只在 UI 中保持。
        self.last_sim = best;
        best
    }
}

/// 对一段窗口与已 L2 归一化的模板计算 Pearson 相关系数（窗口现场去均值/归一化）。
fn zncc(window: &[f32], normalized_template: &[f32]) -> f32 {
    let n = window.len();
    let mean = window.iter().sum::<f32>() / n as f32;
    let mut num = 0.0f32;
    let mut den = 0.0f32;
    for i in 0..n {
        let d = window[i] - mean;
        num += d * normalized_template[i];
        den += d * d;
    }
    let ws = den.sqrt();
    if ws < 1e-6 {
        0.0
    } else {
        num / ws
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn silence_clears_previous_match() {
        let template = Template::from_bytes(include_bytes!("../assets/bite.wav"), TEMPLATE_LEN).unwrap();
        let mut detector = Detector::new(template);
        detector.last_sim = 0.9;
        let silence = vec![0.0; detector.window_len];
        assert_eq!(detector.push(&silence), 0.0);
        assert_eq!(detector.last_sim(), 0.0);
    }
}
