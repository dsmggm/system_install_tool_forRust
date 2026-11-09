//! diskpart 封装（← `disk_manager.py`，文档 §6.2）
//!
//! 所有操作通过生成 diskpart 脚本文件、`diskpart /s "path"` 执行：
//! - `CREATE_NO_WINDOW` 隐藏控制台窗口
//! - 300s 超时保护
//! - 成功判据：returncode==0 且输出不含 `error`（大小写不敏感）
//! - 临时脚本文件执行后删除
//!
//! 每个操作拆成 `_script` 构造器（纯函数，可用于 dry-run 预览与黄金测试）
//! 与同名执行函数（构造 → 执行 → 按 Python 打日志）。
//!
//! **权限说明**：`diskpart.exe` 自带"要求管理员权限"清单，普通用户进程调用
//! `CreateProcess` 会直接失败（错误 740 = ERROR_ELEVATION_REQUIRED），
//! 因此磁盘写操作（分区/格式化）必须在管理员权限下执行；本模块只做诊断提示。

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use std::os::windows::process::CommandExt;

use crate::utils::{decode_acp, is_admin, CREATE_NO_WINDOW};

/// diskpart 操作返回值：`(成功标志, 输出文本)`（对应 Python `(ok, output)` 元组）
pub type DiskpartResult = (bool, String);

/// 默认超时（秒），对应 Python `_run_diskpart` 的 timeout=300
const DEFAULT_TIMEOUT: u64 = 300;

/// Windows 错误码 740 = ERROR_ELEVATION_REQUIRED（"请求的操作需要提升"）
///
/// `diskpart.exe` 自带"要求管理员权限"清单：非提升进程调用 `CreateProcess`
/// 直接失败（而不是运行后报错），因此必须给出明确提示。
const ERROR_ELEVATION_REQUIRED: i32 = 740;

/// 权限不足（错误 740）的**统一中文提示**（与系统语言无关）。
///
/// Windows 原始错误文本随系统语言变化：
/// 中文系统为"请求的操作需要提升。(os error 740)"，
/// 英文系统为"The requested operation requires elevation. (os error 740)"。
/// 本程序界面为中文，故固定给出中文说明，避免在英文系统上只显示英文。
const ELEVATION_REQUIRED_HINT: &str =
    "磁盘分区/格式化需要管理员权限：请以管理员身份运行本程序（Windows 错误 740：请求的操作需要提升）";

/// 组装错误 740 的提示文本：中文说明 + 系统原始错误文本（便于排查）。
///
/// `system_msg` 为空时只返回中文说明。
fn elevation_required_message(system_msg: &str) -> String {
    let msg = system_msg.trim();
    if msg.is_empty() {
        ELEVATION_REQUIRED_HINT.to_string()
    } else {
        format!("{ELEVATION_REQUIRED_HINT}\n系统提示: {msg}")
    }
}

/// 数据分区规格（对应 Python `data_parts` 列表中的 dict）
#[derive(Debug, Clone, PartialEq)]
pub struct DataPartSpec {
    /// 分区大小（MB）；None = 使用全部剩余空间
    pub size_mb: Option<i64>,
    /// 文件系统，如 "ntfs"
    pub fs: String,
}

impl DataPartSpec {
    pub fn new(size_mb: Option<i64>, fs: &str) -> Self {
        Self {
            size_mb,
            fs: fs.to_string(),
        }
    }

    /// Python dict 字面量风格，用于日志输出（`{'size_mb': 100000, 'fs': 'ntfs'}`）
    fn py_repr(&self) -> String {
        let size = match self.size_mb {
            Some(v) => v.to_string(),
            None => "None".to_string(),
        };
        format!("{{'size_mb': {size}, 'fs': '{}'}}", self.fs)
    }
}

/// 一键分区实际分配的盘符（对应 Python `assigned_letters` 字典
/// `{"esp": "Z", "data": ["C", "D"]}`）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssignedLetters {
    pub esp: Option<char>,
    pub data: Vec<char>,
}

/// 临时脚本文件守卫：Drop 时无条件删除（对应 Python finally 中的 os.unlink）
struct TempScript {
    path: PathBuf,
}

impl TempScript {
    fn create(commands: &[String]) -> Result<Self, String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sks_diskpart_{}_{}.txt", std::process::id(), n));
        let script = commands.join("\n");
        log::debug!("diskpart script:\n{script}");
        fs::write(&path, script).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 输出是否包含 diskpart 逻辑错误（Python `"error" in output.lower()`）
fn output_has_error(output: &str) -> bool {
    output.to_lowercase().contains("error")
}

/// 输出是否表示"权限不足/需要提升"（中英文特征串，仅用于日志诊断）
fn output_needs_elevation(output: &str) -> bool {
    let low = output.to_lowercase();
    low.contains("requires elevation") || low.contains("需要提升")
}

/// 执行 diskpart 脚本的核心函数（← `_run_diskpart`）
///
/// 流程：写临时脚本 → `diskpart /s path`（CREATE_NO_WINDOW）→ 等待（默认 300s 超时）
/// → 检查返回码与 `error` 关键字 → 无论成败删除临时文件。
///
/// **权限说明**：`diskpart.exe` 要求提升权限运行，普通用户下 `CreateProcess`
/// 直接以错误 740（ERROR_ELEVATION_REQUIRED）失败 —— 磁盘写操作（分区/格式化）
/// 无法在非管理员进程内完成，本函数只负责给出明确诊断日志。
pub fn run_diskpart(commands: &[String], timeout_secs: u64) -> DiskpartResult {
    if !is_admin() {
        log::warn!(
            "当前进程没有管理员权限：diskpart 需要提升权限才能执行磁盘写操作，\
             本次操作预计会被系统拒绝（错误 740）"
        );
    }
    let tmp = match TempScript::create(commands) {
        Ok(t) => t,
        Err(e) => {
            log::error!("diskpart execution failed: {e}");
            return (false, e);
        }
    };

    let mut child = match Command::new("diskpart")
        .arg("/s")
        .arg(&tmp.path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) {
                // 普通用户进程调用 diskpart.exe 必然在此失败（740）——
                // 给出中文、可读、可操作的提示，而不是裸的 os error。
                log::error!("{ELEVATION_REQUIRED_HINT}");
                return (false, elevation_required_message(&e.to_string()));
            }
            log::error!("diskpart execution failed: {e}");
            return (false, e.to_string());
        }
    };

    // 后台线程读 stdout/stderr，避免管道写满导致子进程阻塞
    let mut stdout_pipe = child.stdout.take();
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stdout_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let mut stderr_pipe = child.stderr.take();
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    // 带超时的等待（对应 Python subprocess.run(timeout=...)）
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = t_out.join();
                    let _ = t_err.join();
                    log::error!("diskpart timed out");
                    return (false, "操作超时".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = t_out.join();
                let _ = t_err.join();
                log::error!("diskpart execution failed: {e}");
                return (false, e.to_string());
            }
        }
    };

    let raw_out = t_out.join().unwrap_or_default();
    let raw_err = t_err.join().unwrap_or_default();
    let output = decode_acp(&raw_out);
    log::debug!("diskpart output:\n{output}");

    let rc = status.code().unwrap_or(-1);
    if rc != 0 {
        let err = decode_acp(&raw_err);
        log::error!("diskpart error (rc={rc}): {err}");
        return (false, format!("{output}{err}"));
    }

    if output_has_error(&output) {
        if output_needs_elevation(&output) {
            // diskpart 已启动但报告权限不足：同样给出中文提示
            log::error!("{ELEVATION_REQUIRED_HINT}");
            return (false, elevation_required_message(&output));
        }
        log::error!("diskpart reported error:\n{output}");
        return (false, output);
    }

    (true, output)
}

/// 格式化指定分区脚本（← `format_partition`）
pub fn format_partition_script(
    disk_number: u32,
    partition_number: u32,
    fs_type: &str,
    label: &str,
) -> Vec<String> {
    let mut cmd = format!("format fs={fs_type} quick");
    if !label.is_empty() {
        cmd.push_str(&format!(" label=\"{label}\""));
    }
    vec![
        format!("select disk {disk_number}"),
        format!("select partition {partition_number}"),
        cmd,
    ]
}

/// 格式化指定分区（普通模式，← `format_partition`）
pub fn format_partition(
    disk_number: u32,
    partition_number: u32,
    fs_type: &str,
    label: &str,
) -> DiskpartResult {
    log::info!(
        "格式化: 磁盘 {disk_number}, 分区 {partition_number}, 文件系统={fs_type}, 卷标={}",
        if label.is_empty() { "(无)" } else { label }
    );
    let ok = run_diskpart(
        &format_partition_script(disk_number, partition_number, fs_type, label),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!("格式化完成: 磁盘 {disk_number} 分区 {partition_number}");
    } else {
        log::error!("格式化失败: 磁盘 {disk_number} 分区 {partition_number}");
    }
    ok
}

/// 格式化 EFI 系统分区脚本（← `format_efi_partition`，强制 FAT32）
pub fn format_efi_partition_script(disk_number: u32, partition_number: u32) -> Vec<String> {
    vec![
        format!("select disk {disk_number}"),
        format!("select partition {partition_number}"),
        "format fs=fat32 quick".to_string(),
    ]
}

/// 格式化 EFI 系统分区（ESP 必须 FAT32，← `format_efi_partition`）
pub fn format_efi_partition(disk_number: u32, partition_number: u32) -> DiskpartResult {
    log::info!("格式化EFI分区: 磁盘 {disk_number}, 分区 {partition_number}");
    let ok = run_diskpart(
        &format_efi_partition_script(disk_number, partition_number),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!("EFI分区格式化完成: 磁盘 {disk_number} 分区 {partition_number}");
    } else {
        log::error!("EFI分区格式化失败: 磁盘 {disk_number} 分区 {partition_number}");
    }
    ok
}

/// 删除分区脚本（← `delete_partition`，override 允许删除受保护分区）
pub fn delete_partition_script(disk_number: u32, partition_number: u32) -> Vec<String> {
    vec![
        format!("select disk {disk_number}"),
        format!("select partition {partition_number}"),
        "delete partition override".to_string(),
    ]
}

/// 删除指定分区（← `delete_partition`）
pub fn delete_partition(disk_number: u32, partition_number: u32) -> DiskpartResult {
    log::info!("删除分区: 磁盘 {disk_number}, 分区 {partition_number}");
    let ok = run_diskpart(
        &delete_partition_script(disk_number, partition_number),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!("分区已删除: 磁盘 {disk_number} 分区 {partition_number}");
    } else {
        log::error!("删除分区失败: 磁盘 {disk_number} 分区 {partition_number}");
    }
    ok
}

/// 新建分区脚本（← `create_partition`）
///
/// - `size_mb = None`（或 0）→ 不带 size 参数，使用全部剩余空间
/// - `fs_type` 为空 → 不执行 format
/// - `assign_letter=false` → 不分配盘符
pub fn create_partition_script(
    disk_number: u32,
    size_mb: Option<i64>,
    fs_type: &str,
    assign_letter: bool,
) -> Vec<String> {
    let mut cmds = vec![format!("select disk {disk_number}")];
    // Python `if size_mb:` —— None 与 0 均为假
    if matches!(size_mb, Some(v) if v != 0) {
        cmds.push(format!(
            "create partition primary size={}",
            size_mb.unwrap_or_default()
        ));
    } else {
        cmds.push("create partition primary".to_string());
    }
    if !fs_type.is_empty() {
        cmds.push(format!("format fs={fs_type} quick"));
    }
    if assign_letter {
        cmds.push("assign".to_string());
    }
    cmds
}

/// 在指定磁盘上新建分区（← `create_partition`）
pub fn create_partition(
    disk_number: u32,
    size_mb: Option<i64>,
    fs_type: &str,
    assign_letter: bool,
) -> DiskpartResult {
    let size_desc = match size_mb {
        Some(v) if v != 0 => format!("{v}MB"),
        _ => "全部可用空间".to_string(),
    };
    log::info!(
        "新建分区: 磁盘 {disk_number}, 大小={size_desc}, 文件系统={fs_type}, 分配盘符={}",
        if assign_letter { "是" } else { "否" }
    );
    let ok = run_diskpart(
        &create_partition_script(disk_number, size_mb, fs_type, assign_letter),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!("新建分区完成: 磁盘 {disk_number}");
    } else {
        log::error!("新建分区失败: 磁盘 {disk_number}");
    }
    ok
}

/// 分配盘符脚本（← `assign_drive_letter`）
pub fn assign_drive_letter_script(
    disk_number: u32,
    partition_number: u32,
    letter: Option<char>,
) -> Vec<String> {
    vec![
        format!("select disk {disk_number}"),
        format!("select partition {partition_number}"),
        match letter {
            Some(c) => format!("assign letter={c}"),
            None => "assign".to_string(),
        },
    ]
}

/// 为指定分区分配盘符（← `assign_drive_letter`）
pub fn assign_drive_letter(
    disk_number: u32,
    partition_number: u32,
    letter: Option<char>,
) -> DiskpartResult {
    log::info!(
        "分配盘符: 磁盘 {disk_number}, 分区 {partition_number}, 指定盘符={}",
        match letter {
            Some(c) => c.to_string(),
            None => "(自动)".to_string(),
        }
    );
    let ok = run_diskpart(
        &assign_drive_letter_script(disk_number, partition_number, letter),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!("盘符分配成功: 磁盘 {disk_number} 分区 {partition_number}");
    } else {
        log::error!("盘符分配失败: 磁盘 {disk_number} 分区 {partition_number}");
    }
    ok
}

/// 当前已占用的盘符集合（← `_get_used_letters`，对应 `os.path.exists("X:\\")`）
pub fn get_used_letters() -> HashSet<char> {
    let mut used = HashSet::new();
    for c in 'A'..='Z' {
        if std::path::Path::new(&format!("{c}:\\")).exists() {
            used.insert(c);
        }
    }
    used
}

/// 从已占用集合中找可用盘符（纯函数部分）
///
/// ← `_find_available_drive_letter`：**Z 向 A** 倒序，跳过已占用与 **A、B、C、D**。
fn available_letter_from(used: &HashSet<char>) -> Option<char> {
    for c in ('A'..='Z').rev() {
        if !used.contains(&c) && !matches!(c, 'A' | 'B' | 'C' | 'D') {
            return Some(c);
        }
    }
    None
}

/// 查找可用驱动器号（← `_find_available_drive_letter`）
pub fn find_available_drive_letter() -> Option<char> {
    available_letter_from(&get_used_letters())
}

/// 一键分区脚本构造（纯函数，dry-run 可用；← `one_click_partition` 的命令生成部分）
///
/// 流程对照 Python：
/// 1. `select disk` → `clean` → `convert {style}`
/// 2. GPT 且勾选 ESP：`create partition efi size={n}` → FAT32 quick label=`ESP`
///    → 盘符：fixed 时 **Z 优先**（占用则从反向字母中选，跳过 A/B，仍找不到则裸 assign）
/// 3. GPT：`create partition msr size=16`（不分配）
/// 4. 数据分区循环（MBR 第一个数据分区 `active`），盘符从 **A 向 Z**（跳过 A/B、已占用）
///    data_parts 为空 → 单个占满的 NTFS 分区
///
/// `used` 由调用方传入（执行时 = `get_used_letters()`，测试可注入），
/// 会被就地更新以避免多分区盘符冲突。
pub fn one_click_script(
    disk_number: u32,
    style: &str,
    esp_enabled: bool,
    esp_size_mb: i64,
    data_parts: Option<&[DataPartSpec]>,
    fixed_letters: bool,
    used: &mut HashSet<char>,
) -> (Vec<String>, AssignedLetters) {
    let mut commands = vec![
        format!("select disk {disk_number}"),
        "clean".to_string(),
        format!("convert {style}"),
    ];
    let mut assigned = AssignedLetters::default();

    // 第二步：ESP（仅 GPT）
    if esp_enabled && style == "gpt" {
        commands.push(format!("create partition efi size={esp_size_mb}"));
        commands.push("format fs=fat32 quick label=\"ESP\"".to_string());
        if fixed_letters {
            // ESP 优先 Z:，被占用则从可用盘符中选（倒序，避开 A/B）
            let esp_letter = if !used.contains(&'Z') {
                Some('Z')
            } else {
                ('A'..='Z')
                    .rev()
                    .find(|&c| !used.contains(&c) && !matches!(c, 'A' | 'B'))
            };
            match esp_letter {
                Some(c) => {
                    commands.push(format!("assign letter={c}"));
                    used.insert(c);
                    assigned.esp = Some(c);
                }
                None => commands.push("assign".to_string()),
            }
        } else {
            commands.push("assign".to_string());
        }
    }

    // 第三步：GPT 自动创建 MSR 保留分区（不分配盘符）
    if style == "gpt" {
        commands.push("create partition msr size=16".to_string());
    }

    // 第四步：数据分区
    if let Some(parts) = data_parts.filter(|p| !p.is_empty()) {
        for (part_idx, part) in parts.iter().enumerate() {
            if matches!(part.size_mb, Some(v) if v != 0) {
                commands.push(format!(
                    "create partition primary size={}",
                    part.size_mb.unwrap_or_default()
                ));
            } else {
                commands.push("create partition primary".to_string());
            }
            if !part.fs.is_empty() {
                commands.push(format!("format fs={} quick", part.fs));
            }
            // MBR 模式：第一个数据分区标记为 active（引导分区）
            if style == "mbr" && part_idx == 0 {
                commands.push("active".to_string());
            }
            if fixed_letters {
                // 从 A 向 Z 顺序找可用盘符（跳过 A/B、已占用）
                let letter =
                    ('A'..='Z').find(|&c| !used.contains(&c) && !matches!(c, 'A' | 'B'));
                match letter {
                    Some(c) => {
                        commands.push(format!("assign letter={c}"));
                        used.insert(c);
                        assigned.data.push(c);
                    }
                    None => commands.push("assign".to_string()),
                }
            } else {
                commands.push("assign".to_string());
            }
        }
    } else {
        commands.push("create partition primary".to_string());
        commands.push("format fs=ntfs quick".to_string());
        if style == "mbr" {
            commands.push("active".to_string());
        }
        if fixed_letters {
            let letter = ('A'..='Z').find(|&c| !used.contains(&c) && !matches!(c, 'A' | 'B'));
            match letter {
                Some(c) => {
                    commands.push(format!("assign letter={c}"));
                    assigned.data.push(c);
                }
                None => commands.push("assign".to_string()),
            }
        } else {
            commands.push("assign".to_string());
        }
    }

    (commands, assigned)
}

/// 一键分区（← `one_click_partition`）：清除磁盘并按指定方案重新分区
///
/// `assigned_letters` 会写入实际分配结果（对应 Python 出参字典）。
pub fn one_click_partition(
    disk_number: u32,
    style: &str,
    esp_enabled: bool,
    esp_size_mb: i64,
    data_parts: Option<&[DataPartSpec]>,
    fixed_letters: bool,
    assigned_letters: &mut AssignedLetters,
) -> DiskpartResult {
    let parts_desc = match data_parts {
        None => "None".to_string(),
        Some([]) => "[]".to_string(),
        Some(parts) => format!(
            "[{}]",
            parts
                .iter()
                .map(DataPartSpec::py_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    log::warn!(
        "一键分区: 磁盘 {disk_number}, 分区表={style}, ESP={}, 数据分区方案={parts_desc}, 动态盘符={fixed_letters}",
        if esp_enabled {
            format!("启用({esp_size_mb}MB)")
        } else {
            "禁用".to_string()
        }
    );

    let mut used = get_used_letters();
    let (commands, assigned) = one_click_script(
        disk_number,
        style,
        esp_enabled,
        esp_size_mb,
        data_parts,
        fixed_letters,
        &mut used,
    );
    *assigned_letters = assigned;

    let ok = run_diskpart(&commands, DEFAULT_TIMEOUT);
    if ok.0 {
        log::info!(
            "一键分区完成: 磁盘 {disk_number}, 分配盘符={assigned_letters:?}"
        );
    } else {
        log::error!("一键分区失败: 磁盘 {disk_number}");
    }
    ok
}

/// 初始化磁盘脚本（← `initialize_disk`：clean → convert gpt|mbr）
pub fn initialize_disk_script(disk_number: u32, style: &str) -> Vec<String> {
    vec![
        format!("select disk {disk_number}"),
        "clean".to_string(),
        format!("convert {style}"),
    ]
}

/// 初始化磁盘（清除所有分区并设置分区表类型，← `initialize_disk`）
pub fn initialize_disk(disk_number: u32, style: &str) -> DiskpartResult {
    log::warn!(
        "初始化磁盘: 磁盘 {disk_number}, 分区表={} (此操作将清除磁盘上所有数据!)",
        style.to_uppercase()
    );
    let ok = run_diskpart(
        &initialize_disk_script(disk_number, style),
        DEFAULT_TIMEOUT,
    );
    if ok.0 {
        log::info!(
            "磁盘初始化完成: 磁盘 {disk_number} ({})",
            style.to_uppercase()
        );
    } else {
        log::error!("磁盘初始化失败: 磁盘 {disk_number}");
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oneclick(
        disk: u32,
        style: &str,
        esp: bool,
        esp_mb: i64,
        parts: Option<&[DataPartSpec]>,
        fixed: bool,
        used: &[char],
    ) -> (Vec<String>, AssignedLetters) {
        let mut set: HashSet<char> = used.iter().copied().collect();
        one_click_script(disk, style, esp, esp_mb, parts, fixed, &mut set)
    }

    /// ← Python format_partition：含/不含卷标
    #[test]
    fn format_partition_scripts() {
        assert_eq!(
            format_partition_script(1, 2, "ntfs", ""),
            ["select disk 1", "select partition 2", "format fs=ntfs quick"]
        );
        assert_eq!(
            format_partition_script(0, 3, "fat32", "WIN"),
            [
                "select disk 0",
                "select partition 3",
                "format fs=fat32 quick label=\"WIN\""
            ]
        );
    }

    /// ← Python format_efi_partition：强制 FAT32 quick
    #[test]
    fn format_efi_script() {
        assert_eq!(
            format_efi_partition_script(2, 1),
            ["select disk 2", "select partition 1", "format fs=fat32 quick"]
        );
    }

    /// ← Python delete_partition：override
    #[test]
    fn delete_partition_script_golden() {
        assert_eq!(
            delete_partition_script(0, 4),
            [
                "select disk 0",
                "select partition 4",
                "delete partition override"
            ]
        );
    }

    /// ← Python create_partition：size 有/无、fs 空、assign 开关、size=0 等价 None
    #[test]
    fn create_partition_scripts() {
        assert_eq!(
            create_partition_script(1, Some(100_000), "ntfs", true),
            [
                "select disk 1",
                "create partition primary size=100000",
                "format fs=ntfs quick",
                "assign"
            ]
        );
        assert_eq!(
            create_partition_script(1, None, "ntfs", true),
            [
                "select disk 1",
                "create partition primary",
                "format fs=ntfs quick",
                "assign"
            ]
        );
        // Python `if size_mb:` —— 0 视同未指定；`if fs_type:` —— 空则跳过格式化
        assert_eq!(
            create_partition_script(1, Some(0), "", false),
            ["select disk 1", "create partition primary"]
        );
    }

    /// ← Python assign_drive_letter
    #[test]
    fn assign_letter_scripts() {
        assert_eq!(
            assign_drive_letter_script(0, 1, Some('Z')),
            ["select disk 0", "select partition 1", "assign letter=Z"]
        );
        assert_eq!(
            assign_drive_letter_script(0, 1, None),
            ["select disk 0", "select partition 1", "assign"]
        );
    }

    /// ← Python initialize_disk
    #[test]
    fn initialize_scripts() {
        assert_eq!(
            initialize_disk_script(3, "gpt"),
            ["select disk 3", "clean", "convert gpt"]
        );
        assert_eq!(
            initialize_disk_script(3, "mbr"),
            ["select disk 3", "clean", "convert mbr"]
        );
    }

    /// ← Python `_find_available_drive_letter`：倒序 Z→A，跳过已占用与 A/B/C/D
    #[test]
    fn available_letter_rules() {
        assert_eq!(available_letter_from(&HashSet::new()), Some('Z'));
        assert_eq!(available_letter_from(&HashSet::from(['Z', 'C'])), Some('Y'));
        // D 显式跳过 → W（Z/Y/X 已占）
        assert_eq!(
            available_letter_from(&HashSet::from(['C', 'Z', 'Y', 'X'])),
            Some('W')
        );
        let full: HashSet<char> = "EFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
        assert_eq!(available_letter_from(&full), None);
    }

    /// 真实机器上 C 盘必然存在
    #[test]
    fn used_letters_contains_c() {
        assert!(get_used_letters().contains(&'C'));
    }

    /// ← Python one_click_partition 黄金脚本（GPT + ESP + 两数据分区 + 动态盘符）
    /// 已占用 {C,D,E} → ESP=Z、数据=F、G（A/B 恒跳过，C/D/E 被占）
    #[test]
    fn oneclick_gpt_esp_two_parts_golden() {
        let parts = [
            DataPartSpec::new(Some(100_000), "ntfs"),
            DataPartSpec::new(None, "ntfs"),
        ];
        let (cmds, assigned) = oneclick(
            0,
            "gpt",
            true,
            300,
            Some(&parts),
            true,
            &['C', 'D', 'E'],
        );
        assert_eq!(
            cmds,
            [
                "select disk 0",
                "clean",
                "convert gpt",
                "create partition efi size=300",
                "format fs=fat32 quick label=\"ESP\"",
                "assign letter=Z",
                "create partition msr size=16",
                "create partition primary size=100000",
                "format fs=ntfs quick",
                "assign letter=F",
                "create partition primary",
                "format fs=ntfs quick",
                "assign letter=G",
            ]
        );
        assert_eq!(
            assigned,
            AssignedLetters {
                esp: Some('Z'),
                data: vec!['F', 'G']
            }
        );
    }

    /// Z 被占用 → ESP 取反向第一个空闲（跳过 A/B）
    #[test]
    fn oneclick_esp_falls_back_when_z_taken() {
        let (cmds, assigned) = oneclick(0, "gpt", true, 300, None, true, &['Z', 'C']);
        assert!(cmds.contains(&"assign letter=Y".to_string()));
        assert_eq!(assigned.esp, Some('Y'));
        // 无 data_parts → 单分区占满，下一个空闲字母（C 被占 → D）
        assert_eq!(assigned.data, vec!['D']);
    }

    /// MBR + 无 ESP + 无 data_parts：无 MSR、第一个分区 active
    #[test]
    fn oneclick_mbr_golden() {
        let (cmds, assigned) = oneclick(1, "mbr", false, 300, None, true, &[]);
        assert_eq!(
            cmds,
            [
                "select disk 1",
                "clean",
                "convert mbr",
                "create partition primary",
                "format fs=ntfs quick",
                "active",
                "assign letter=C",
            ]
        );
        assert_eq!(assigned.data, vec!['C']);
    }

    /// MBR 多分区：仅第一个 active；数据盘符从 C 起
    #[test]
    fn oneclick_mbr_two_parts_active_only_first() {
        let parts = [
            DataPartSpec::new(Some(50_000), "ntfs"),
            DataPartSpec::new(None, "ntfs"),
        ];
        let (cmds, assigned) = oneclick(1, "mbr", false, 300, Some(&parts), true, &['C']);
        assert_eq!(
            cmds,
            [
                "select disk 1",
                "clean",
                "convert mbr",
                "create partition primary size=50000",
                "format fs=ntfs quick",
                "active",
                "assign letter=D",
                "create partition primary",
                "format fs=ntfs quick",
                "assign letter=E",
            ]
        );
        assert_eq!(assigned.data, vec!['D', 'E']);
    }

    /// fixed_letters=false → 全部裸 assign，assigned 为空
    #[test]
    fn oneclick_auto_letters() {
        let parts = [DataPartSpec::new(Some(1000), "fat32")];
        let (cmds, assigned) = oneclick(0, "gpt", true, 260, Some(&parts), false, &[]);
        assert_eq!(
            cmds,
            [
                "select disk 0",
                "clean",
                "convert gpt",
                "create partition efi size=260",
                "format fs=fat32 quick label=\"ESP\"",
                "assign",
                "create partition msr size=16",
                "create partition primary size=1000",
                "format fs=fat32 quick",
                "assign",
            ]
        );
        assert_eq!(assigned, AssignedLetters::default());
    }

    /// error 关键字大小写不敏感（Python `in output.lower()`）
    #[test]
    fn error_keyword_detection() {
        assert!(output_has_error("VIRTUAL DISK SERVICE ERROR: ..."));
        assert!(output_has_error("Error: 参数不正确。"));
        assert!(!output_has_error("成功: 磁盘 0 现在是选定磁盘。"));
    }

    /// 权限不足特征串识别（仅用于日志诊断，不影响返回值）
    #[test]
    fn elevation_error_detection() {
        assert!(output_needs_elevation(
            "The requested operation requires elevation."
        ));
        assert!(output_needs_elevation("请求的操作需要提升。"));
        assert!(!output_needs_elevation("成功: 磁盘 0 现在是选定磁盘。"));
    }

    /// 错误 740 的提示必须是**中文、可读、可操作**，且与系统语言无关
    #[test]
    fn elevation_message_is_chinese_and_actionable() {
        // 中文系统：系统原始文本为"请求的操作需要提升。(os error 740)"
        let zh = elevation_required_message("请求的操作需要提升。 (os error 740)");
        assert!(zh.starts_with("磁盘分区/格式化需要管理员权限"));
        assert!(zh.contains("以管理员身份运行本程序"));
        assert!(zh.contains("740"));
        assert!(zh.contains("请求的操作需要提升"), "应保留系统原始提示: {zh}");

        // 英文系统：同样以中文说明开头，不得只显示英文
        let en = elevation_required_message(
            "The requested operation requires elevation. (os error 740)",
        );
        assert!(en.starts_with("磁盘分区/格式化需要管理员权限"));
        assert!(en.contains("The requested operation requires elevation"), "{en}");

        // 系统文本为空 → 只给中文说明，不产生多余空行
        assert_eq!(elevation_required_message("   "), ELEVATION_REQUIRED_HINT);
        assert_eq!(elevation_required_message(""), ELEVATION_REQUIRED_HINT);
    }

    /// 提示文本不得为空、不得含乱码替换符（ACP 解码相关）
    #[test]
    fn elevation_message_is_not_empty() {
        let msg = elevation_required_message("os error 740");
        assert!(!msg.trim().is_empty());
        assert!(!msg.contains('\u{FFFD}'));
    }

    /// 错误码 740 常量与 Windows ERROR_ELEVATION_REQUIRED 一致
    #[test]
    fn elevation_required_constant() {
        assert_eq!(ERROR_ELEVATION_REQUIRED, 740);
    }

    /// Python `'\n'.join(commands)`：无结尾换行、行序保持
    #[test]
    fn script_join_matches_python() {
        let cmds = initialize_disk_script(0, "gpt");
        assert_eq!(cmds.join("\n"), "select disk 0\nclean\nconvert gpt");
    }

    /// DataPartSpec 日志用 dict 字面量风格
    #[test]
    fn data_part_py_repr() {
        assert_eq!(
            DataPartSpec::new(Some(100_000), "ntfs").py_repr(),
            "{'size_mb': 100000, 'fs': 'ntfs'}"
        );
        assert_eq!(
            DataPartSpec::new(None, "ntfs").py_repr(),
            "{'size_mb': None, 'fs': 'ntfs'}"
        );
    }

    /// 冒烟测试（只读，无破坏性）：真实执行 `diskpart /s` 的 `list disk`，
    /// 验证临时脚本/CREATE_NO_WINDOW/管道读取/ACP 解码/错误检测整条管线。
    /// 非管理员环境 diskpart 会拒绝执行 —— 关键断言是不 panic 且解码无 U+FFFD。
    /// 手动运行：cargo test -- --ignored --nocapture
    #[test]
    #[ignore]
    fn run_diskpart_smoke_list_disk() {
        let (ok, out) = run_diskpart(&["list disk".to_string()], 30);
        println!("ok={ok}\n{out}");
        assert!(!out.contains('\u{FFFD}'), "输出经 ACP 解码不应出现替换符");
        // 临时脚本必须已删除
        assert!(!std::path::Path::new(&format!(
            "{}\\sks_diskpart_{}_0.txt",
            std::env::temp_dir().display(),
            std::process::id()
        ))
        .exists());
    }
}
