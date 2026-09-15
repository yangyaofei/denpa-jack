// 麦克风采集(Rust 移植自 Swift AudioRecorder)
// cpal tap 原生格式 → 下混 mono → 线性重采样 16k(相位推进, C10 教训: 不依赖系统重采样)
// 200ms = 6400B 分包 → std mpsc → 引擎 Feed
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

const CHUNK_BYTES: usize = 6400; // 16000 * 2B * 0.2s

struct Shared {
    carry_t: f64, // 相位(输入样本单位)
    buf: Vec<u8>, // 待分包 16k PCM
    total: usize, // 累计字节(判有效音频, C15: 不能用 buf 判)
    rate: f64,
    pcm_all: Vec<u8>, // 全程累积(停止后写 WAV)
    peak: f32,        // 本次会话采集到的峰值幅度(诊断: 区分"采到静音"与"没采到")
    chunks: usize,    // 已发出的 200ms 分块数(诊断)
}

pub struct Recording {
    pub stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    level: Arc<std::sync::atomic::AtomicU32>, // RMS 0-1000 定点(浮窗音量条)
    /// 会话取消令牌: 录音会话的闭包载体——B5 定时器等附属物持有它,
    /// 会话结束(停止/中止)置位, 附属物自知作废, 无全局状态
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Recording {
    pub fn level_handle(&self) -> Arc<std::sync::atomic::AtomicU32> {
        self.level.clone()
    }

    pub fn total(&self) -> usize {
        self.shared.lock().unwrap().total
    }
    /// 取走全程 PCM(停止后调用)
    pub fn take_pcm(&self) -> Vec<u8> {
        std::mem::take(&mut self.shared.lock().unwrap().pcm_all)
    }
    /// 有效音频判据: 累计 >= 0.3s(9600B)
    pub fn has_enough(&self) -> bool {
        self.total() >= 9600
    }
    /// 最近 RMS 0-1000
    pub fn level(&self) -> u32 {
        self.level.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// 采集诊断: (累计字节, 已发分块数, 峰值幅度)
    pub fn stats(&self) -> (usize, usize, f32) {
        let s = self.shared.lock().unwrap();
        (s.total, s.chunks, s.peak)
    }
}

unsafe impl Send for Recording {} // cpal Stream 平台限制; 由 AppState Mutex 独占持有, 创建线程 drop

/// (uid, name) 双元列表——uid 为 CoreAudio DeviceUID(稳定硬件 ID, 存储键), name 仅显示
pub fn list_mics() -> Vec<(String, String)> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut out = vec![];
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let uid = d.id().map(|i| i.to_string()).unwrap_or_default();
            if let Ok(n) = d.description().map(|x| x.name().to_string()) {
                if !uid.is_empty() && !out.iter().any(|(u, _)| *u == uid) {
                    out.push((uid, n));
                }
            }
        }
    }
    out
}

pub fn start_input(device: Option<String>, tx: Sender<Vec<u8>>) -> Result<Recording, String> {
    let host = cpal::default_host();
    // device 语义 = CoreAudio DeviceUID(稳定硬件 ID); 找不到时回落默认设备
    let dev = match &device {
        Some(uid) if !uid.is_empty() => {
            let found = host
                .input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.id().ok().map(|i| i.to_string()).as_deref() == Some(uid.as_str()));
            match found {
                Some(d) => d,
                None => host.default_input_device().ok_or("没有默认输入设备")?,
            }
        }
        _ => host.default_input_device().ok_or("没有默认输入设备")?,
    };
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let rate = cfg.sample_rate() as f64;
    let channels = cfg.channels() as usize;
    let fmt = cfg.sample_format();
    let dev_name = dev
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "?".into());
    crate::log::elog(&format!(
        "[audio] 打开设备 name={dev_name} requested_uid={:?} rate={rate} ch={channels} fmt={fmt:?}",
        device
    ));

    let shared = Arc::new(Mutex::new(Shared {
        carry_t: 0.0,
        buf: vec![],
        total: 0,
        rate,
        pcm_all: vec![],
        peak: 0.0,
        chunks: 0,
    }));
    let level = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let lv2 = level.clone();
    let sh2 = shared.clone();
    let tx2 = tx.clone();
    // 诊断: 流错误必须进 app.log(此前只有 eprintln, 打包版不可见)
    let err_fn = |e| {
        crate::log::elog(&format!("[audio] stream error: {e}"));
    };

    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_input_stream(
            cfg.clone().into(),
            move |data: &[f32], _| {
                update_level(&lv2, data);
                ingest(data, channels, &sh2, &tx2)
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => {
            let sh3 = shared.clone();
            let tx3 = tx;
            let lv3 = level.clone();
            dev.build_input_stream(
                cfg.clone().into(),
                move |data: &[i16], _| {
                    let f: Vec<f32> = data.iter().map(|x| *x as f32 / 32768.0).collect();
                    update_level(&lv3, &f);
                    ingest(&f, channels, &sh3, &tx3)
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("不支持的采样格式: {other:?}")),
    }
    .map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    Ok(Recording { stream, shared, level,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
}

/// 下混任意声道到 mono
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 线性重采样 mono→16k i16le(照 Swift: 相位推进, 边界不跨缓冲插值)
/// 返回 (pcm字节, 新相位)——纯函数可测
fn resample_to_16k(mono: &[f32], src_rate: f64, carry_t: f64) -> (Vec<u8>, f64) {
    let n = mono.len();
    let ratio = src_rate / 16000.0;
    let mut t = carry_t;
    let mut out = Vec::new();
    while t + 1.0 < n as f64 {
        let i0 = t as usize;
        let f = (t - i0 as f64) as f32;
        let v = mono[i0] * (1.0 - f) + mono[i0 + 1] * f;
        let v = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
        t += ratio;
    }
    (out, (t - n as f64).max(0.0))
}

fn ingest(data: &[f32], channels: usize, s: &Arc<Mutex<Shared>>, tx: &Sender<Vec<u8>>) {
    let mut st = s.lock().unwrap();
    let mono = downmix(data, channels);
    if mono.is_empty() {
        return;
    }
    // 诊断: 峰值(判断是否真采到声音)
    let p = mono.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    if p > st.peak {
        st.peak = p;
    }
    let (pcm, carry) = resample_to_16k(&mono, st.rate, st.carry_t);
    st.carry_t = carry;
    let produced = pcm.len();
    // C15: total = 累计产出字节(含已发分块), 不能用 buf.len() 判
    st.total += produced;
    st.buf.extend_from_slice(&pcm);
    st.pcm_all.extend_from_slice(&pcm);
    // 200ms 分包
    while st.buf.len() >= CHUNK_BYTES {
        let piece: Vec<u8> = st.buf.drain(..CHUNK_BYTES).collect();
        st.chunks += 1;
        let _ = tx.send(piece);
    }
}

fn update_level(lv: &std::sync::atomic::AtomicU32, data: &[f32]) {
    if data.is_empty() {
        return;
    }
    let sum: f32 = data.iter().map(|x| x * x).sum();
    let rms = (sum / data.len() as f32).sqrt();
    let v = (rms * 1000.0).clamp(0.0, 1000.0) as u32;
    lv.store(v, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod gap_tests {
    use super::*;

    #[test]
    fn downmix_stereo_averages() {
        let out = downmix(&[1.0, 0.0, 0.5, 0.5], 2);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_passthrough() {
        assert_eq!(downmix(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resample_48k_ratio_output_count() {
        // 480 个 48k 样本 = 10ms → 应产出 ~160 个 16k 样本 = ~320 字节
        let mono: Vec<f32> = (0..480).map(|i| ((i as f32) * 0.01).sin()).collect();
        let (pcm, carry) = resample_to_16k(&mono, 48000.0, 0.0);
        assert_eq!(pcm.len() / 2, 160, "48k→16k 1/3 采样数");
        assert!(carry < 3.0, "相位残留小");
    }

    #[test]
    fn resample_carry_continuity_no_double_count() {
        // 两段连续喂入: 总产出 = 单段长输入产出(±1 样本), 边界不重复不丢失
        let mono: Vec<f32> = (0..960).map(|i| ((i as f32) * 0.02).sin()).collect();
        let (a, carry) = resample_to_16k(&mono[..480], 48000.0, 0.0);
        let (b, _) = resample_to_16k(&mono[480..], 48000.0, carry);
        let (whole, _) = resample_to_16k(&mono, 48000.0, 0.0);
        assert!(((a.len() + b.len()) as i64 - whole.len() as i64).abs() <= 2, "分块与整段产出一致");
    }

    #[test]
    fn resample_amplitude_clamped() {
        let mono = vec![10.0f32, -10.0, 10.0, -10.0];
        let (pcm, _) = resample_to_16k(&mono, 16000.0, 0.0);
        for i in 0..pcm.len() / 2 {
            let v = i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]);
            assert!(v <= 32767);
        }
    }

    #[test]
    fn resample_short_input_no_panic() {
        let (pcm, _) = resample_to_16k(&[0.5], 48000.0, 0.0);
        assert!(pcm.is_empty(), "单样本不足以插值");
    }
}
