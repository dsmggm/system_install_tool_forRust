//! 调试工具复制（← `copy_tools.py`）
//!
//! 将用户配置的 Tools 文件夹复制到 `<目标盘符>\Windows\panther\TestTools`。
//! 未配置或源不存在 → 跳过（视为成功，不影响安装流程）。
//!
//! 注意：Python `os.path.join("C:", "Windows", ...)` 得到盘符相对路径
//! `"C:Windows\panther\TestTools"`（首段拼接不加反斜杠）—— 此处保持同样的
//! 字符串构造，最终由 Windows API 按盘符当前目录语义解析。
//!
//! 注：本模块**不启动任何子进程**（仅目录/文件复制），无黑色控制台窗口风险。

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// 进度回调（对应 Python `progress_callback(status: str, progress: int)`）
pub type ProgressFn<'a> = &'a dyn Fn(&str, i32);

/// 获取 Tools 源目录（← `get_tools_source_dir`）
///
/// - 空/纯空白 → debug 后返回 None
/// - 是目录 → debug 后返回 Some
/// - 不存在 → warning 后返回 None
pub fn get_tools_source_dir(source_path: &str) -> Option<String> {
    let source_dir = source_path.trim();
    if source_dir.is_empty() {
        log::debug!("未配置 Tools 源路径，跳过");
        return None;
    }
    if Path::new(source_dir).is_dir() {
        log::debug!("找到 Tools 源目录: {source_dir}");
        return Some(source_dir.to_string());
    }
    log::warn!("未找到 Tools 源目录: {source_dir}");
    None
}

/// 目标目录构造（← `os.path.join(partition, "Windows", "panther", "TestTools")`）
///
/// Python：`"C:" + "Windows" + ...` —— 盘符后直接拼接（ntpath 不加分隔符），
/// 与 `rstrip("\\")` + 不足冒号补冒号的预处理一一对应。
fn panther_target_dir(windows_partition: &str) -> String {
    let mut p = windows_partition.trim_end_matches('\\').to_string();
    if !p.ends_with(':') {
        p.push(':');
    }
    format!("{p}Windows\\panther\\TestTools")
}

/// 递归复制目录（← `shutil.copytree(..., dirs_exist_ok=True)` 的合并语义）
///
/// 默认 `copytree` 使用 `copy2` 保留 mtime；此处仅复制内容（对工具部署无行为差异）。
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 将 Tools 文件夹复制到目标系统的 Windows\panther 目录下（← `copy_tools_to_panther`）
///
/// 返回 `(成功标志, 输出文本)`，与 Python 元组一致：
/// - 未配置/未找到源 → `(true, "未配置或未找到…跳过…")`
/// - 复制成功 → `(true, "调试工具已复制到 …")`
/// - 异常 → `(false, "复制调试工具失败: …")`
pub fn copy_tools_to_panther(
    windows_partition: &str,
    progress_callback: Option<ProgressFn<'_>>,
    source_path: &str,
) -> (bool, String) {
    log::info!("开始复制调试工具到目标系统 panther 目录");
    match copy_inner(windows_partition, progress_callback, source_path) {
        Ok(msg) => (true, msg),
        Err(e) => {
            log::error!("复制 Tools 文件夹失败: {e}");
            (false, format!("复制调试工具失败: {e}"))
        }
    }
}

fn copy_inner(
    windows_partition: &str,
    progress_callback: Option<ProgressFn<'_>>,
    source_path: &str,
) -> Result<String, String> {
    let target_dir = panther_target_dir(windows_partition);

    let source_dir = match get_tools_source_dir(source_path) {
        Some(d) => d,
        None => {
            let msg = "未配置或未找到 Tools 源路径，跳过复制调试工具";
            log::info!("{msg}");
            return Ok(msg.to_string());
        }
    };

    copy_core(&source_dir, &target_dir, progress_callback)
}

/// 复制核心（回调 + 日志 + 目录合并），抽出来便于用临时目录闭环测试
fn copy_core(
    source_dir: &str,
    target_dir: &str,
    progress_callback: Option<ProgressFn<'_>>,
) -> Result<String, String> {
    if let Some(cb) = progress_callback {
        cb("正在复制调试工具...", 99);
    }

    log::info!("源目录: {source_dir}");
    log::info!("目标目录: {target_dir}");

    fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;
    copy_tree(Path::new(source_dir), Path::new(target_dir)).map_err(|e| e.to_string())?;
    log::info!("Tools 文件夹复制完成: {target_dir}");

    if let Some(cb) = progress_callback {
        cb("调试工具复制完成", 100);
    }

    Ok(format!("调试工具已复制到 {target_dir}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python `os.path.join("C:", "Windows", "panther", "TestTools")`：
    /// 盘符后不加分隔符 → `C:Windows\panther\TestTools`
    #[test]
    fn target_dir_matches_python_ntpath_join() {
        assert_eq!(
            panther_target_dir("C:"),
            "C:Windows\\panther\\TestTools"
        );
        // rstrip("\\") + 补冒号
        assert_eq!(
            panther_target_dir("D:\\"),
            "D:Windows\\panther\\TestTools"
        );
        assert_eq!(
            panther_target_dir("E"),
            "E:Windows\\panther\\TestTools"
        );
    }

    /// 源目录判定三分支
    #[test]
    fn source_dir_branches() {
        assert_eq!(get_tools_source_dir(""), None);
        assert_eq!(get_tools_source_dir("   "), None);
        assert_eq!(get_tools_source_dir("Z:\\不存在的目录\\x"), None);
        assert!(get_tools_source_dir(std::env::temp_dir().to_str().unwrap()).is_some());
    }

    /// 未配置源 → 跳过且成功，不产生任何文件
    #[test]
    fn skip_when_not_configured() {
        let (ok, msg) = copy_tools_to_panther("C:", None, "");
        assert!(ok);
        assert_eq!(msg, "未配置或未找到 Tools 源路径，跳过复制调试工具");
    }

    /// 进度回调顺序与真实复制（temp 目录内闭环，无破坏性）
    #[test]
    fn copy_and_progress_order() {
        use std::cell::RefCell;
        let root = std::env::temp_dir().join(format!("sks_tools_{}", std::process::id()));
        let src = root.join("src");
        let dst = root.join("dst");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.bin"), b"hello").unwrap();
        fs::write(src.join("sub/b.bin"), b"world").unwrap();

        let calls: RefCell<Vec<(String, i32)>> = RefCell::new(Vec::new());
        let cb: ProgressFn<'_> = &|status: &str, p: i32| {
            calls.borrow_mut().push((status.to_string(), p));
        };

        let msg = copy_core(
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
            Some(cb),
        )
        .expect("复制应成功");

        // 内容合并复制（dirs_exist_ok 语义）
        assert_eq!(fs::read(dst.join("a.bin")).unwrap(), b"hello");
        assert_eq!(fs::read(dst.join("sub/b.bin")).unwrap(), b"world");
        assert!(msg.starts_with("调试工具已复制到 "));

        let c = calls.take();
        assert_eq!(
            c,
            vec![
                ("正在复制调试工具...".to_string(), 99),
                ("调试工具复制完成".to_string(), 100),
            ]
        );

        let _ = fs::remove_dir_all(&root);
    }
}
