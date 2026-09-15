// 诊断: cpal 视角的设备枚举 (all vs input-only, 带 default_input_config 结果)
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    let host = cpal::default_host();
    println!("--- host.devices() 全部 ---");
    if let Ok(devs) = host.devices() {
        for d in devs {
            let uid = d.id().map(|i| i.to_string()).unwrap_or_default();
            let name = d.description().map(|x| x.name().to_string()).unwrap_or_default();
            let cfg = d.default_input_config().map(|c| format!("{:?}", c)).unwrap_or_else(|e| format!("ERR: {e}"));
            println!("  name={name}\n    uid={uid}\n    default_input_config={cfg}");
        }
    } else {
        println!("  devices() 失败");
    }
    println!("--- host.input_devices() 仅输入 ---");
    match host.input_devices() {
        Ok(devs) => {
            let mut n = 0;
            for d in devs {
                n += 1;
                let name = d.description().map(|x| x.name().to_string()).unwrap_or_default();
                println!("  [{n}] {name}");
            }
            println!("  共 {n} 个");
        }
        Err(e) => println!("  input_devices() 失败: {e}"),
    }
}
