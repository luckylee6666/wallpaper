//! 系统音频采集 → rustfft 算出与 Web Audio AnalyserNode 对齐的 512 bin / 0-255 频谱，
//! base64 后以 `system-audio-spectrum` 事件推给前端。
//!
//! - macOS：spawn Swift 助手（ScreenCaptureKit）读 stdout PCM。
//! - Windows：进程内 WASAPI loopback 采集默认播放设备（无需任何权限）。
//!
//! 会话用代际号（generation）标识：采集结束时只清理"自己这一代"，
//! 避免旧线程误杀快速重开后的新会话；运行超过 30s 后意外结束自动重启一次。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rustfft::{num_complex::Complex, FftPlanner};
#[cfg_attr(not(target_os = "macos"), allow(unused_imports))]
use tauri::{Emitter, Manager};

// ═══════════════════════════════════════════════════════════════
//  共享状态
// ═══════════════════════════════════════════════════════════════

struct Session {
    gen: u64,
    /// macOS：Swift 助手子进程；杀进程使 stdout 读取失败以退出 reader 线程。
    #[cfg(target_os = "macos")]
    child: std::process::Child,
    /// Windows：置 true 通知 WASAPI 采集循环退出。
    #[cfg(target_os = "windows")]
    stop: std::sync::Arc<AtomicBool>,
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

/// 累积任意长度的单声道 f32 数据成 FFT_SIZE 帧，逐帧处理并推送频谱事件。
/// macOS 每次正好喂一帧；Windows WASAPI 返回变长缓冲，靠累积器对齐。
#[allow(dead_code)]
struct SpectrumPipeline {
    analyzer: SpectrumAnalyzer,
    acc: Vec<f32>,
}

#[allow(dead_code)]
impl SpectrumPipeline {
    fn new() -> Self {
        Self {
            analyzer: SpectrumAnalyzer::new(),
            acc: Vec::with_capacity(FFT_SIZE * 2),
        }
    }

    fn feed(&mut self, chunk: &[f32], app: &tauri::AppHandle) {
        self.acc.extend_from_slice(chunk);
        while self.acc.len() >= FFT_SIZE {
            let bytes = self.analyzer.process(&self.acc[..FFT_SIZE]);
            let _ = app.emit("system-audio-spectrum", B64.encode(bytes));
            self.acc.drain(..FFT_SIZE);
        }
    }
}

// ═══════════════════════════════════════════════════════════════
//  macOS：Swift 助手（ScreenCaptureKit）
// ═══════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
pub fn start(app: &tauri::AppHandle) -> Result<(), String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

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

        finish(
            &app,
            my_gen,
            started,
            "系统音频已停止（若是首次使用，请在 系统设置→隐私与安全性→屏幕录制 中授权后重启应用）",
        );
    });
    Ok(())
}

#[cfg(target_os = "macos")]
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

// ═══════════════════════════════════════════════════════════════
//  Windows：WASAPI loopback
// ═══════════════════════════════════════════════════════════════

#[cfg(target_os = "windows")]
pub fn start(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<SysAudioState>();
    state.desired.store(true, Ordering::SeqCst);
    let mut guard = state.session.lock().unwrap();
    if guard.is_some() {
        return Ok(());
    }
    let my_gen = NEXT_GEN.fetch_add(1, Ordering::SeqCst);
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let started = std::time::Instant::now();
    *guard = Some(Session {
        gen: my_gen,
        stop: stop.clone(),
    });
    drop(guard);

    let app = app.clone();
    std::thread::spawn(move || {
        let debug = std::env::var("YINLANG_SYS_AUDIO").as_deref() == Ok("1");
        let mut pipeline = SpectrumPipeline::new();
        let result = win_loopback::run(&stop, |chunk| pipeline.feed(chunk, &app));
        if debug {
            if let Err(e) = &result {
                eprintln!("[sys-audio] loopback ended: {e}");
            }
        }
        finish(
            &app,
            my_gen,
            started,
            "系统音频已停止（未检测到可采集的播放设备，请确认有音频输出设备）",
        );
    });
    Ok(())
}

#[cfg(target_os = "windows")]
pub fn stop(app: &tauri::AppHandle) -> bool {
    let state = app.state::<SysAudioState>();
    state.desired.store(false, Ordering::SeqCst);
    let taken = state.session.lock().unwrap().take();
    if let Some(s) = taken {
        s.stop.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
mod win_loopback {
    use super::{AtomicBool, Ordering};
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    /// 采集默认播放设备的 loopback 数据，逐块以单声道 f32 交给回调，直到 stop 置位或出错。
    pub fn run(stop: &AtomicBool, mut on_chunk: impl FnMut(&[f32])) -> Result<(), String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
            let res = capture(stop, &mut on_chunk);
            CoUninitialize();
            res
        }
    }

    unsafe fn capture(
        stop: &AtomicBool,
        on_chunk: &mut impl FnMut(&[f32]),
    ) -> Result<(), String> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;

        let pwfx = client.GetMixFormat().map_err(|e| e.to_string())?;
        let channels = (*pwfx).nChannels as usize;
        let bits = (*pwfx).wBitsPerSample;
        if bits != 32 {
            CoTaskMemFree(Some(pwfx as *const _));
            return Err(format!("不支持的采样位深 {bits}（期望 32-bit float）"));
        }

        // 200ms 缓冲（100ns 单位）
        let buf_dur: i64 = 2_000_000;
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            buf_dur,
            0,
            pwfx,
            None,
        );
        CoTaskMemFree(Some(pwfx as *const _));
        init.map_err(|e| e.to_string())?;

        let capture_client: IAudioCaptureClient =
            client.GetService().map_err(|e| e.to_string())?;
        client.Start().map_err(|e| e.to_string())?;

        let mut mono: Vec<f32> = Vec::with_capacity(4096);
        while !stop.load(Ordering::SeqCst) {
            let mut packet = capture_client
                .GetNextPacketSize()
                .map_err(|e| e.to_string())?;
            if packet == 0 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }
            while packet != 0 {
                let mut pdata: *mut u8 = std::ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;
                capture_client
                    .GetBuffer(&mut pdata, &mut num_frames, &mut flags, None, None)
                    .map_err(|e| e.to_string())?;
                let frames = num_frames as usize;
                mono.clear();
                if (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 {
                    mono.resize(frames, 0.0);
                } else if !pdata.is_null() {
                    let samples = std::slice::from_raw_parts(pdata as *const f32, frames * channels);
                    for f in 0..frames {
                        let mut s = 0.0f32;
                        for c in 0..channels {
                            s += samples[f * channels + c];
                        }
                        mono.push(s / channels as f32);
                    }
                }
                on_chunk(&mono);
                capture_client
                    .ReleaseBuffer(num_frames)
                    .map_err(|e| e.to_string())?;
                packet = capture_client
                    .GetNextPacketSize()
                    .map_err(|e| e.to_string())?;
            }
        }
        let _ = client.Stop();
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════
//  其它平台占位（Linux 等，仅为可编译）
// ═══════════════════════════════════════════════════════════════

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn start(_app: &tauri::AppHandle) -> Result<(), String> {
    Err("当前平台暂不支持系统音频采集".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn stop(_app: &tauri::AppHandle) -> bool {
    false
}

// ═══════════════════════════════════════════════════════════════
//  共享退出路径（会话清理 + 自动重启 + 错误事件）
// ═══════════════════════════════════════════════════════════════

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn finish(app: &tauri::AppHandle, my_gen: u64, started: std::time::Instant, err_msg: &str) {
    let state = app.state::<SysAudioState>();
    let mut guard = state.session.lock().unwrap();
    let is_current = guard.as_ref().map_or(false, |s| s.gen == my_gen);
    if !is_current {
        // stop() 或新会话已接管，静默退出（不误报、不误杀）
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(mut s) = guard.take() {
        let _ = s.child.kill();
        let _ = s.child.wait();
    }
    #[cfg(target_os = "windows")]
    let _ = guard.take();
    drop(guard);

    // 运行较久后意外结束（睡眠唤醒、设备切换、显示器重配）→ 自动重启一次
    if started.elapsed() > std::time::Duration::from_secs(30)
        && state.desired.load(Ordering::SeqCst)
    {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if state.desired.load(Ordering::SeqCst) && start(app).is_ok() {
            return;
        }
    }

    let _ = app.emit("system-audio", false);
    let _ = app.emit("system-audio-error", err_msg);
}

// ═══════════════════════════════════════════════════════════════
//  测试（SpectrumAnalyzer 平台无关）
// ═══════════════════════════════════════════════════════════════

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
