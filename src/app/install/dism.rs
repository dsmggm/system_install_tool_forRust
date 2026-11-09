//! DISM 命令封装（← `sys_install.py` 的 `_run_command` / `_run_command_with_progress`
//! / `get_wim_info` / `apply_image`，文档 §6.3）

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use std::os::windows::process::CommandExt;

use crate::utils::{decode_acp, CREATE_NO_WINDOW};

/// 进度回调 `(status, progress)`（对应 Python `progress_callback(status, progress)`）
pub type ProgressFn<'a> = &'a dyn Fn(&str, i32);
/// 取消回调，返回 true 表示用户要求取消
pub type CancelFn<'a> = &'a dyn Fn() -> bool;

/// DISM 命令超时（秒），对应 Python `DISM_TIMEOUT = 3600`
pub const DISM_TIMEOUT: u64 = 3600;
/// bootsect 超时（秒）
pub const BOOTSECT_TIMEOUT: u64 = 60;
/// bcdboot 超时（秒）（Python 常量名为 BCDEDIT_TIMEOUT）
pub const BCDBOOT_TIMEOUT: u64 = 60;

/// 镜像信息（← `get_wim_info` 返回的 dict：index/name/description/size）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WimInfo {
    pub index: i64,
    pub name: String,
    pub description: String,
    pub size: i64,
}

/// 规范化 Windows 路径：正斜杠 → 反斜杠（← `_normalize_path`；空串原样返回）
pub fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    path.replace('/', "\\")
}

/// 只规范化 `/imagefile:` `/applydir:` 两类参数（大小写敏感，与 Python 一致）
fn normalize_cmd(cmd: &[String]) -> Vec<String> {
    cmd.iter()
        .map(|part| {
            if part.starts_with("/imagefile:") || part.starts_with("/applydir:") {
                if let Some((key, val)) = part.split_once(':') {
                    return format!("{key}:{}", normalize_path(val));
                }
            }
            part.clone()
        })
        .collect()
}

/// `shutil.which("dism")` 等价：按 PATH + PATHEXT 顺序查找（仅用于调试日志）
pub fn which_dism() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let exts: Vec<String> = pathext
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("dism{ext}"));
            if cand.is_file() {
                return Some(cand.to_string_lossy().into_owned());
            }
        }
        let bare = dir.join("dism");
        if bare.is_file() {
            return Some(bare.to_string_lossy().into_owned());
        }
    }
    None
}

/// 调试日志：目标文件存在性/大小/父目录（← `_run_command` 中 file_path 分支。
/// 与 Python 相同：循环覆盖，最终取**最后一个** `/imagefile:` 或 `/applydir:` 参数）
fn debug_file_details(normalized: &[String]) {
    let file_path = normalized.iter().rev().find_map(|part| {
        if part.starts_with("/imagefile:") || part.starts_with("/applydir:") {
            part.split_once(':').map(|(_, v)| v.to_string())
        } else {
            None
        }
    });
    let Some(file_path) = file_path else { return };

    log::debug!("目标文件: {file_path}");
    log::debug!("文件存在: {}", Path::new(&file_path).exists());
    if Path::new(&file_path).exists() {
        if let Ok(md) = std::fs::metadata(&file_path) {
            log::debug!("文件大小: {} 字节", md.len());
        }
    } else {
        let parent = Path::new(&file_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        log::debug!("父目录: {parent}");
        log::debug!("父目录存在: {}", Path::new(&parent).exists());
        if Path::new(&parent).exists() {
            match std::fs::read_dir(&parent) {
                Ok(rd) => {
                    let names: Vec<String> = rd
                        .filter_map(|e| e.ok())
                        .take(10)
                        .map(|e| crate::utils::py_str_repr(&e.file_name().to_string_lossy()))
                        .collect();
                    log::debug!("父目录内容: [{}]", names.join(", "));
                }
                Err(e) => log::debug!("无法列出父目录内容: {e}"),
            }
        }
    }
}

/// 读取管道到线程结束（用于 run_command 的 stdout/stderr 收集）
pub(crate) fn spawn_reader(
    pipe: Option<impl std::io::Read + Send + 'static>,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

/// 带超时等待（返回 Some(status) = 正常退出；None = 超时且已 kill）
pub(crate) fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout_secs: u64,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// 执行命令的通用函数（← `_run_command`）
///
/// 返回 `(成功标志, 输出文本)`：
/// - 路径参数规范化 → 调试日志（命令/目标文件/which dism）→ 捕获执行
/// - 超时 → `(false, "命令执行超时")`；异常 → `(false, str(e))`
/// - rc!=0 → `(false, stdout+stderr)`
pub fn run_command(cmd: &[String], timeout_secs: u64, capture_output: bool) -> (bool, String) {
    let normalized = normalize_cmd(cmd);
    log::debug!("执行命令: {}", normalized.join(" "));
    debug_file_details(&normalized);

    log::debug!("DISM 路径检查:");
    let which = which_dism();
    log::debug!(
        "  which('dism'): {}",
        which.as_deref().unwrap_or("None")
    );

    let mut command = Command::new(&normalized[0]);
    command.args(&normalized[1..]);
    command.creation_flags(CREATE_NO_WINDOW);
    if capture_output {
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    } else {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::error!("命令执行失败: {e}");
            return (false, e.to_string());
        }
    };

    if capture_output {
        let t_out = spawn_reader(child.stdout.take());
        let t_err = spawn_reader(child.stderr.take());
        let status = wait_with_timeout(&mut child, timeout_secs);
        let Some(status) = status else {
            let _ = t_out.join();
            let _ = t_err.join();
            log::error!("命令执行超时 ({timeout_secs}秒)");
            return (false, "命令执行超时".to_string());
        };
        let output = decode_acp(&t_out.join().unwrap_or_default())
            + &decode_acp(&t_err.join().unwrap_or_default());
        log::debug!("命令输出:\n{output}");
        let rc = status.code().unwrap_or(-1);
        if rc != 0 {
            log::error!("命令失败 (返回码 {rc})");
            return (false, output);
        }
        (true, output)
    } else {
        let status = wait_with_timeout(&mut child, timeout_secs);
        let Some(status) = status else {
            log::error!("命令执行超时 ({timeout_secs}秒)");
            return (false, "命令执行超时".to_string());
        };
        let rc = status.code().unwrap_or(-1);
        if rc != 0 {
            log::error!("命令失败 (返回码 {rc})");
            return (false, String::new());
        }
        (true, String::new())
    }
}

/// 解析行内首个百分比（← `re.compile(r'(\d+(?:\.\d+)?)\s*%')`.search）
///
/// 手写以避免引入 regex 依赖；语义对齐 Python 回溯：每个起始位尝试
/// 整数 + 可选小数 + 空白 + `%`，取最左匹配。
pub fn parse_percent(line: &str) -> Option<f64> {
    let b = line.as_bytes();
    let len = b.len();
    let is_digit = |c: u8| c.is_ascii_digit();
    let is_ws = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c);

    let mut start = 0;
    while start < len {
        if !is_digit(b[start]) {
            start += 1;
            continue;
        }
        // 整数部分（贪婪）
        let mut i = start;
        while i < len && is_digit(b[i]) {
            i += 1;
        }
        // 尝试两种分支：带小数 / 不带小数
        let mut attempts = vec![i];
        if i + 1 < len && b[i] == b'.' && is_digit(b[i + 1]) {
            let mut j = i + 1;
            while j < len && is_digit(b[j]) {
                j += 1;
            }
            attempts.insert(0, j); // 贪心：优先尝试带小数
        }
        for end in attempts {
            let mut k = end;
            while k < len && is_ws(b[k]) {
                k += 1;
            }
            if k < len && b[k] == b'%' {
                return line[start..end].parse::<f64>().ok();
            }
        }
        start += 1;
    }
    None
}

/// 执行命令并实时解析进度百分比（← `_run_command_with_progress`）
///
/// - 逐行读 stdout（stderr=STDOUT 语义：由调用方合并；此处 stderr 单独收取、
///   结束时拼接，进度解析只依赖 stdout —— DISM 所有进度行均在 stdout）
/// - 每行到达后检查：取消 → kill → `(false, 已读输出 + "\n操作已取消")`
///   超时 → kill → `(false, 已读输出 + "\n命令执行超时")`
/// - 仅整数部分变化时回调；0-100 映射到 `[progress_min, progress_max]`
#[allow(clippy::too_many_arguments)]
pub fn run_command_with_progress(
    cmd: &[String],
    timeout_secs: u64,
    progress_callback: Option<ProgressFn<'_>>,
    cancel_callback: Option<CancelFn<'_>>,
    progress_min: i32,
    progress_max: i32,
    status_text: &str,
) -> (bool, String) {
    let normalized = normalize_cmd(cmd);
    log::debug!("执行命令(带进度): {}", normalized.join(" "));

    let mut command = Command::new(&normalized[0]);
    command
        .args(&normalized[1..])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::error!("命令执行失败: {e}");
            return (false, e.to_string());
        }
    };

    // stderr 单独收取（见模块说明），结束后与 stdout 输出拼接
    let t_err = spawn_reader(child.stderr.take());
    let stdout = child.stdout.take();

    let mut output_lines: Vec<String> = Vec::new();
    let mut last_percent: f64 = -1.0;
    let start_time = Instant::now();

    if let Some(stdout) = stdout {
        let reader = std::io::BufReader::new(stdout);
        for raw in reader.split(b'\n').map_while(Result::ok) {
            let line = decode_acp(&raw);
            let line = line.trim_end_matches(['\r', '\n']);
            output_lines.push(line.to_string());
            log::debug!("  DISM: {line}");

            if cancel_callback.is_some_and(|f| f()) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = t_err.join();
                log::warn!("命令执行被用户取消");
                let mut out = output_lines.join("\n");
                out.push_str("\n操作已取消");
                return (false, out);
            }

            if timeout_secs > 0
                && start_time.elapsed() > std::time::Duration::from_secs(timeout_secs)
            {
                let _ = child.kill();
                let _ = child.wait();
                let _ = t_err.join();
                log::error!("命令执行超时 ({timeout_secs}秒)");
                let mut out = output_lines.join("\n");
                out.push_str("\n命令执行超时");
                return (false, out);
            }

            if let Some(percent) = parse_percent(line) {
                if let Some(cb) = progress_callback {
                    let percent = percent.clamp(0.0, 100.0);
                    // 仅整数部分变化时回调（int(percent) != int(last_percent)）
                    if (percent as i64) != (last_percent as i64) {
                        last_percent = percent;
                        let mapped = progress_min as f64
                            + (progress_max - progress_min) as f64 * (percent / 100.0);
                        let status = if status_text.is_empty() {
                            line.trim()
                        } else {
                            status_text
                        };
                        cb(status, mapped as i32);
                    }
                }
            }
        }
    }

    let status = child.wait();
    let stderr_out = decode_acp(&t_err.join().unwrap_or_default());
    let mut full_output = output_lines.join("\n");
    if !stderr_out.is_empty() {
        full_output.push_str(&stderr_out);
    }

    match status {
        Ok(st) => {
            let rc = st.code().unwrap_or(-1);
            log::debug!("命令完成，返回码: {rc}");
            if rc != 0 {
                log::error!("命令失败 (返回码 {rc})");
                return (false, full_output);
            }
            (true, full_output)
        }
        Err(e) => {
            log::error!("命令执行失败: {e}");
            (false, e.to_string())
        }
    }
}

/// 解析 `dism /get-imageinfo` 输出（← `get_wim_info` 的行级解析部分，
/// 抽出便于离线测试）
pub fn parse_wim_output(output: &str) -> Vec<WimInfo> {
    let mut images: Vec<WimInfo> = Vec::new();
    let mut current: Option<WimInfo> = None;
    for line in output.split('\n') {
        let line = line.trim();
        if line.starts_with("索引") || line.starts_with("Index") {
            if let Some(cur) = current.take() {
                images.push(cur);
            }
            // Python `int(line.split(":")[1].strip())`；防御性跳过损坏行
            let idx = line
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0);
            current = Some(WimInfo {
                index: idx,
                ..Default::default()
            });
        } else if line.starts_with("名称") || line.starts_with("Name") {
            if current.is_none() {
                current = Some(WimInfo::default());
            }
            if let Some((_, v)) = line.split_once(':') {
                current.as_mut().unwrap().name = v.trim().to_string();
            }
        } else if line.starts_with("描述") || line.starts_with("Description") {
            if current.is_none() {
                current = Some(WimInfo::default());
            }
            if let Some((_, v)) = line.split_once(':') {
                current.as_mut().unwrap().description = v.trim().to_string();
            }
        } else if line.starts_with("大小") || line.starts_with("Size") {
            if current.is_none() {
                current = Some(WimInfo::default());
            }
            if let Some((_, v)) = line.split_once(':') {
                let size_str = v.trim().replace(',', "").replace("bytes", "");
                let size = size_str.trim().parse::<i64>().unwrap_or(0);
                current.as_mut().unwrap().size = size;
            }
        }
    }
    if let Some(cur) = current {
        images.push(cur);
    }
    images
}

/// 获取 WIM/ESD 文件中的镜像信息（← `get_wim_info`）
pub fn get_wim_info(wim_path: &str) -> Vec<WimInfo> {
    let wim_path = normalize_path(wim_path);
    log::info!("获取镜像信息: {wim_path}");

    let cmd = vec![
        "dism".to_string(),
        "/get-imageinfo".to_string(),
        format!("/imagefile:{wim_path}"),
    ];
    let (ok, output) = run_command(&cmd, 60, true);
    if !ok {
        log::error!("获取镜像信息失败: {output}");
        return Vec::new();
    }

    let images = parse_wim_output(&output);
    log::debug!("找到 {} 个镜像", images.len());
    images
}

/// 构造 `dism /apply-image` 命令（← `apply_image` 内联构造，抽出便于测试）
///
/// ESD 文件在通用 `/verify` 之外再追加 `/checkintegrity` 与第二个
/// `/verify` —— 与 Python 的 `cmd.append` 顺序一致（含重复项）。
pub fn build_apply_cmd(wim_path: &str, image_index: i64, target_partition: &str) -> Vec<String> {
    let mut cmd = vec![
        "dism".to_string(),
        "/apply-image".to_string(),
        format!("/imagefile:{}", normalize_path(wim_path)),
        format!("/index:{image_index}"),
        format!("/applydir:{}\\", normalize_path(target_partition)),
        "/verify".to_string(),
    ];
    if normalize_path(wim_path).to_lowercase().ends_with(".esd") {
        cmd.push("/checkintegrity".to_string());
        cmd.push("/verify".to_string());
    }
    cmd
}

/// 使用 DISM 应用镜像到目标分区（← `apply_image`）
///
/// 有进度回调时走行级进度解析，否则直接执行（3600s 超时）。
#[allow(clippy::too_many_arguments)]
pub fn apply_image(
    wim_path: &str,
    image_index: i64,
    target_partition: &str,
    progress_callback: Option<ProgressFn<'_>>,
    cancel_callback: Option<CancelFn<'_>>,
    progress_min: i32,
    progress_max: i32,
) -> (bool, String) {
    let wim_path_norm = normalize_path(wim_path);
    let target_partition_norm = normalize_path(target_partition);
    log::info!("开始应用镜像: {wim_path_norm}");
    log::info!("镜像索引: {image_index}, 目标分区: {target_partition_norm}");

    let cmd = build_apply_cmd(wim_path, image_index, target_partition);
    let status_text = format!("正在应用镜像到 {target_partition_norm}...");
    let (ok, output) = if progress_callback.is_some() {
        run_command_with_progress(
            &cmd,
            DISM_TIMEOUT,
            progress_callback,
            cancel_callback,
            progress_min,
            progress_max,
            &status_text,
        )
    } else {
        run_command(&cmd, DISM_TIMEOUT, true)
    };

    if ok {
        log::info!("镜像应用成功: {target_partition_norm}");
    } else {
        log::error!("镜像应用失败: {output}");
    }
    (ok, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    /// ← `_normalize_path`：仅反斜杠化
    #[test]
    fn normalize_path_behavior() {
        assert_eq!(normalize_path(""), "");
        assert_eq!(normalize_path("a/b/c.iso"), "a\\b\\c.iso");
        assert_eq!(normalize_path(r"D:\x\install.wim"), r"D:\x\install.wim");
    }

    /// ← apply_image 命令形状：WIM 不重复 /verify；ESD 追加 checkintegrity + 第二个 /verify
    #[test]
    fn apply_cmd_shapes() {
        assert_eq!(
            build_apply_cmd(r"D:\iso\install.wim", 1, "C:"),
            [
                "dism",
                "/apply-image",
                r"/imagefile:D:\iso\install.wim",
                "/index:1",
                "/applydir:C:\\",
                "/verify",
            ]
        );
        let esd = build_apply_cmd("D:/esd/install.esd", 2, "D:/");
        assert_eq!(
            esd,
            [
                "dism",
                "/apply-image",
                r"/imagefile:D:\esd\install.esd",
                "/index:2",
                "/applydir:D:\\\\",
                "/verify",
                "/checkintegrity",
                "/verify",
            ]
        );
    }

    /// ← 进度正则 `(\d+(?:\.\d+)?)\s*%`：中英文样例
    #[test]
    fn percent_parsing_matches_python_regex() {
        assert_eq!(parse_percent("25.0%"), Some(25.0));
        assert_eq!(parse_percent("正在应用映像  25.0%"), Some(25.0));
        assert_eq!(parse_percent("Applying image  75.0%"), Some(75.0));
        assert_eq!(parse_percent("[===25%===]"), Some(25.0));
        assert_eq!(parse_percent("进度 100% 完成"), Some(100.0));
        // 贪心回溯：优先更长的小数匹配
        assert_eq!(parse_percent("12.34.56%"), Some(34.56));
        assert_eq!(parse_percent("no percent here"), None);
        assert_eq!(parse_percent("100 %"), Some(100.0));
    }

    /// ← get_wim_info 中英文行级解析
    #[test]
    fn wim_info_parse_shapes() {
        let output = "映像索引: #1\n\
            索引: 1\n\
            名称: Windows 10 Pro\n\
            描述: Windows 10 Pro\n\
            大小(字节): 15,000,000,000\n\
            \n\
            索引: 2\n\
            名称: Windows 11 Home\n\
            描述: Windows 11 Home\n\
            大小: 14,152,991,606 bytes\n";

        let images = parse_wim_output(output);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].index, 1);
        assert_eq!(images[0].name, "Windows 10 Pro");
        assert_eq!(images[0].description, "Windows 10 Pro");
        assert_eq!(images[0].size, 15_000_000_000);
        assert_eq!(images[1].index, 2);
        assert_eq!(images[1].name, "Windows 11 Home");
        assert_eq!(images[1].size, 14_152_991_606);

        // 英文格式
        let en = "Index : 1\nName : Windows 10 Pro\nDescription : Pro\nSize : 14,152,991,606 bytes\n";
        let images = parse_wim_output(en);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].index, 1);
        assert_eq!(images[0].size, 14_152_991_606);
    }

    /// run_command 真实执行（只读 echo 级别替代：不存在的命令 → 失败上抛）
    #[test]
    fn run_command_missing_binary_fails_gracefully() {
        let (ok, out) = run_command(&cmd(&["sks_不存在的命令_xyz", "/a:1"]), 5, true);
        assert!(!ok);
        assert!(!out.is_empty());
    }

    /// run_command 成功路径（cmd /c echo 不含窗口地回显）
    #[test]
    fn run_command_success_echo() {
        let (ok, out) = run_command(
            &cmd(&["cmd", "/c", "echo hello-sks"]),
            10,
            true,
        );
        assert!(ok, "{out}");
        assert!(out.contains("hello-sks"));
    }
}
