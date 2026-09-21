//! 音频回环采集 + 真实切水咬钩声特征匹配与声学判定。
//!
//! 原理：WASAPI 回环捕获游戏音频输出，下采样到 8kHz 立体声。
//! 针对《三角洲行动》咬钩“鱼漂顿挫切水声”（核心共鸣能量在 200 Hz ~ 1200 Hz）的物理特性，
//! 采用 4 阶 Butterworth 数字带通滤波切除无关低频嗡鸣与高频流水/枪炮白噪，
//! 并结合“带通波形 + 水涌瞬态包络”双通道 ZNCC 匹配与空间声像差判定，
//! 实现高置信度、抗环境干扰的咬钩识别。

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// 内部固定处理采样率（涵盖 0 ~ 4000 Hz，完美覆盖咬钩核心声学特征）。
pub const INTERNAL_SR: u32 = 8000;
/// 每个音频块时长（秒）。
pub const BLOCK_SECS: f32 = 0.05;
/// 真实咬钩切水瞬态的有效截取长度（秒）——取 300ms 水涌爆发核心段。
pub const TEMPLATE_LEN: f32 = 0.30;

/// 双声道立体声采样帧。
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct StereoFrame {
    pub l: f32,
    pub r: f32,
}

impl StereoFrame {
    #[inline]
    pub fn new(l: f32, r: f32) -> Self {
        Self { l, r }
    }

    #[inline]
    pub fn mono(&self) -> f32 {
        (self.l + self.r) * 0.5
    }
}

/// 咬钩音检测与声学特征判定结果。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DetectionResult {
    /// 归一化互相关最高分（0.0..=1.0）
    pub sim: f32,
    /// 命中段综合电平（dBFS，负数，如 -21.0 dB）
    pub level_db: f32,
    /// 命中段声像差（dB，左声道分贝减右声道分贝，0 表示正中）
    pub pan_db: f32,
}

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
    /// 启动回环采集，把 8kHz 立体声样本持续写入 buffer。
    pub fn start(device_name: &str, buffer: Arc<Mutex<VecDeque<StereoFrame>>>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = select_device(&host, device_name)?;

        let supported = device
            .default_output_config()
            .map_err(|e| format!("获取设备格式失败: {e}"))?;
        let channels = supported.channels() as usize;
        let sample_rate = supported.sample_rate();
        let config: cpal::StreamConfig = supported.config();

        let mut resampler = StereoResampler::new(sample_rate, INTERNAL_SR);
        let mut raw_frames: Vec<StereoFrame> = Vec::new();

        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                    raw_frames.clear();
                    if channels == 1 {
                        for &s in data {
                            raw_frames.push(StereoFrame::new(s, s));
                        }
                    } else {
                        for frame in data.chunks(channels) {
                            let l = frame[0];
                            let r = if frame.len() > 1 { frame[1] } else { l };
                            raw_frames.push(StereoFrame::new(l, r));
                        }
                    }
                    let mut out: Vec<StereoFrame> =
                        Vec::with_capacity(raw_frames.len() * INTERNAL_SR as usize / sample_rate as usize + 16);
                    resampler.process(&raw_frames, &mut out);
                    if !out.is_empty() {
                        if let Ok(mut q) = buffer.lock() {
                            for s in out {
                                q.push_back(s);
                            }
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

/// 双声道线性插值重采样器（输入任意采样率 -> 固定 8kHz 输出）。
struct StereoResampler {
    step: f64,
    pos: f64,
    last_l: f32,
    last_r: f32,
}

impl StereoResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            step: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            last_l: 0.0,
            last_r: 0.0,
        }
    }

    fn process(&mut self, input: &[StereoFrame], out: &mut Vec<StereoFrame>) {
        while self.pos < input.len() as f64 {
            let idx = self.pos.floor() as usize;
            let frac = (self.pos - idx as f64) as f32;
            let (a_l, a_r) = if idx == 0 {
                (self.last_l, self.last_r)
            } else {
                (input[idx - 1].l, input[idx - 1].r)
            };
            let b = input[idx.min(input.len() - 1)];
            let l = a_l + (b.l - a_l) * frac;
            let r = a_r + (b.r - a_r) * frac;
            out.push(StereoFrame::new(l, r));
            self.pos += self.step;
        }
        if let Some(last) = input.last() {
            self.last_l = last.l;
            self.last_r = last.r;
        }
        self.pos -= input.len() as f64;
    }
}

/// 4 阶数字 Butterworth 带通滤波器（通带 200 Hz ~ 1200 Hz @ 8000 Hz 采样率）。
/// 精准保留鱼漂顿水与鱼线切水的核心共鸣峰，彻底切除无关低频交流声与高频流水/枪炮白噪。
#[derive(Clone, Default)]
pub struct BandpassFilter {
    x: [f32; 4],
    y: [f32; 4],
}

impl BandpassFilter {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn process_sample(&mut self, input: f32) -> f32 {
        const B: [f32; 5] = [
            0.09763107,
            0.0,
            -0.19526215,
            0.0,
            0.09763107,
        ];
        const A: [f32; 4] = [
            -2.71589217,
            2.88146304,
            -1.48537075,
            0.33333333,
        ];

        let output = B[0] * input
            + B[1] * self.x[0]
            + B[2] * self.x[1]
            + B[3] * self.x[2]
            + B[4] * self.x[3]
            - (A[0] * self.y[0] + A[1] * self.y[1] + A[2] * self.y[2] + A[3] * self.y[3]);

        self.x[3] = self.x[2];
        self.x[2] = self.x[1];
        self.x[1] = self.x[0];
        self.x[0] = input;

        self.y[3] = self.y[2];
        self.y[2] = self.y[1];
        self.y[1] = self.y[0];
        self.y[0] = output;

        output
    }

    pub fn filter_slice(&mut self, samples: &[f32]) -> Vec<f32> {
        samples.iter().map(|&s| self.process_sample(s)).collect()
    }
}

/// 提取水涌切水瞬态包络（整流 + 15ms 平滑滤波）。
fn extract_envelope(bandpassed: &[f32]) -> Vec<f32> {
    let alpha = 0.08f32; // 8kHz 下约 15ms 平滑时间常数
    let mut env = 0.0f32;
    bandpassed
        .iter()
        .map(|&bp| {
            env += alpha * (bp.abs() - env);
            env
        })
        .collect()
}

/// 归一化后的音频模板：居中 + L2 单位范数（Pearson 相关系数）。
#[derive(Clone)]
pub struct Template {
    tpl: Vec<f32>,
    env: Vec<f32>,
}

impl Template {
    /// 从内存中的 WAV 字节加载模板，提取真正的咬钩切水水涌爆发段。
    pub fn from_bytes(bytes: &[u8], max_len_secs: f32) -> Result<Self, String> {
        let (samples, in_rate) = decode_wav(bytes)?;
        let samples = resample_once(&samples, in_rate, INTERNAL_SR);

        let samples = trim_burst_segment(&samples, max_len_secs);
        if samples.len() < 32 {
            return Err("模板有效长度过短".to_string());
        }
        Ok(Self::from_samples(&samples))
    }

    /// 经过 200~1200Hz 带通滤波后居中并做 L2 归一化。
    fn from_samples(samples: &[f32]) -> Self {
        let mut filter = BandpassFilter::new();
        let bp = filter.filter_slice(samples);
        let tpl = l2_normalize(&center(&bp));
        let env = l2_normalize(&center(&extract_envelope(&bp)));
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

/// 从整段录音中精准截取咬钩切水瞬态爆发段（剔除前置静音与长尾水声）。
fn trim_burst_segment(samples: &[f32], max_len_secs: f32) -> Vec<f32> {
    let peak = samples.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
    if peak < 1e-6 {
        return samples.to_vec();
    }
    let thr = peak * 0.15;
    let start = samples.iter().position(|&s| s.abs() >= thr).unwrap_or(0);
    let max_len = (max_len_secs * INTERNAL_SR as f32) as usize;
    let end = (start + max_len).min(samples.len());
    samples[start..end].to_vec()
}

/// 一次性线性重采样（用于模板加载阶段）。
fn resample_once(samples: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate {
        return samples.to_vec();
    }
    let mut r = StereoResampler::new(in_rate, out_rate);
    let frames: Vec<StereoFrame> = samples.iter().map(|&s| StereoFrame::new(s, s)).collect();
    let mut out = Vec::with_capacity(samples.len() * out_rate as usize / in_rate as usize + 16);
    r.process(&frames, &mut out);
    out.iter().map(|f| f.l).collect()
}

/// 滚动缓冲上的实时咬钩检测与声学特征分析器。
pub struct Detector {
    template: Template,
    buf: VecDeque<StereoFrame>,
    window_len: usize,
    last_result: DetectionResult,
}

impl Detector {
    pub fn new(template: Template) -> Self {
        let n = template.len();
        let block = (BLOCK_SECS * INTERNAL_SR as f32) as usize;
        Self {
            template,
            buf: VecDeque::with_capacity(n + block),
            window_len: n + block,
            last_result: DetectionResult::default(),
        }
    }

    pub fn last_result(&self) -> DetectionResult {
        self.last_result
    }

    #[inline]
    #[allow(dead_code)]
    pub fn last_sim(&self) -> f32 {
        self.last_result.sim
    }

    /// 喂入新立体声样本，返回当前最佳检测结果（相似度、综合电平 dBFS、声像差 Pan dB）。
    pub fn push(&mut self, samples: &[StereoFrame]) -> DetectionResult {
        self.buf.extend(samples.iter().copied());
        while self.buf.len() > self.window_len {
            self.buf.pop_front();
        }
        let n = self.template.len();
        if self.buf.len() < n + 1 {
            self.last_result = DetectionResult::default();
            return self.last_result;
        }
        let frames = self.buf.make_contiguous();
        let m = frames.len();

        // 提取单声道并应用 200~1200Hz 核心带通滤波
        let mono: Vec<f32> = frames.iter().map(|f| f.mono()).collect();
        let mut filter = BandpassFilter::new();
        let bp = filter.filter_slice(&mono);
        let env = extract_envelope(&bp);

        let mut best_sim = 0.0f32;
        let mut best_k = 0usize;

        // 通道 1：200~1200Hz 咬钩切水共鸣带通波形 ZNCC，对齐步长 4 个样本（0.5ms）。
        for k in (0..=(m - n)).step_by(4) {
            let s = zncc(&bp[k..k + n], &self.template.tpl);
            if s > best_sim {
                best_sim = s;
                best_k = k;
            }
        }

        // 通道 2：水涌冲击包络 ZNCC，步长 16（2ms），对相位扰动与微观水花完全免疫。
        for k in (20..=(m - n)).step_by(16) {
            let s = zncc(&env[k..k + n], &self.template.env);
            if s > best_sim {
                best_sim = s;
                best_k = k;
            }
        }

        // 提取出峰窗口对应的立体声能量特征（计算电平与声像差）
        let seg = &frames[best_k..best_k + n];
        let n_f = n as f32;
        let sum_sq_l: f32 = seg.iter().map(|f| f.l * f.l).sum();
        let sum_sq_r: f32 = seg.iter().map(|f| f.r * f.r).sum();
        let sum_sq_mono: f32 = seg.iter().map(|f| {
            let val = f.mono();
            val * val
        }).sum();

        let rms_l = (sum_sq_l / n_f).sqrt();
        let rms_r = (sum_sq_r / n_f).sqrt();
        let rms_mono = (sum_sq_mono / n_f).sqrt();

        let level_db = 20.0 * rms_mono.max(1e-6).log10();
        let pan_db = 20.0 * rms_l.max(1e-6).log10() - 20.0 * rms_r.max(1e-6).log10();

        let res = DetectionResult {
            sim: best_sim,
            level_db,
            pan_db,
        };
        self.last_result = res;
        res
    }
}

/// 对一段窗口与已 L2 归一化的模板计算 Pearson 相关系数。
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
        let silence = vec![StereoFrame::new(0.0, 0.0); detector.window_len];
        let res = detector.push(&silence);
        assert_eq!(res.sim, 0.0);
        assert_eq!(detector.last_sim(), 0.0);
    }

    #[test]
    fn bandpass_preserves_bite_resonance() {
        let mut filter = BandpassFilter::new();
        let t: Vec<f32> = (0..8000).map(|i| i as f32 / 8000.0).collect();
        let s400: Vec<f32> = t.iter().map(|&x| (x * 400.0 * std::f32::consts::TAU).sin()).collect();
        let out400 = filter.filter_slice(&s400);
        let rms400 = (out400[2000..].iter().map(|x| x * x).sum::<f32>() / 6000.0).sqrt();
        assert!(rms400 > 0.6, "400Hz 核心通带增益正常");

        let mut filter2 = BandpassFilter::new();
        let s3500: Vec<f32> = t.iter().map(|&x| (x * 3500.0 * std::f32::consts::TAU).sin()).collect();
        let out3500 = filter2.filter_slice(&s3500);
        let rms3500 = (out3500[2000..].iter().map(|x| x * x).sum::<f32>() / 6000.0).sqrt();
        assert!(rms3500 < 0.05, "3500Hz 高频噪声被深度压制");
    }
}
