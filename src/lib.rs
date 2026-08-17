use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{Manager, RunEvent, WebviewWindow};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const DSH_URL: &str = "http://127.0.0.1:3080";
const READY_TIMEOUT: Duration = Duration::from_secs(90);
const MARKET_SPEC: &str = "dshmarket";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct DshProcess(Mutex<Option<Child>>);

fn is_ready() -> bool {
    let addr: std::net::SocketAddr = "127.0.0.1:3080".parse().unwrap();
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

fn wait_until_ready() -> bool {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if is_ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

#[cfg(windows)]
fn spawn_cmd(line: &str) -> std::io::Result<Child> {
    std::process::Command::new("cmd")
        .args(["/C", line])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
}

#[cfg(not(windows))]
fn spawn_cmd(line: &str) -> std::io::Result<Child> {
    std::process::Command::new("sh").args(["-c", line]).spawn()
}

fn spawn_dsh() -> std::io::Result<Child> {
    let dsh_cmd = std::env::var("DSH_DESKTOP_DSH_CMD").unwrap_or_else(|_| "dsh".into());
    spawn_cmd(&format!("{} web", dsh_cmd))
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .output();
    }
}

fn dsh_home() -> PathBuf {
    if let Ok(h) = std::env::var("DSH_HOME") {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE")
            .map(|p| PathBuf::from(p).join(".dsh"))
            .unwrap_or_else(|_| PathBuf::from(".dsh"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".dsh")
    }
}

fn profile_package_json() -> PathBuf {
    dsh_home().join("profiles").join("web").join("package.json")
}

fn market_installed() -> bool {
    let Ok(text) = std::fs::read_to_string(profile_package_json()) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    v.get("dependencies")
        .and_then(|d| d.get(MARKET_SPEC))
        .is_some()
}

/// 在系统 shell 中执行命令，捕获输出尾部。返回 (是否成功, 输出尾部)
fn run_cmd(line: &str, extra_env: Option<(&str, &str)>, timeout: Duration) -> (bool, String) {
    let mut cmd = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    cmd.args(if cfg!(windows) { ["/C", line] } else { ["-c", line] })
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("SYSTEMROOT", std::env::var("SYSTEMROOT").unwrap_or_default())
        .env("USERPROFILE", std::env::var("USERPROFILE").unwrap_or_default())
        .env("DSH_HOME", std::env::var("DSH_HOME").unwrap_or_default())
        .env("npm_config_registry", std::env::var("npm_config_registry").unwrap_or_default())
        .env("NODE_OPTIONS", std::env::var("NODE_OPTIONS").unwrap_or_default())
        .env("APPDATA", std::env::var("APPDATA").unwrap_or_default())
        .env("LOCALAPPDATA", std::env::var("LOCALAPPDATA").unwrap_or_default())
        .env("TEMP", std::env::var("TEMP").unwrap_or_default())
        .env("TMP", std::env::var("TMP").unwrap_or_default())
        .env("HOMEDRIVE", std::env::var("HOMEDRIVE").unwrap_or_default())
        .env("HOMEPATH", std::env::var("HOMEPATH").unwrap_or_default())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    if let Some((k, v)) = extra_env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (false, format!("无法启动命令: {}", e)),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_tree(child.id());
                    return (false, "命令执行超时".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => return (false, "无法等待命令".into()),
        }
    }
    let out = child.wait_with_output();
    let captured = match out {
        Ok(o) => {
            let mut all = String::from_utf8_lossy(&o.stdout).to_string();
            all.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.success(), all)
        }
        Err(e) => (false, format!("无法读取输出: {}", e)),
    };
    let tail: String = captured.1.chars().rev().take(1200).collect::<Vec<_>>().into_iter().rev().collect();
    (captured.0, tail)
}

/// 幂等确保插件市场已安装到 web profile
fn ensure_plugin_market() -> Result<(), String> {
    if market_installed() {
        return Ok(());
    }
    let line = format!("dsh plugin --profile web add {} -w", MARKET_SPEC);
    let (ok, tail) = run_cmd(&line, None, Duration::from_secs(180));
    if ok && market_installed() {
        return Ok(());
    }
    // 用户环境失败(如网络) → npmmirror 镜像兜底重试
    let (ok2, tail2) = run_cmd(
        &line,
        Some(("npm_config_registry", "https://registry.npmmirror.com")),
        Duration::from_secs(180),
    );
    if ok2 && market_installed() {
        Ok(())
    } else {
        Err(format!(
            "安装插件市场失败. 首次: {} | 镜像重试: {} | 输出: {}",
            if ok { "ok" } else { "fail" },
            if ok2 { "ok" } else { "fail" },
            if tail.is_empty() { &tail2 } else { &tail }
        ))
    }
}

fn emit(window: &WebviewWindow, payload: &serde_json::Value) {
    let js = format!(
        "window.__setStage && window.__setStage({});",
        serde_json::to_string(payload).unwrap_or_default()
    );
    let _ = window.eval(&js);
}

fn check_dsh() -> bool {
    let (ok, _) = run_cmd("dsh --version", None, Duration::from_secs(30));
    ok
}

fn boot_backend(window: WebviewWindow, app: tauri::AppHandle) {
    emit(&window, &json!({ "stage": "checking", "progress": 8 }));
    if !check_dsh() {
        emit(
            &window,
            &json!({ "stage": "error", "message": "未找到 dsh 命令。请先运行: npm install -g @deepseek-ai/dsh" }),
        );
        return;
    }

    emit(&window, &json!({ "stage": "market", "progress": 15 }));
    if let Err(msg) = ensure_plugin_market() {
        emit(&window, &json!({ "stage": "market-warn", "message": msg }));
    } else {
        emit(&window, &json!({ "stage": "market-done", "progress": 30 }));
    }

    let borrowed = is_ready();
    if !borrowed {
        emit(&window, &json!({ "stage": "starting", "progress": 40 }));
        match spawn_dsh() {
            Ok(child) => {
                if let Some(state) = app.try_state::<DshProcess>() {
                    *state.0.lock().unwrap() = Some(child);
                }
            }
            Err(e) => {
                emit(
                    &window,
                    &json!({ "stage": "error", "message": format!("无法启动 dsh: {}", e) }),
                );
                return;
            }
        }
    }

    emit(&window, &json!({ "stage": "waiting", "progress": 55 }));
    if wait_until_ready() {
        emit(&window, &json!({ "stage": "ready", "progress": 100 }));
        std::thread::sleep(Duration::from_millis(250));
        let _ = window.navigate(DSH_URL.parse().unwrap());
    } else {
        if !borrowed {
            if let Some(state) = app.try_state::<DshProcess>() {
                if let Some(child) = state.0.lock().unwrap().take() {
                    kill_tree(child.id());
                }
            }
        }
        emit(
            &window,
            &json!({ "stage": "error", "message": "DSH 后端未在 90 秒内就绪，已自动停止。请关闭窗口后重试。" }),
        );
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .manage(DshProcess(Mutex::new(None)))
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || boot_backend(window, app_handle));
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            let is_exit = matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. });
            if is_exit {
                if let Some(state) = app_handle.try_state::<DshProcess>() {
                    if let Some(child) = state.0.lock().unwrap().take() {
                        kill_tree(child.id());
                    }
                }
            }
        });
}