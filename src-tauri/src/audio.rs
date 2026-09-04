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

    let shared = Arc::new(Mutex::new(Shared {
        carry_t: 0.0,
        buf: vec![],
        total: 0,
        rate,
        pcm_all: vec![],
    }));
    let level = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let lv2 = level.clone();
    let sh2 = shared.clone();
    let tx2 = tx.clone();
    let err_fn = |e| eprintln!("audio stream error: {e}");

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

fn ingest(data: &[f32], channels: usize, s: &Arc<Mutex<Shared>>, tx: &Sender<Vec<u8>>) {
    let mut st = s.lock().unwrap();
    // 下混 mono
    let mono: Vec<f32> = if channels <= 1 {
        data.to_vec()
    } else {
        data.chunks(channels)
            .map(|c| c.iter().sum::<f32>() / channels as f32)
            .collect()
    };
    let n = mono.len();
    if n == 0 {
        return;
    }
    let ratio = st.rate / 16000.0;
    // 线性重采样(照 Swift: 相位推进, 边界不跨缓冲插值)
    let mut t = st.carry_t;
    let mut produced: usize = 0;
    while t + 1.0 < n as f64 {
        let i0 = t as usize;
        let f = (t - i0 as f64) as f32;
        let v = mono[i0] * (1.0 - f) + mono[i0 + 1] * f;
        let v = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        st.buf.extend_from_slice(&v.to_le_bytes());
        produced += 2;
        t += ratio;
    }
    st.carry_t = (t - n as f64).max(0.0);
    // C15: total = 累计产出字节(含已发分块), 不能用 buf.len() 判
    st.total += produced;
    let produced_slice = st.buf[st.buf.len() - produced..].to_vec();
    st.pcm_all.extend_from_slice(&produced_slice);
    // 200ms 分包
    while st.buf.len() >= CHUNK_BYTES {
        let piece: Vec<u8> = st.buf.drain(..CHUNK_BYTES).collect();
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
