//! 基座工具：跨域共用的纯函数（批5 立）。
//!
//! 判据：一个纯函数被两个以上域需要时，住这里（基座无域归属，人人可用，
//! 不经门面闸）。只被一个域用的工具留在域内——不要往这里堆。
#![allow(dead_code)]

/// PCM16/mono/16k 的 WAV 头(44 字节)。
///
/// 消费方: pipeline/history(落盘 wav)与 model/speech/zhipu(拼上传体)——
/// 引擎输入格式与历史存储格式是同一格式，故住基座。
pub fn wav_header(data_len: usize) -> [u8; 44] {
    let mut h = vec![0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    let total = (36 + data_len) as u32;
    h[4..8].copy_from_slice(&total.to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes());
    h[22..24].copy_from_slice(&1u16.to_le_bytes());
    h[24..28].copy_from_slice(&16000u32.to_le_bytes());
    h[28..32].copy_from_slice(&32000u32.to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes());
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&(data_len as u32).to_le_bytes());
    let mut out = [0u8; 44];
    out.copy_from_slice(&h);
    out
}
