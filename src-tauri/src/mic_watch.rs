//! 麦克风设备热插拔监听（CoreAudio 设备列表变化 → 版本号递增）
//!
//! 设计（用户定则：事件驱动，不按秒轮询设备）：
//! CoreAudio 在系统设备增删时回调本模块，回调线程只做一件事——递增 MIC_VER；
//! 前端经已有的 poll_versions 通道读到版本号变化时，才真正重扫设备列表
//! （list_mics + get_active_mic）。平时轮询只读一个计数器，不枚举设备。

use std::sync::atomic::{AtomicU64, Ordering};

static MIC_VER: AtomicU64 = AtomicU64::new(0);

pub fn mic_version() -> u64 {
    MIC_VER.load(Ordering::Relaxed)
}

#[repr(C)]
struct AudioObjectPropertyAddress {
    m_selector: u32,
    m_scope: u32,
    m_element: u32,
}

const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = 0x6465_7623; // 'dev#'
const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = 0x676c_6f62; // 'glob'
const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

type ListenerProc =
    extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut std::ffi::c_void) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectAddPropertyListener(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_listener: ListenerProc,
        in_client_data: *mut std::ffi::c_void,
    ) -> i32;
}

extern "C" fn on_devices_changed(
    _obj: u32,
    _n: u32,
    _addr: *const AudioObjectPropertyAddress,
    _data: *mut std::ffi::c_void,
) -> i32 {
    MIC_VER.fetch_add(1, Ordering::Relaxed);
    0
}

/// 注册系统设备列表变化监听（幂等，只在 setup 调一次）
pub fn start() -> Result<(), String> {
    let addr = AudioObjectPropertyAddress {
        m_selector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
        m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    let status = unsafe {
        AudioObjectAddPropertyListener(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &addr,
            on_devices_changed,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(format!("AudioObjectAddPropertyListener 失败: {status}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mic_version_increments_monotonically() {
        let before = mic_version();
        MIC_VER.fetch_add(1, Ordering::SeqCst);
        assert!(mic_version() > before);
    }
}
