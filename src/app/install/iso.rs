//! ISO 挂载、盘符检测与卸载（← `sys_install.py::_get_drive_letters` /
//! `_enum_volume_drive_letters` / `_find_new_drive_letter` / `mount_iso` /
//! `_close_explorer_window` / `unmount_iso` / `find_wim_in_iso`，文档 §6.3）

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows_sys::Win32::Foundation::{BOOL, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::Storage::FileSystem::{
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetLogicalDrives,
    GetVolumePathNamesForVolumeNameW,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, IsWindowVisible, PostMessageW, WNDENUMPROC,
    WM_CLOSE,
};

use crate::utils::CREATE_NO_WINDOW;

/// 卷名缓冲长度（Python `create_unicode_buffer(260)`）
const VOLUME_NAME_LEN: usize = 260;
/// 卷挂载点缓冲长度（Python `buf_size = 1024`，不足时不重试——与 Python 一致）
const PATH_NAMES_LEN: usize = 1024;

/// 卷句柄 RAII（← Python `finally: FindVolumeClose(handle)`）
struct VolumeHandle(HANDLE);

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        unsafe { FindVolumeClose(self.0) };
    }
}

/// ISO 挂载映射：挂载路径 → ISO 文件路径（← `_iso_mount_map`）。
/// Python 存 `(iso_path, None)`，第二项从未被读取，Rust 只存路径。
fn mount_map() -> &'static Mutex<HashMap<String, String>> {
    static MAP: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_map() -> std::sync::MutexGuard<'static, HashMap<String, String>> {
    mount_map().lock().unwrap_or_else(|e| e.into_inner())
}

/// Python `sorted(char_set)` → `['C', 'D']` 列表 repr（仅日志用）
fn py_list_repr(items: &[char]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let body: Vec<String> = items.iter().map(|c| format!("'{c}'")).collect();
    format!("[{}]", body.join(", "))
}

/// Python `sorted(char_set)` → 集合 repr `{'C', 'D'}`（仅日志用）
fn py_set_repr(set: &HashSet<char>) -> String {
    let mut items: Vec<char> = set.iter().copied().collect();
    items.sort_unstable();
    if items.is_empty() {
        return "{}".to_string();
    }
    let body: Vec<String> = items.iter().map(|c| format!("'{c}'")).collect();
    format!("{{{}}}", body.join(", "))
}

/// 多字符串中提取盘符（← `len(chunk)>=2 and chunk[1]==':'` → `chunk[0].upper()`；
/// Python 不校验首字符是否为字母）
fn drive_from_chunk(chunk: &str) -> Option<char> {
    let s = chunk.trim();
    let mut it = s.chars();
    let first = it.next()?;
    let second = it.next()?;
    if second != ':' {
        return None;
    }
    // Python str.upper() 可能产出多字符（如 ß），盘符场景取首字符即可
    Some(first.to_uppercase().next().unwrap_or(first))
}

/// 获取当前盘符集合（← `_get_drive_letters`）
///
/// 方法1 `GetLogicalDrives` 位掩码（API 层，Rust 侧失败表现为返回 0）；
/// 方法2 `X:\` 路径存在性检查（文件系统层，解决 PE 下 API 缓存不更新）。
pub fn get_drive_letters() -> HashSet<char> {
    let mut drives = HashSet::new();

    // 方法1: GetLogicalDrives 位掩码
    let mask = unsafe { GetLogicalDrives() };
    for (i, letter) in ('A'..='Z').enumerate() {
        if mask & (1 << i) != 0 {
            drives.insert(letter);
        }
    }

    // 方法2: 路径存在性检查（始终执行）
    for letter in 'A'..='Z' {
        let probe = format!("{letter}:\\");
        if Path::new(&probe).exists() {
            drives.insert(letter);
        }
    }

    drives
}

/// 通过枚举所有卷设备获取盘符集合（← `_enum_volume_drive_letters`）
///
/// `FindFirstVolumeW` / `FindNextVolumeW` / `GetVolumePathNamesForVolumeNameW`
/// 能列出 GetLogicalDrives 漏报的盘符（如刚挂载但未登记到逻辑驱动器列表的卷）。
pub fn enum_volume_drive_letters() -> HashSet<char> {
    let mut drives = HashSet::new();

    let mut volume_name = vec![0u16; VOLUME_NAME_LEN];
    let handle = unsafe { FindFirstVolumeW(volume_name.as_mut_ptr(), VOLUME_NAME_LEN as u32) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        log::debug!("FindFirstVolumeW 返回无效句柄");
        return drives;
    }
    let _guard = VolumeHandle(handle);

    loop {
        // 卷名（NUL 结尾）
        let vol: Vec<u16> = volume_name
            .iter()
            .copied()
            .take_while(|&c| c != 0)
            .collect();
        if vol.is_empty() {
            break;
        }

        // 该卷的所有挂载点（多字符串 "C:\\\0D:\\\0\0"）
        let mut buf = vec![0u16; PATH_NAMES_LEN];
        let mut returned: u32 = 0;
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                vol.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut returned,
            )
        };
        if ok != 0 && returned > 0 {
            let full: Vec<u16> = buf.iter().copied().take(returned as usize).collect();
            for chunk in full.split(|&c| c == 0) {
                if chunk.is_empty() {
                    continue;
                }
                let text = String::from_utf16_lossy(chunk);
                if let Some(d) = drive_from_chunk(&text) {
                    drives.insert(d);
                }
            }
        }

        // 下一个卷
        let next = unsafe { FindNextVolumeW(handle, volume_name.as_mut_ptr(), VOLUME_NAME_LEN as u32) };
        if next == 0 {
            break;
        }
    }

    drives
}

/// 查找新挂载的驱动器盘符（← `_find_new_drive_letter`）
///
/// 策略1：每 200ms 轮询 `get_drive_letters()` 对比，持续 `timeout_sec` 秒；
/// 策略2：超时后 `enum_volume_drive_letters()` 枚举卷设备补充。
/// 取新集合中字典序最大的盘符；未找到返回空串。
pub fn find_new_drive_letter(drives_before: &HashSet<char>, timeout_sec: u64) -> String {
    // 策略1: 轮询（每 200ms 一次）
    let poll_count = timeout_sec * 5;
    for _ in 0..poll_count {
        std::thread::sleep(Duration::from_millis(200));
        let drives_after = get_drive_letters();
        let new_drives: HashSet<char> = drives_after.difference(drives_before).copied().collect();
        if !new_drives.is_empty() {
            let chosen = new_drives.iter().copied().max().unwrap_or_default();
            log::debug!("轮询发现新盘符: {chosen} (新集合={})", py_set_repr(&new_drives));
            return chosen.to_string();
        }
    }

    log::debug!("轮询未发现新盘符，尝试枚举卷设备...");

    // 策略2: 枚举卷的挂载点（补充）
    let vol_drives = enum_volume_drive_letters();
    let new_drives: HashSet<char> = vol_drives.difference(drives_before).copied().collect();
    if !new_drives.is_empty() {
        let chosen = new_drives.iter().copied().max().unwrap_or_default();
        log::debug!("枚举卷发现新盘符: {chosen} (新集合={})", py_set_repr(&new_drives));
        return chosen.to_string();
    }

    String::new()
}

/// 挂载 ISO 镜像（← `mount_iso`）
///
/// 使用 explorer.exe 打开 ISO（模拟双击挂载，WinNTSetup 推荐工作流：
/// 由系统/PE 的 explorer 负责挂载，本工具只检测盘符并应用镜像）。
/// Python 的 `progress_callback` 参数为从未使用的死参数，Rust 省略。
///
/// 返回 `(成功标志, 挂载路径, 错误信息)`。
pub fn mount_iso(iso_path: &str) -> (bool, String, String) {
    log::info!("挂载 ISO: {iso_path}");

    // ← `_safe_path_exists`：Rust 侧 Path::exists 不抛异常
    if !Path::new(iso_path).exists() {
        let error_msg = format!("找不到ISO文件: {iso_path}");
        log::error!("{error_msg}");
        return (false, String::new(), error_msg);
    }

    let drives_before = get_drive_letters();
    let mut before_sorted: Vec<char> = drives_before.iter().copied().collect();
    before_sorted.sort_unstable();
    log::debug!("挂载前盘符: {}", py_list_repr(&before_sorted));

    log::info!("使用 explorer.exe 打开 ISO...");
    if let Err(e) = Command::new("explorer.exe")
        .arg(iso_path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        let error_msg = format!("启动 explorer.exe 失败: {e}");
        log::error!("{error_msg}");
        return (false, String::new(), error_msg);
    }
    // Python 同样不等待 explorer 退出（Popen 句柄即弃）

    // 等待盘符出现
    let drive_letter = find_new_drive_letter(&drives_before, 30);
    if drive_letter.is_empty() {
        let error_msg = "explorer 打开后未找到新盘符".to_string();
        log::error!("{error_msg}");
        return (false, String::new(), error_msg);
    }

    let mount_path = format!("{drive_letter}:\\");
    log::info!("ISO 已挂载到: {mount_path}");

    close_explorer_window(&drive_letter);
    lock_map().insert(mount_path.clone(), iso_path.to_string());
    (true, mount_path, String::new())
}

/// EnumWindows 回调上下文（目标标题片段 + 命中窗口）
struct EnumCtx {
    targets: Vec<String>,
    found: Vec<HWND>,
}

/// 回调：可见 + CabinetWClass/ExploreWClass + 标题含盘符片段 → 收集
unsafe extern "system" fn enum_close_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam as *mut EnumCtx) };
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return 1; // Python: return True
        }
        let mut class_buf = [0u16; 256];
        let n = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
        if n <= 0 {
            return 1;
        }
        let class_name = String::from_utf16_lossy(&class_buf[..n as usize]);
        if class_name == "CabinetWClass" || class_name == "ExploreWClass" {
            let mut text_buf = [0u16; 512];
            let t = GetWindowTextW(hwnd, text_buf.as_mut_ptr(), 512);
            if t > 0 {
                let text = String::from_utf16_lossy(&text_buf[..t as usize]);
                if ctx.targets.iter().any(|target| text.contains(target.as_str())) {
                    ctx.found.push(hwnd);
                }
            }
        }
        1
    }
}

/// 尝试关闭指定盘符的资源管理器窗口（← `_close_explorer_window`）
fn close_explorer_window(drive_letter: &str) {
    std::thread::sleep(Duration::from_millis(300));

    let upper = drive_letter.to_uppercase();
    let lower = drive_letter.to_lowercase();
    let mut ctx = EnumCtx {
        targets: vec![
            format!("{upper}:"),
            format!("{lower}:"),
            format!("({upper}:)"),
            format!("({lower}:)"),
        ],
        found: Vec::new(),
    };

    let enum_proc: WNDENUMPROC = Some(enum_close_cb);
    unsafe { EnumWindows(enum_proc, &mut ctx as *mut EnumCtx as isize) };

    for &hwnd in &ctx.found {
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
    if !ctx.found.is_empty() {
        log::info!("已关闭 {} 个资源管理器窗口", ctx.found.len());
    }
}

/// UTF-16 + NUL（ShellExecuteW 参数）
fn wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 卸载 ISO 镜像（← `unmount_iso`）
///
/// ShellExecuteW "eject" 动词（explorer 右键"弹出"的底层实现），
/// 兼容 WinPE（无需 PowerShell）。返回 `(成功标志, 错误信息)`。
pub fn unmount_iso(iso_mount_path: &str) -> (bool, String) {
    log::info!("卸载 ISO: {iso_mount_path}");

    let drive_letter = iso_mount_path
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    lock_map().remove(iso_mount_path);

    let operation = wide_null("eject");
    let file = wide_null(&format!("{drive_letter}:"));
    let hinst = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    } as isize;
    if hinst > 32 {
        log::info!("ISO 已卸载（ShellExecute eject）");
        return (true, String::new());
    }
    log::debug!("ShellExecute eject 失败，错误码: {hinst}");
    log::warn!("ISO 卸载失败（ShellExecute eject 失败）");
    (false, "卸载失败".to_string())
}

/// WIM/ESD 搜索路径（← `find_wim_in_iso` 内 `os.path.join` 拼接）
///
/// 调用方传入的挂载路径恒为 `X:\`（mount_iso 生成）；直接拼接与
/// `os.path.join("X:\\", "sources", …)`、盘符相对 `"X:"` 两种输入结果一致。
fn wim_search_paths(mount_path: &str) -> [String; 2] {
    [
        format!("{mount_path}sources\\install.wim"),
        format!("{mount_path}sources\\install.esd"),
    ]
}

/// 在挂载的 ISO 中查找 WIM/ESD 文件（← `find_wim_in_iso`）
pub fn find_wim_in_iso(mount_path: &str) -> Vec<String> {
    log::debug!("在 {mount_path} 中查找 WIM/ESD 文件");

    let mut files = Vec::new();
    for path in wim_search_paths(mount_path) {
        if Path::new(&path).exists() {
            log::debug!("找到: {path}");
            files.push(path);
        }
    }

    if files.is_empty() {
        log::warn!("ISO 中未找到 install.wim/install.esd: {mount_path}");
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ← Python 集合/列表 repr（日志格式对齐）
    #[test]
    fn py_reprs() {
        let mut s = HashSet::new();
        assert_eq!(py_set_repr(&s), "{}");
        assert_eq!(py_list_repr(&[]), "[]");
        s.insert('D');
        s.insert('C');
        assert_eq!(py_set_repr(&s), "{'C', 'D'}");
        assert_eq!(py_list_repr(&['C', 'D']), "['C', 'D']");
    }

    /// ← 多字符串盘符提取：trim + 第 2 字符冒号 + 首字母大写
    #[test]
    fn chunk_drive_extraction() {
        assert_eq!(drive_from_chunk("C:\\"), Some('C'));
        assert_eq!(drive_from_chunk("e:\\"), Some('E'));
        // " G " 经 trim 后长度 1，`chunk[1]` 越界 → Python 亦返回 None
        assert_eq!(drive_from_chunk(" G "), None);
        assert_eq!(drive_from_chunk(""), None);
        assert_eq!(drive_from_chunk("\\"), None);
        assert_eq!(drive_from_chunk("XY"), None);
        // Python 不校验首字符是否为字母（chunk[1]==':' 即取）
        assert_eq!(drive_from_chunk("1:\\"), Some('1'));
    }

    /// ← 新盘符选择：集合字典序最大值
    #[test]
    fn max_of_new_set() {
        let before: HashSet<char> = ['C', 'D'].into_iter().collect();
        let after: HashSet<char> = ['C', 'D', 'E', 'Z'].into_iter().collect();
        let new: HashSet<char> = after.difference(&before).copied().collect();
        assert_eq!(new.iter().copied().max(), Some('Z'));
    }

    /// ← WIM 搜索路径拼接（挂载路径恒为 `X:\`；`X:` 输入亦与 Python 一致）
    #[test]
    fn wim_paths_shape() {
        assert_eq!(
            wim_search_paths("E:\\"),
            ["E:\\sources\\install.wim", "E:\\sources\\install.esd"]
        );
        assert_eq!(
            wim_search_paths("E:"),
            ["E:sources\\install.wim", "E:sources\\install.esd"]
        );
    }

    /// ← `_iso_mount_map` 的插入/弹出语义
    #[test]
    fn mount_map_insert_pop() {
        let mut map = lock_map();
        map.insert("X:\\".to_string(), "a.iso".to_string());
        assert_eq!(map.remove("X:\\"), Some("a.iso".to_string()));
        assert_eq!(map.remove("X:\\"), None);
    }

    /// ← UTF-16 NUL 结尾参数构造
    #[test]
    fn wide_null_encoding() {
        assert_eq!(wide_null("eject"), [101, 106, 101, 99, 116, 0]);
        assert_eq!(wide_null(""), [0]);
    }
}
