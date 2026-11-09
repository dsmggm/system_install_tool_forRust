//! 分区格式化与引导修复（← `sys_install.py::format_partition_quick` / `repair_boot`，
//! 文档 §6.3）

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use crate::utils::{decode_acp, py_str_repr, CREATE_NO_WINDOW};

use super::dism::{run_command, spawn_reader, wait_with_timeout, BCDBOOT_TIMEOUT, BOOTSECT_TIMEOUT};

/// format.com 超时（秒）
const FORMAT_TIMEOUT: u64 = 120;

/// 禁用 Windows Defender（离线 SYSTEM hive）超时（秒）
const DISABLE_DEFENDER_TIMEOUT: u64 = 30;

/// 规范化盘符为 `"C:"` 形式（← Python `strip().rstrip("\\/").rstrip(":")` +
/// `letter[0].isalpha()` 校验 + `letter[0].upper()`）。
///
/// 无效（空/首字符非字母）返回 None。
fn normalize_drive_spec(partition_letter: &str) -> Option<String> {
    let trimmed = partition_letter.trim();
    let stripped = trimmed.trim_end_matches(['\\', '/']);
    let letter = stripped.trim_end_matches(':');
    let first = letter.chars().next()?;
    if !first.is_alphabetic() {
        return None;
    }
    // 盘符恒为单个 ASCII 字母（Python `letter[0].upper()`）
    Some(format!("{}:", first.to_ascii_uppercase()))
}

/// bootsect 命令形状（← `repair_boot` 第 1 步，抽出便于测试）
fn bootsect_cmd(system_partition: &str) -> Vec<String> {
    vec![
        "bootsect".to_string(),
        "/nt60".to_string(),
        system_partition.to_string(),
        "/force".to_string(),
        "/mbr".to_string(),
    ]
}

/// bcdboot 命令形状（← `repair_boot` 第 2 步，抽出便于测试）
fn bcdboot_cmd(windows_partition: &str, system_partition: &str) -> Vec<String> {
    vec![
        "bcdboot".to_string(),
        format!("{windows_partition}\\Windows"),
        "/s".to_string(),
        system_partition.to_string(),
        "/f".to_string(),
        "all".to_string(), // 同时支持 BIOS(MBR) 与 UEFI 引导
    ]
}

/// 快速格式化分区（← `format_partition_quick`）
///
/// 使用 `format.com` 而非 `format`：CreateProcess 仅自动追加 `.exe`，
/// 系统工具实际扩展名为 `.com`。stdin 重定向 DEVNULL，避免卷标确认挂起。
pub fn format_partition_quick(partition_letter: &str) -> (bool, String) {
    let Some(drive_spec) = normalize_drive_spec(partition_letter) else {
        let err = format!("无效的分区盘符: {}", py_str_repr(partition_letter));
        log::error!("{err}");
        return (false, err);
    };

    log::info!("格式化分区: {drive_spec}");
    let cmd = [
        "format.com",
        &drive_spec,
        "/fs:ntfs",
        "/q",
        "/y",
    ];
    log::debug!("执行命令: {}", cmd.join(" "));

    let mut child = match Command::new("format.com")
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                log::error!("找不到 format.com，请检查系统路径");
                return (false, "找不到 format.com (系统找不到指定的文件)".to_string());
            }
            log::error!("格式化执行失败: {e}");
            return (false, e.to_string());
        }
    };

    let t_out = spawn_reader(child.stdout.take());
    let t_err = spawn_reader(child.stderr.take());
    let status = wait_with_timeout(&mut child, FORMAT_TIMEOUT);
    let Some(status) = status else {
        let _ = t_out.join();
        let _ = t_err.join();
        log::error!("格式化执行超时 ({FORMAT_TIMEOUT}秒)");
        return (false, "格式化执行超时".to_string());
    };

    let output = decode_acp(&t_out.join().unwrap_or_default())
        + &decode_acp(&t_err.join().unwrap_or_default());
    log::debug!("format 输出:\n{output}");

    let rc = status.code().unwrap_or(-1);
    if rc != 0 {
        log::error!("格式化失败 (返回码 {rc}): {output}");
        return (false, output);
    }

    log::info!("格式化成功: {drive_spec}");
    (true, output)
}

/// 修复系统引导（← `repair_boot`）
///
/// bootsect 失败仅告警并继续；bcdboot 失败才置 `all_ok = False`。
/// 超时常量沿用 dism 模块（Python 的 `BOOTSECT_TIMEOUT` / `BCDEDIT_TIMEOUT`）。
pub fn repair_boot(
    boot_partition: &str,
    system_partition: &str,
    windows_partition: &str,
) -> (bool, String) {
    log::info!("开始修复引导");
    log::info!(
        "引导分区: {boot_partition}, 系统分区: {system_partition}, Windows分区: {windows_partition}"
    );

    let mut all_ok = true;
    let mut all_output: Vec<String> = Vec::new();

    // 1. bootsect 修复 MBR 和引导扇区
    log::info!("执行 bootsect...");
    let (ok, output) = run_command(&bootsect_cmd(system_partition), BOOTSECT_TIMEOUT, true);
    all_output.push(format!("bootsect: {output}"));
    if !ok {
        log::warn!("bootsect 可能失败: {output}");
        // 继续尝试 bcdboot
    }

    // 2. bcdboot 创建引导文件（Windows 分区与 EFI 分区可能相同）
    log::info!("执行 bcdboot...");
    let (ok, output) = run_command(
        &bcdboot_cmd(windows_partition, system_partition),
        BCDBOOT_TIMEOUT,
        true,
    );
    all_output.push(format!("bcdboot: {output}"));
    if !ok {
        log::error!("bcdboot 失败: {output}");
        all_ok = false;
    } else {
        log::info!("引导修复成功");
    }

    (all_ok, all_output.join("\n"))
}

/// 在 PE 环境下对目标系统离线禁用 Windows Defender（SYSTEM + SOFTWARE hive）。
///
/// 流程：
/// 1. `reg load HKLM\mount <win>\System32\config\SYSTEM` →
///    逐个将 `ControlSet001\Services\<service>\Start` 设为 4（禁用）→
///    `reg unload HKLM\mount`；
/// 2. `reg load HKLM\mountsw <win>\System32\config\SOFTWARE` →
///    将 `Policies\Microsoft\Windows Defender\DisableAntiSpyware` 设为 1 →
///    `reg unload HKLM\mountsw`（策略键位于 SOFTWARE hive）。
///
/// 【关键】不使用 `cmd /c` 串联多条语句：cmd 对嵌套引号的解析行为不稳定，
/// 会导致 `reg.exe` 收到的路径参数带了多余的引号 →
/// 「文件名、目录名或卷标语法不正确」。
/// 改为逐条调用 `reg.exe`，每个子命令单独构造 `Command`。
///
/// 失败仅告警不阻塞安装（← 与 bootsect 失败语义一致）。
pub fn disable_defender_offline(windows_partition: &str) -> (bool, String) {
    let win_dir = format!("{windows_partition}\\Windows");
    let hive_path = format!("{win_dir}\\System32\\config\\SYSTEM");
    let services = ["Sense", "WdBoot", "WdFilter", "WdNisDrv", "WdNisSvc", "WinDefend"];

    let mut output_parts: Vec<String> = Vec::new();
    let mut all_ok = true;

    // 1. Load SYSTEM hive
    let load_out = run_reg_command(&["load", "HKLM\\mount", &hive_path], DISABLE_DEFENDER_TIMEOUT);
    output_parts.push(format!("load: {load_out}"));
    if !reg_ok(&load_out) {
        log::warn!("reg load 失败: {load_out}");
        all_ok = false;
    }

    // 2. 设置各 Defender 服务 Start=4（DISABLE）
    for svc in &services {
        let add_out = run_reg_command(
            &[
                "add",
                &format!("HKLM\\mount\\ControlSet001\\Services\\{svc}"),
                "/v", "Start",
                "/t", "REG_DWORD",
                "/d", "4",
                "/f",
            ],
            DISABLE_DEFENDER_TIMEOUT,
        );
        output_parts.push(format!("{svc}: {add_out}"));
        if !reg_ok(&add_out) {
            log::warn!("reg add {svc} 失败: {add_out}");
            all_ok = false;
        }
    }

    // 3. Unload SYSTEM hive
    let unload_out = run_reg_command(&["unload", "HKLM\\mount"], DISABLE_DEFENDER_TIMEOUT);
    output_parts.push(format!("unload: {unload_out}"));
    if !reg_ok(&unload_out) {
        log::warn!("reg unload 失败: {unload_out}");
        // unload 失败不影响结果，hive 会在系统重启后自动卸载
    }

    // 4. Load SOFTWARE hive（策略键 HKLM\SOFTWARE\Policies 位于 SOFTWARE hive）
    let sw_hive_path = format!("{win_dir}\\System32\\config\\SOFTWARE");
    let sw_load_out =
        run_reg_command(&["load", "HKLM\\mountsw", &sw_hive_path], DISABLE_DEFENDER_TIMEOUT);
    output_parts.push(format!("load SOFTWARE: {sw_load_out}"));
    if reg_ok(&sw_load_out) {
        // 5. 写入 DisableAntiSpyware 策略（reg add 自动创建缺失的中间键）
        let pol_out = run_reg_command(
            &[
                "add",
                "HKLM\\mountsw\\Policies\\Microsoft\\Windows Defender",
                "/v", "DisableAntiSpyware",
                "/t", "REG_DWORD",
                "/d", "1",
                "/f",
            ],
            DISABLE_DEFENDER_TIMEOUT,
        );
        output_parts.push(format!("DisableAntiSpyware: {pol_out}"));
        if !reg_ok(&pol_out) {
            log::warn!("reg add DisableAntiSpyware 失败: {pol_out}");
            all_ok = false;
        }

        // 6. Unload SOFTWARE hive
        let sw_unload_out =
            run_reg_command(&["unload", "HKLM\\mountsw"], DISABLE_DEFENDER_TIMEOUT);
        output_parts.push(format!("unload SOFTWARE: {sw_unload_out}"));
        if !reg_ok(&sw_unload_out) {
            log::warn!("reg unload SOFTWARE 失败: {sw_unload_out}");
            // unload 失败不影响结果，hive 会在系统重启后自动卸载
        }
    } else {
        log::warn!("reg load SOFTWARE hive 失败: {sw_load_out}");
        all_ok = false;
    }

    let output = output_parts.join("\n");
    if all_ok {
        log::info!("Windows Defender 离线禁用成功: {win_dir}");
    } else {
        log::warn!(
            "Windows Defender 离线禁用部分失败（不影响安装，将在系统内手动处理）: {output}"
        );
    }
    (all_ok, output)
}

/// 判断 `run_reg_command` 返回结果是否成功（按失败前缀反推，
/// 因为 reg 的成功输出随系统语言变化：「操作成功完成。」/「The operation completed successfully.»）。
fn reg_ok(output: &str) -> bool {
    !(output.starts_with("失败")
        || output.starts_with("启动失败")
        || output == "执行超时")
}

/// 调用 `reg.exe` 并返回输出（stdout+stderr，ACP 解码）。
fn run_reg_command(args: &[&str], timeout_secs: u64) -> String {
    use std::process::{Command, Stdio};
    let mut child = match Command::new("reg.exe")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(crate::utils::CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return format!("启动失败: {e}"),
    };
    let t_out = crate::app::install::dism::spawn_reader(child.stdout.take());
    let t_err = crate::app::install::dism::spawn_reader(child.stderr.take());
    let Some(status) = crate::app::install::dism::wait_with_timeout(&mut child, timeout_secs) else {
        let _ = t_out.join();
        let _ = t_err.join();
        return "执行超时".to_string();
    };
    let out = crate::utils::decode_acp(&t_out.join().unwrap_or_default());
    let err = crate::utils::decode_acp(&t_err.join().unwrap_or_default());
    let combined = out + &err;
    if status.code().map_or(false, |c| c == 0) {
        combined.trim().to_string()
    } else {
        format!("失败 (rc={}) {}", status.code().unwrap_or(-1), combined.trim())
    }
}

/// GHO 分区还原后的引导修复（← `gui.py::_on_gho_install` 的 bcdboot 段）。
///
/// 与 `repair_boot` 的区别：GHO 还原后**只跑 bcdboot**（Python 原版 likewise
/// 只调 bcdboot，不调 bootsect）——GHO 本身已保留原 MBR/引导扇区，
/// 只需重建 BCD。参数是查出来的 ESP 盘符与 Windows 盘符（而非分区键）。
pub fn repair_boot_from_letters(esp_letter: &str, windows_letter: &str) -> (bool, String) {
    let esp = normalize_drive_spec(esp_letter);
    let win = normalize_drive_spec(windows_letter);
    let (esp, win) = match (esp, win) {
        (Some(e), Some(w)) => (e, w),
        _ => {
            let err = format!(
                "GHO 分区还原后引导修复跳过: 盘符无效 (ESP={:?}, Windows={:?})",
                esp_letter, windows_letter
            );
            log::warn!("{err}");
            return (true, err);
        }
    };
    log::info!("GHO 分区还原后修复引导: ESP={esp}, Windows={win}");
    let cmd = bcdboot_cmd(&win, &esp);
    log::info!("执行 bcdboot: {}", cmd.join(" "));
    let (ok, output) = run_command(&cmd, BCDBOOT_TIMEOUT, true);
    if ok {
        log::info!("GHO 引导修复成功");
    } else {
        log::warn!("GHO 引导修复失败: {output}");
    }
    (ok, format!("bcdboot: {output}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ← 盘符规范化：strip + rstrip("\\/") + rstrip(":") + 首字母 + 大写
    #[test]
    fn drive_spec_normalization() {
        assert_eq!(normalize_drive_spec("C:"), Some("C:".to_string()));
        assert_eq!(normalize_drive_spec("c"), Some("C:".to_string()));
        assert_eq!(normalize_drive_spec(" D"), Some("D:".to_string()));
        // 尾随反斜杠/正斜杠先剥，再剥冒号
        assert_eq!(normalize_drive_spec("E:\\"), Some("E:".to_string()));
        assert_eq!(normalize_drive_spec("E:/"), Some("E:".to_string()));
        assert_eq!(normalize_drive_spec(" E:  "), Some("E:".to_string()));
        // 无效输入
        assert_eq!(normalize_drive_spec(""), None);
        assert_eq!(normalize_drive_spec("   "), None);
        assert_eq!(normalize_drive_spec(":"), None);
        assert_eq!(normalize_drive_spec("1:"), None);
        assert_eq!(normalize_drive_spec("12"), None);
    }

    /// ← repair_boot 命令形状
    #[test]
    fn boot_cmd_shapes() {
        assert_eq!(
            bootsect_cmd("C:"),
            ["bootsect", "/nt60", "C:", "/force", "/mbr"]
        );
        assert_eq!(
            bcdboot_cmd("C:", "C:"),
            ["bcdboot", "C:\\Windows", "/s", "C:", "/f", "all"]
        );
        // 非 Windows 分区（wimlib 场景常见 D:）
        assert_eq!(
            bcdboot_cmd("D:", "B:"),
            ["bcdboot", "D:\\Windows", "/s", "B:", "/f", "all"]
        );
    }
}
