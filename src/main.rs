//! 硕科斯系统安装工具 — 入口（← main.py）
//!
//! 职责：日志初始化 → 启动 GUI。

// GUI 子系统：启动时不弹黑色控制台窗口，直接显示窗口。
// 必须是 crate 根属性，且位于所有 item 之前。
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod config;
mod ui;
mod utils;

use std::env;
use std::process;

use windows_sys::Win32::Foundation::{GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetFileType, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{
    AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_TOPMOST};

/// 版本号（Python 版 1.0.6 → Rust 版从 2.0.0 起，见文档 §2）
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 主窗口基础标题（含版本，不含动态状态）。
pub fn app_base_title() -> String {
    format!("硕科斯系统安装工具 V{}", VERSION)
}

/// 弹错误消息框。
///
/// GUI 子系统进程没有控制台，`println!`/`eprintln!` 的内容无处显示，
/// 启动期的致命错误只能用消息框告知用户。
fn show_error(caption: &str, text: &str) {
    let cap: Vec<u16> = caption.encode_utf16().chain(std::iter::once(0)).collect();
    let body: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 两个字符串都已补 NUL 结尾，且在调用返回前一直存活；
    // hwnd 传空表示没有父窗口。
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            cap.as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST,
        );
    }
}

/// 标准句柄是否可用（不是 NULL、不是 INVALID_HANDLE_VALUE、类型已知）。
///
/// # Safety
/// 需传入有效的句柄；本函数只做查询，不修改句柄状态。
unsafe fn is_usable_std_handle(h: HANDLE) -> bool {
    !h.is_null() && h != INVALID_HANDLE_VALUE && GetFileType(h) != 0
}

/// 把标准输出挂回父进程的控制台。
///
/// GUI 子系统进程默认没有控制台，标准句柄是无效的：
/// - stdout 被父进程重定向到文件/管道时，句柄由系统继承而来，直接可用；
/// - 否则（正常从 cmd/PowerShell 启动）需要 `AttachConsole(ATTACH_PARENT_PROCESS)`
///   挂回父进程的控制台，`--dump-disks` 的 JSON 才能打到屏幕上。
///
/// 返回 stdout 是否可用（不可用时只有文件日志能记录）。
fn attach_parent_console() -> bool {
    // SAFETY: 启动阶段单线程调用；只做句柄查询、挂接控制台、打开 CONOUT$。
    unsafe {
        if is_usable_std_handle(GetStdHandle(STD_OUTPUT_HANDLE)) {
            return true;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            // 父进程没有控制台（例如从资源管理器双击启动）。
            return false;
        }
        // AttachConsole 之后标准句柄应已指向控制台；个别情况下仍为空，
        // 这时用 CONOUT$ 显式重开 stdout。
        if is_usable_std_handle(GetStdHandle(STD_OUTPUT_HANDLE)) {
            return true;
        }
        let mut name = [0u16; 8];
        for (i, ch) in "CONOUT$".encode_utf16().enumerate() {
            name[i] = ch;
        }
        name["CONOUT$".encode_utf16().count()] = 0;
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return false;
        }
        SetStdHandle(STD_OUTPUT_HANDLE, h);
        true
    }
}

/// 安装 panic 钩子。
///
/// GUI 子系统没有控制台，默认的 panic 输出会丢失、程序静默退出；
/// 这里改成弹消息框，让用户能看到出错位置。
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "未知错误".to_string()
        };
        let detail = match info.location() {
            Some(loc) => format!("{}:{}:{} — {msg}", loc.file(), loc.line(), loc.column()),
            None => msg,
        };
        log::error!("panic: {detail}");
        show_error("程序发生严重错误", &detail);
    }));
}

fn main() {
    // GUI 子系统没有控制台，panic 必须用消息框显示。
    install_panic_hook();

    // 1. 日志先行（对应 main.py 模块级 setup_logging()）
    let base = utils::base_dir();
    if let Err(e) = utils::logging::init(&base) {
        show_error("日志初始化失败", &e.to_string());
    }

    log::info!("硕科斯系统安装工具启动 V{}", VERSION);

    // 2. --dump-disks 调试开关（对照 `python GetDiskInfo.py` 的 JSON 输出，里程碑 2 验收用）。
    if env::args().any(|a| a == "--dump-disks") {
        // GUI 子系统默认没有控制台，先挂回父进程的控制台，stdout 才能输出内容。
        let has_console = attach_parent_console();
        let disks = app::disk_info::enumerate();
        let json = app::disk_info::to_json(&disks);
        if has_console {
            println!("{json}");
        } else {
            // 没有可用控制台（例如从资源管理器启动），写日志文件，避免
            // 向无效的 stdout 写入导致 panic。
            log::info!("--dump-disks 输出:\n{json}");
        }
        return;
    }

    // 3. 加载配置（不存在则生成默认，对应文档 §8.1 步骤 1）
    let app_config = config::settings::AppConfig::load(None);

    // 3.1 设置主题模式并初始化
    use ui::theme_mode::ThemeMode;
    use ui::theme;
    
    let theme_mode = match app_config.theme_mode.as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::Auto,
    };
    theme::set_mode(theme_mode);

    // 4. 启动 GUI（阻塞至窗口关闭）
    if let Err(e) = ui::run(app_config) {
        log::error!("GUI 运行失败: {e}");
        show_error("程序启动失败", &e.to_string());
        process::exit(1);
    }
}
