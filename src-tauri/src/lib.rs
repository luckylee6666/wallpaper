mod system_audio;
mod wallpaper;

use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Listener, Manager, Wry};

/// 建托盘菜单；返回 (系统音频, 麦克风) 两个勾选项供状态同步。
fn build_tray(app: &tauri::App) -> tauri::Result<(CheckMenuItem<Wry>, CheckMenuItem<Wry>)> {
    let theme_names = [
        "夜色 Nocturnal",
        "霓虹东京 Neon Tokyo",
        "赛博森林 Cyber Forest",
        "水墨 Ink Wash",
        "极简单色 Minimal Mono",
        "晨曦 Dawn",
    ];
    let mut themes = SubmenuBuilder::new(app, "配色主题");
    for (i, name) in theme_names.iter().enumerate() {
        themes = themes.text(format!("theme-{i}"), *name);
    }
    let themes = themes.build()?;

    let pick = MenuItem::with_id(app, "pick-file", "选择音乐文件…", true, None::<&str>)?;
    let play = MenuItem::with_id(app, "play-pause", "播放 / 暂停", true, None::<&str>)?;
    let sys_audio =
        CheckMenuItem::with_id(app, "sys-audio", "系统音频（全局听歌律动）", true, false, None::<&str>)?;
    let mic = CheckMenuItem::with_id(app, "mic", "麦克风驱动", true, false, None::<&str>)?;
    let clock = CheckMenuItem::with_id(app, "clock", "显示时钟", true, false, None::<&str>)?;
    let interactive =
        CheckMenuItem::with_id(app, "interactive", "互动模式（壁纸可点击）", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = MenuBuilder::new(app)
        .item(&pick)
        .item(&play)
        .item(&sys_audio)
        .item(&mic)
        .separator()
        .item(&themes)
        .item(&clock)
        .item(&interactive)
        .separator()
        .item(&quit)
        .build()?;

    let interactive_ref = interactive.clone();
    let sys_audio_ref = sys_audio.clone();
    TrayIconBuilder::with_id("tray")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            match event.id().as_ref() {
                "quit" => app.exit(0),
                "pick-file" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let file = rfd::AsyncFileDialog::new()
                            .add_filter("音频", &["mp3", "wav", "flac", "m4a", "aac", "ogg"])
                            .pick_file()
                            .await;
                        if let Some(file) = file {
                            let _ = app
                                .emit("play-audio-file", file.path().to_string_lossy().to_string());
                        }
                    });
                }
                "play-pause" => {
                    let _ = app.emit("toggle-play", ());
                }
                "mic" => {
                    let _ = app.emit("toggle-mic", ());
                }
                "clock" => {
                    let _ = app.emit("toggle-clock", ());
                }
                "sys-audio" => {
                    let on = sys_audio_ref.is_checked().unwrap_or(false);
                    let result = if on {
                        system_audio::start(app)
                    } else {
                        system_audio::stop(app);
                        Ok(())
                    };
                    match result {
                        Ok(()) => {
                            let _ = app.emit("system-audio", on);
                        }
                        Err(e) => {
                            let _ = sys_audio_ref.set_checked(false);
                            let _ = app.emit("system-audio-error", e);
                        }
                    }
                }
                "interactive" => {
                    let on = interactive_ref.is_checked().unwrap_or(false);
                    // 所有壁纸窗口（主屏 + 各副屏）一起切换
                    for (_label, w) in app.webview_windows() {
                        let _ = w.set_ignore_cursor_events(!on);
                        // 桌面层收不到点击（图标层盖在上面），互动时升到普通层，退出降回
                        wallpaper::set_interactive(&w, on);
                        if on {
                            let _ = w.set_focus();
                        }
                    }
                    let _ = app.emit("interactive-mode", on);
                }
                other => {
                    if let Some(idx) = other.strip_prefix("theme-") {
                        if let Ok(n) = idx.parse::<usize>() {
                            let _ = app.emit("set-theme", n);
                        }
                    }
                }
            }
        })
        .build(app)?;
    Ok((sys_audio, mic))
}

/// Rust 侧作为托盘勾选态的唯一同步点：
/// - `system-audio` 事件（无论来自菜单、自动重启失败还是互斥关闭）→ 同步系统音频勾选
/// - `source-changed` 事件（前端麦克风/文件音源变化）→ 同步麦克风勾选 + 互斥停掉系统音频
fn register_state_sync(app: &tauri::App, sys_audio: CheckMenuItem<Wry>, mic: CheckMenuItem<Wry>) {
    let handle = app.handle().clone();
    let sys_ref = sys_audio.clone();
    app.listen("system-audio", move |event| {
        let on = event.payload() == "true";
        let item = sys_ref.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = item.set_checked(on);
        });
    });

    let handle = app.handle().clone();
    app.listen("source-changed", move |event| {
        let state = event.payload().trim_matches('"').to_string();
        let app = handle.clone();
        let mic_item = mic.clone();
        let sys_item = sys_audio.clone();
        let _ = handle.run_on_main_thread(move || match state.as_str() {
            "mic-on" => {
                let _ = mic_item.set_checked(true);
                if system_audio::stop(&app) {
                    let _ = sys_item.set_checked(false);
                    let _ = app.emit("system-audio", false);
                }
            }
            "mic-off" => {
                let _ = mic_item.set_checked(false);
            }
            "file" => {
                if system_audio::stop(&app) {
                    let _ = sys_item.set_checked(false);
                    let _ = app.emit("system-audio", false);
                }
            }
            _ => {}
        });
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(system_audio::SysAudioState::new())
        .setup(|app| {
            // 不占 Dock，只留菜单栏托盘
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let window = app.get_webview_window("main").expect("main window missing");

            // 铺满整个屏幕（含菜单栏之下的区域）
            if let Ok(Some(monitor)) = window.current_monitor() {
                let _ = window.set_position(*monitor.position());
                let _ = window.set_size(*monitor.size());
            }

            wallpaper::attach(&window);

            // 壁纸默认点击穿透，桌面图标照常可用；托盘可切互动模式
            let _ = window.set_ignore_cursor_events(true);

            // 多显示器：主窗口占当前显示器，其余显示器各建一个壁纸窗口。
            // 事件全部走 app.emit 广播，所有窗口同步律动/主题/互动模式。
            let main_monitor_pos = window.current_monitor().ok().flatten().map(|m| *m.position());
            if let Ok(monitors) = window.available_monitors() {
                for (i, m) in monitors.iter().enumerate() {
                    if Some(*m.position()) == main_monitor_pos {
                        continue;
                    }
                    let label = format!("wp{i}");
                    match tauri::WebviewWindowBuilder::new(
                        app,
                        &label,
                        tauri::WebviewUrl::App("index.html".into()),
                    )
                    .decorations(false)
                    .resizable(false)
                    .shadow(false)
                    .skip_taskbar(true)
                    .visible_on_all_workspaces(true)
                    .disable_drag_drop_handler()
                    .build()
                    {
                        Ok(w) => {
                            let _ = w.set_position(*m.position());
                            let _ = w.set_size(*m.size());
                            wallpaper::attach(&w);
                            let _ = w.set_ignore_cursor_events(true);
                        }
                        Err(e) => eprintln!("[multi-display] create {label} failed: {e}"),
                    }
                }
            }

            let (sys_audio, mic) = build_tray(app)?;
            register_state_sync(app, sys_audio, mic);

            // 调试用：YINLANG_SYS_AUDIO=1 时启动即打开系统音频采集
            if std::env::var("YINLANG_SYS_AUDIO").as_deref() == Ok("1") {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if let Err(e) = system_audio::start(&handle) {
                        eprintln!("[sys-audio] autostart failed: {e}");
                    } else {
                        let _ = handle.emit("system-audio", true);
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
