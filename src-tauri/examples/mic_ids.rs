// 打印当前输入设备 (uid, name) — 麦克风配置链路测试辅助
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let uid = d.id().map(|i| i.to_string()).unwrap_or_default();
            let name = d
                .description()
                .map(|x| x.name().to_string())
                .unwrap_or_default();
            println!("uid={uid} name={name}");
        }
    }
}
