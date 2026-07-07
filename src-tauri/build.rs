fn main() {
    #[cfg(target_os = "macos")]
    compile_audio_tap();
    tauri_build::build()
}

/// 编译系统音频助手（ScreenCaptureKit → stdout PCM）。
/// tauri.conf.json 将 helper/audio-tap 声明为打包资源，缺失会让 tauri_build 硬失败，
/// 所以编译失败且无既有二进制时直接 panic 给出可操作的错误。
#[cfg(target_os = "macos")]
fn compile_audio_tap() {
    use std::path::Path;

    println!("cargo:rerun-if-changed=helper/audio-tap.swift");

    let src = Path::new("helper/audio-tap.swift");
    let bin = Path::new("helper/audio-tap");
    let marker = Path::new("helper/.audio-tap.arch");

    // 架构跟随 cargo 编译目标（universal 构建会按目标各跑一次）
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        _ => "arm64",
    };

    // 二进制比源码新且架构一致时跳过——build.rs 因其他 watched 文件重跑时不重复编译
    let up_to_date = match (
        src.metadata().and_then(|m| m.modified()),
        bin.metadata().and_then(|m| m.modified()),
    ) {
        (Ok(sm), Ok(bm)) => bm >= sm,
        _ => false,
    };
    let arch_matches = std::fs::read_to_string(marker)
        .map(|s| s.trim() == arch)
        .unwrap_or(false);
    if up_to_date && arch_matches {
        return;
    }

    let target = format!("{arch}-apple-macos13.0");
    let status = std::process::Command::new("xcrun")
        .args(["swiftc", "-O", "-target", &target, "-o", "helper/audio-tap", "helper/audio-tap.swift"])
        .status();

    match status {
        Ok(s) if s.success() => {
            let _ = std::fs::write(marker, arch);
        }
        _ if bin.exists() => {
            println!("cargo:warning=audio-tap 重新编译失败，沿用已有二进制（可能架构/版本过旧）");
        }
        _ => {
            panic!(
                "audio-tap 助手编译失败且无既有二进制。\
                 需要 Xcode Command Line Tools（xcode-select --install）才能构建系统音频功能。"
            );
        }
    }
}
