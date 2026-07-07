//! 系统音频采集：spawn Swift 助手（ScreenCaptureKit）读 PCM，
//! rustfft 算出与 Web Audio AnalyserNode 对齐的 512 bin / 0-255 频谱，
//! base64 后以 `system-audio-spectrum` 事件推给前端。
//!
//! 会话用代际号（generation）标识：reader 线程退出时只清理"自己这一代"，
//! 避免旧线程误杀快速重开后的新会话；意外死亡（睡眠唤醒、显示器变化）
//! 在运行超过 30s 后自动重启一次。

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rustfft::{num_complex::Complex, FftPlanner};
use tauri::{Emitter, Manager};

struct Session {
    child: Child,
    gen: u64,
}

pub struct SysAudioState {
    session: Mutex<Option<Session>>,
    /// 用户意图：菜单开=true / 关=false。自动重启只在仍为 true 时进行。
    desired: AtomicBool,
}

impl SysAudioState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            desired: AtomicBool::new(false),
        }
    }
}

static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

const FFT_SIZE: usize = 1024;
const BINS: usize = FFT_SIZE / 2;
// 与前端 analyser.minDecibels = -75 / 默认 maxDecibels = -30 对齐
const MIN_DB: f32 = -75.0;
const MAX_DB: f32 = -30.0;

/// 频谱分析器：Blackman 窗 + |X[k]|/N + 线性域 0.8 平滑 + dB→byte 映射，
/// 与 Web Audio 规范的 AnalyserNode 完全同构，保证系统音频模式与麦克风/文件模式亮度一致。
struct SpectrumAnalyzer {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    smoothed: Vec<f32>,
    bytes: Vec<u8>,
}

impl SpectrumAnalyzer {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        Self {
            fft: planner.plan_fft_forward(FFT_SIZE),
            window: (0..FFT_SIZE)
                .map(|i| {
                    let x = std::f32::consts::TAU * i as f32 / FFT_SIZE as f32;
                    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
                })
                .collect(),
            buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            smoothed: vec![0.0; BINS],
            bytes: vec![0u8; BINS],
        }
    }

    fn process(&mut self, samples: &[f32]) -> &[u8] {
        for i in 0..FFT_SIZE {
            self.buf[i] = Complex::new(samples[i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.buf);
        for k in 0..BINS {
            let mag = self.buf[k].norm() / FFT_SIZE as f32;
            // 与 AnalyserNode smoothingTimeConstant=0.8 同语义（线性域平滑再取 dB）
            self.smoothed[k] = 0.8 * self.smoothed[k] + 0.2 * mag;
            let db = 20.0 * (self.smoothed[k] + 1e-10).log10();
            self.bytes[k] = ((db - MIN_DB) / (MAX_DB - MIN_DB) * 255.0).clamp(0.0, 255.0) as u8;
        }
        &self.bytes
    }
}

fn helper_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    if let Ok(dir) = app.path().resource_dir() {
        let p = dir.join("helper").join("audio-tap");
        if p.exists() {
            return Some(p);
        }
    }
    // dev 模式：直接用源码树里编译出的助手
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("helper")
        .join("audio-tap");
    if dev.exists() {
        Some(dev)
    } else {
        None
    }
}

pub fn start(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<SysAudioState>();
    state.desired.store(true, Ordering::SeqCst);
    let mut guard = state.session.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }
    let path = helper_path(app).ok_or("找不到 audio-tap 助手程序")?;
    let mut child = Command::new(&path)
        .stdout(Stdio::piped())
        .stdin(Stdio::piped()) // 保持管道：主进程退出时助手随之退出
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("启动系统音频助手失败: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("无法读取助手输出")?;
    let my_gen = NEXT_GEN.fetch_add(1, Ordering::SeqCst);
    let started = std::time::Instant::now();
    *guard = Some(Session { child, gen: my_gen });
    drop(guard);

    let app = app.clone();
    std::thread::spawn(move || {
        let debug = std::env::var("YINLANG_SYS_AUDIO").as_deref() == Ok("1");
        let mut analyzer = SpectrumAnalyzer::new();
        let mut raw = vec![0u8; FFT_SIZE * 4];
        let mut samples = vec![0.0f32; FFT_SIZE];
        let mut n = 0usize;

        loop {
            if stdout.read_exact(&mut raw).is_err() {
                break;
            }
            for i in 0..FFT_SIZE {
                samples[i] =
                    f32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]);
            }
            let bytes = analyzer.process(&samples);
            let _ = app.emit("system-audio-spectrum", B64.encode(bytes));

            if debug {
                n += 1;
                if n % 100 == 0 {
                    eprintln!("[sys-audio] chunk {n}, max bin: {}", bytes.iter().max().unwrap());
                }
            }
        }

        // ── 退出路径 ──
        let state = app.state::<SysAudioState>();
        let mut guard = state.session.lock().unwrap();
        let is_current = guard.as_ref().map_or(false, |s| s.gen == my_gen);
        if !is_current {
            // stop() 或新会话已接管，静默退出（不误报、不误杀）
            return;
        }
        if let Some(mut s) = guard.take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
        drop(guard);

        // 运行较久后意外死亡（睡眠唤醒、锁屏、显示器重配）→ 自动重启一次
        if started.elapsed() > std::time::Duration::from_secs(30)
            && state.desired.load(Ordering::SeqCst)
        {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if state.desired.load(Ordering::SeqCst) && start(&app).is_ok() {
                if debug {
                    eprintln!("[sys-audio] helper died after long run, restarted");
                }
                return;
            }
        }

        let _ = app.emit("system-audio", false);
        let _ = app.emit(
            "system-audio-error",
            "系统音频已停止（若是首次使用，请在 系统设置→隐私与安全性→屏幕录制 中授权后重启应用）",
        );
    });
    Ok(())
}

/// 停止采集；返回 true 表示确实有在跑的会话被停掉。
pub fn stop(app: &tauri::AppHandle) -> bool {
    let state = app.state::<SysAudioState>();
    state.desired.store(false, Ordering::SeqCst);
    let taken = state.session.lock().unwrap().take();
    if let Some(mut s) = taken {
        let _ = s.child.kill();
        let _ = s.child.wait();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amp: f32, bin: f32) -> Vec<f32> {
        (0..FFT_SIZE)
            .map(|i| amp * (std::f32::consts::TAU * bin * i as f32 / FFT_SIZE as f32).cos())
            .collect()
    }

    /// -40dBFS 正弦落在 bin 中心：AnalyserNode 语义下 |X| = 0.42·a/2
    /// → 20·log10(0.21·0.01) ≈ -53.6dB → byte ≈ (-53.6+75)/45·255 ≈ 121
    #[test]
    fn sine_calibration_matches_analyser_node() {
        let mut an = SpectrumAnalyzer::new();
        let samples = sine(0.01, 100.0);
        let mut last = 0u8;
        for _ in 0..60 {
            last = an.process(&samples)[100];
        }
        assert!(
            (115..=127).contains(&(last as i32)),
            "期望 ~121（与 Web Audio AnalyserNode 对齐），实际 {last}"
        );
    }

    #[test]
    fn silence_maps_to_zero() {
        let mut an = SpectrumAnalyzer::new();
        let bytes = an.process(&vec![0.0f32; FFT_SIZE]);
        assert!(bytes.iter().all(|&b| b == 0));
    }

    /// 满幅正弦 -13.6dB 高于 maxDecibels(-30) → 打顶 255（与 AnalyserNode 一致）
    #[test]
    fn full_scale_saturates_like_analyser_node() {
        let mut an = SpectrumAnalyzer::new();
        let samples = sine(1.0, 50.0);
        let mut last = 0u8;
        for _ in 0..60 {
            last = an.process(&samples)[50];
        }
        assert_eq!(last, 255);
    }
}
