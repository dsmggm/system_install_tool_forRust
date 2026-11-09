//! GHO 中文路径 workaround（← `gui.py::_to_short_path` / `_relative_gho_src`
//! / `_safe_try_remove`，文档 §8.4）
//!
//! Ghost 命令行按系统 ANSI 代码页解析，中文会变成 `?` 导致打不开镜像。
//! 方案：工作目录切到镜像所在目录（系统以 Unicode 处理），`src` 只传
//! 相对文件名；文件名含中文时优先用 8.3 短名，不可用则复制 ASCII 临时副本。

use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

/// 将路径转换为 8.3 短路径（← `_to_short_path`）。失败返回原路径。
pub fn to_short_path(path: &str) -> String {
    if path.is_empty() {
        return path.to_string();
    }
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 260];
    // SAFETY: wide 为合法 NUL 结尾宽字符串，buf 容量与传入一致
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 {
        return path.to_string();
    }
    if n as usize >= buf.len() {
        buf = vec![0u16; n as usize + 1];
        // SAFETY: 同上，重新以更大的缓冲调用
        let n2 = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        if n2 == 0 {
            return path.to_string();
        }
        buf.truncate(n2 as usize);
    } else {
        buf.truncate(n as usize);
    }
    let s = String::from_utf16_lossy(&buf);
    if s.is_empty() {
        path.to_string()
    } else {
        s
    }
}

/// 带重试地删除临时文件（← `_safe_try_remove`，4 次 × 0.5s）。
pub fn safe_try_remove(path: &str) {
    if path.is_empty() || !Path::new(path).exists() {
        return;
    }
    for _ in 0..4 {
        if std::fs::remove_file(path).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Ghost 镜像参数三元组（← `_relative_gho_src` 返回值）：
/// `(src_name, workdir, cleanup)`，其中 cleanup 非空时需在 Ghost 结束后调用。
pub struct GhoSrc {
    pub src_name: String,
    pub workdir: String,
    pub cleanup: Option<PathBuf>,
}

/// 为 Ghost 生成"相对文件名 + 工作目录"的镜像参数（← `_relative_gho_src`）。
pub fn relative_gho_src(gho_path: &str) -> GhoSrc {
    let p = Path::new(gho_path);
    let gho_dir = p
        .parent()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let gho_name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if gho_name.is_empty() || gho_dir.is_empty() {
        return GhoSrc {
            src_name: gho_name,
            workdir: gho_dir,
            cleanup: None,
        };
    }

    // 文件名本身为 ASCII：直接用
    if gho_name.is_ascii() {
        return GhoSrc {
            src_name: gho_name,
            workdir: gho_dir,
            cleanup: None,
        };
    }

    // 文件名含中文：改用 8.3 短文件名
    let short = to_short_path(gho_path);
    let short_name = Path::new(&short)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !short_name.is_empty() && short_name.is_ascii() {
        return GhoSrc {
            src_name: short_name,
            workdir: gho_dir,
            cleanup: None,
        };
    }

    // 短名不可用：同目录复制一个 ASCII 临时副本
    let stem = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dst = Path::new(&gho_dir).join(format!("sks_img_{:08x}.GHO", (stem as u32) ^ std::process::id()));
    match std::fs::copy(gho_path, &dst) {
        Ok(_) => {
            let name = dst
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            GhoSrc {
                src_name: name,
                workdir: gho_dir,
                cleanup: Some(dst),
            }
        }
        Err(e) => {
            log::warn!("生成 ASCII 临时镜像副本失败: {e}");
            GhoSrc {
                src_name: gho_name,
                workdir: gho_dir,
                cleanup: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASCII 文件名直接返回原名；工作目录为父目录，无清理。
    #[test]
    fn ascii_name_passthrough() {
        let r = relative_gho_src(r"D:\images\win11.GHO");
        assert_eq!(r.src_name, "win11.GHO");
        assert_eq!(r.workdir, r"D:\images");
        assert!(r.cleanup.is_none());
    }

    /// ← Python `_to_short_path` 语义：不存在的路径返回原值。
    #[test]
    fn short_path_missing_returns_input() {
        let missing = r"D:\no\such\file_不存在.exe";
        assert_eq!(to_short_path(missing), missing);
        assert_eq!(to_short_path(""), "");
    }
}
