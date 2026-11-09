//! 通用工具：基础目录、路径、编码、日志设施。

use std::env;
use std::path::PathBuf;

pub mod logging;

use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP};
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

/// CREATE_NO_WINDOW：不创建控制台窗口
/// （Python subprocess/ctypes 各处 creationflags=0x08000000）
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 当前进程是否以**提升（管理员）权限**运行（← Python `main.py::is_admin`）
///
/// 用 `shell32.IsUserAnAdmin` 判断（与 Python 原版同一 API）；
/// 查询失败一律按"非管理员"处理（与 Python 的 `except: return False` 一致）。
///
/// 注意：管理员组成员但未提权的进程，本函数同样返回 `false`
/// （UAC 下 `IsUserAnAdmin` 检查的是提升令牌，而非组成员身份）。
pub fn is_admin() -> bool {
    // SAFETY: 无参数、无输出缓冲的查询型 API
    unsafe { IsUserAnAdmin() != 0 }
}

/// 按 ANSI 代码页（ACP）解码子进程原始字节输出。
///
/// Python `text=True` 在中文系统按 cp936 解码 stdout/stderr；
/// `error` 等关键字判断与 GUI 显示均在解码后进行。
pub fn decode_acp(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    unsafe {
        // SAFETY: 入参缓冲区与长度一致；out 长度按查询值分配
        let wide_len = MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if wide_len <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut buf = vec![0u16; wide_len as usize];
        let written = MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            buf.as_mut_ptr(),
            wide_len,
        );
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&buf[..written as usize])
    }
}

/// Python `str.__repr__` 近似（← `f"...{x!r}"` / `os.listdir` 列表 repr 的逐项形式）。
///
/// 单引号优先；含单引号且不含双引号时改用双引号；反斜杠与控制字符转义。
/// 仅用于日志展示，无需与 CPython 逐字节一致。
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                if (c as u32) < 0x100 {
                    out.push_str(&format!("\\x{:02x}", c as u32));
                } else {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
            }
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// 程序基础目录 = exe 所在目录（config.ini、log/ 均相对此目录）。
/// 与 config.py::get_config_path 的"打包后 exe 同目录"语义一致。
pub fn base_dir() -> PathBuf {
    match env::current_exe() {
        Ok(p) => p
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| env::current_dir().unwrap_or_default()),
        Err(_) => env::current_dir().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 权限查询不得 panic（不同环境下返回值不同，只验证调用安全）
    #[test]
    fn is_admin_query_is_safe() {
        let _ = is_admin();
    }

    /// CREATE_NO_WINDOW 位必须为 0x08000000（Python creationflags=0x08000000）
    #[test]
    fn create_no_window_flag_value() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }
}
