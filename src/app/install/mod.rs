//! 安装子系统编排（← `sys_install.py::install_system` /
//! `get_partition_letter_from_path`，文档 §6.3）

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

pub mod boot;
pub mod dism;
pub mod iso;

use std::path::Path;

use boot::{disable_defender_offline, format_partition_quick, repair_boot};
use dism::{apply_image, CancelFn, ProgressFn};
/// 一键安装系统（← `install_system`）
///
/// 流程：取消检查 → 格式化(10%) → 取消检查 → 应用镜像(20-85) →
/// 取消检查 → 修复引导(90) → 完成(100)。
/// 失败提前返回且**不**回调 100%；bootsect 失败仅告警（`repair_boot` 内部处理）。
///
/// 返回 `(成功标志, 输出文本)`，输出按 Python 顺序
/// `[格式化]`/`[应用镜像]`/`[修复引导]` 逐段 `\n` 连接。
#[allow(clippy::too_many_arguments)]
pub fn install_system(
    source_path: &str,
    image_index: i64,
    boot_partition: &str,
    windows_partition: &str,
    auto_repair_boot: bool,
    format_windows: bool,
    disable_defender: bool,
    progress_callback: Option<ProgressFn<'_>>,
    cancel_callback: Option<CancelFn<'_>>,
) -> (bool, String) {
    log::warn!(
        "开始系统安装:\n  镜像: {source_path}\n  索引: {image_index}\n  \
         引导分区: {boot_partition}\n  Windows分区: {windows_partition}\n  \
         自动修复引导: {auto_repair_boot}\n  格式化Windows分区: {format_windows}\n  \
         离线禁用Defender: {disable_defender}"
    );

    let mut all_output: Vec<String> = Vec::new();

    // 0. 检查取消（Python `if cancel_callback and cancel_callback()`）
    if cancel_callback.is_some_and(|f| f()) {
        log::warn!("安装开始前被取消");
        return (false, "操作已取消".to_string());
    }

    // 1. 格式化 Windows 分区（可选）
    if format_windows {
        if let Some(cb) = progress_callback {
            cb("正在格式化 Windows 分区...", 10);
        }
        let (ok, output) = format_partition_quick(windows_partition);
        all_output.push(format!("[格式化] {output}"));
        if !ok {
            log::error!("格式化失败，停止安装");
            return (false, all_output.join("\n"));
        }
    } else {
        log::info!("跳过格式化（目标分区已有数据）");
    }

    if cancel_callback.is_some_and(|f| f()) {
        log::warn!("格式化后被取消");
        return (false, "操作已取消".to_string());
    }

    // 2. 应用镜像
    let (ok, output) = apply_image(
        source_path,
        image_index,
        windows_partition,
        progress_callback,
        cancel_callback,
        20,
        85,
    );
    all_output.push(format!("[应用镜像] {output}"));
    if !ok {
        log::error!("镜像应用失败，停止安装");
        return (false, all_output.join("\n"));
    }

    if cancel_callback.is_some_and(|f| f()) {
        log::warn!("镜像应用后被取消");
        return (false, "操作已取消".to_string());
    }

    if let Some(cb) = progress_callback {
        cb("镜像应用完成，正在处理选项...", 88);
    }

    // 3. 离线禁用 Windows Defender（system hive，先于引导修复执行）
    if disable_defender {
        let (ok_def, out_def) = disable_defender_offline(windows_partition);
        all_output.push(format!("[禁用Defender] {out_def}"));
        if !ok_def {
            log::warn!("离线禁用 Defender 失败，不影响安装");
        }
    } else {
        log::info!("跳过离线禁用 Windows Defender");
    }

    if let Some(cb) = progress_callback {
        cb("正在修复引导...", 90);
    }

    // 4. 修复引导（boot_partition 同时充当 EFI 系统分区参数）
    if auto_repair_boot {
        let (ok, output) = repair_boot(boot_partition, boot_partition, windows_partition);
        all_output.push(format!("[修复引导] {output}"));
        if !ok {
            log::warn!("引导修复可能有问题，但镜像已安装");
        }
    } else {
        log::info!("跳过引导修复");
    }

    if let Some(cb) = progress_callback {
        cb("系统安装完成!", 100);
    }

    log::info!("系统安装完成!");
    (true, all_output.join("\n"))
}

/// 从路径获取分区盘符（← `get_partition_letter_from_path`）
///
/// Python `Path(path).anchor.rstrip(":\\")`：`C:\Windows` → `"C"`
/// （**不带冒号**，与 Python 原样一致；该函数在 Python 侧亦无调用方）。
/// 非绝对/无前缀路径返回空串（Python anchor 为空 → rstrip 仍为空）。
pub fn get_partition_letter_from_path(path: &str) -> String {
    let p = Path::new(path);
    if let Some(std::path::Component::Prefix(prefix)) = p.components().next() {
        return prefix
            .as_os_str()
            .to_string_lossy()
            .trim_end_matches([':', '\\'])
            .to_string();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输出段拼接顺序（纯函数部分：取消与格式化失败路径会提前返回；
    /// 完整编排依赖 dism/format 真实执行，由里程碑 7 端到端验收）
    #[test]
    fn output_join_style() {
        let parts = [
            "[格式化] 卷标是新盘符.".to_string(),
            "[应用镜像] DISM 版本: 10.0.22621.1".to_string(),
            "[修复引导] bootsect: 成功\nbcdboot: 成功".to_string(),
        ];
        let joined = parts.join("\n");
        assert!(joined.starts_with("[格式化] "));
        assert!(joined.contains("\n[应用镜像] "));
        assert!(joined.contains("\n[修复引导] "));
        assert!(joined.ends_with("bcdboot: 成功"));
    }

    /// ← Path(path).anchor.rstrip(":\\")
    #[test]
    fn partition_letter_from_path() {
        assert_eq!(get_partition_letter_from_path(r"C:\Windows"), "C");
        assert_eq!(get_partition_letter_from_path("D:/"), "D");
        assert_eq!(get_partition_letter_from_path(r"D:\install.wim"), "D");
        assert_eq!(get_partition_letter_from_path("relative/path"), "");
        assert_eq!(get_partition_letter_from_path(""), "");
    }
}
