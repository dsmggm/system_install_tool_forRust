//! 系统优化：部署应答文件与调试工具（← `sys_optimize.py`，文档 §6.3/§10）
//!
//! 内置模板与 Python 一致：`assets/unattend.xml` 由 `include_str!` 编译进 exe
//! （对应 Python 的 `UNATTEND_TEMPLATE` 常量 + 根目录 `unattend.xml` 覆盖逻辑）。

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::app::copy_tools::copy_tools_to_panther;
use crate::app::install::dism::ProgressFn;
use crate::utils::base_dir;

/// 内置 unattend.xml 模板（← `UNATTEND_TEMPLATE`）
pub const UNATTEND_TEMPLATE: &str = include_str!("../../assets/unattend.xml");

/// 关闭 Windows 防火墙的 specialize 阶段脚本块（← `FIREWALL_DISABLE_SCRIPTS`）
pub const FIREWALL_DISABLE_SCRIPTS: &str = "\t{\n\t\tnetsh advfirewall set allprofiles state off\n\t};";

/// 替换 `%disable_firwall%` 占位符（← `replace_firewall_placeholder`）
///
/// 注意占位符拼写为 `firwall`（与 unattend.xml 模板保持一致）。
pub fn replace_firewall_placeholder(xml_content: &str, disable_firewall: bool) -> String {
    if disable_firewall {
        xml_content.replace("%disable_firwall%", FIREWALL_DISABLE_SCRIPTS)
    } else {
        xml_content.replace("%disable_firwall%", "")
    }
}

/// 替换 `%username%` 占位符（← `replace_username_placeholder`；
/// 空用户名回退 `"admin"`）
pub fn replace_username_placeholder(xml_content: &str, username: &str) -> String {
    let username = if username.is_empty() { "admin" } else { username };
    xml_content.replace("%username%", username)
}

/// 替换语言/区域占位符（← `replace_language_placeholders`）
pub fn replace_language_placeholders(
    xml_content: &str,
    ui_language: &str,
    user_locale: &str,
) -> String {
    xml_content
        .replace("%UILanguage%", ui_language)
        .replace("%UserLocale%", user_locale)
}

/// 构造「进系统后执行」的 CMD 命令（`unattend-01.cmd` 中的一行）
///
/// - `start "" /wait "<脚本>"`：`start` 的空标题参数不可省（否则带引号的
///   路径会被当成窗口标题）；`/wait` 保证脚本跑完再继续，日志顺序不串。
/// - `if exist` 守卫：脚本可能没被复制到桌面（用户没勾「复制调试工具」），
///   缺失时静默跳过，避免首次登录的 PowerShell 脚本块抛错中断后续步骤。
pub fn build_run_after_command(script_path: &str) -> String {
    let path = script_path.trim();
    let path = if path.is_empty() {
        crate::config::settings::DEFAULT_RUN_AFTER
    } else {
        path
    };
    format!("if exist \"{path}\" start \"\" /wait \"{path}\"")
}

/// 替换 `%run_after%` 占位符（模板中位于 `C:\Windows\Setup\Scripts\unattend-01.cmd`）
///
/// 该 .cmd 由 `FirstLogon.ps1` 在首次登录时调用，此时调试工具已被移动到
/// 桌面，`%USERPROFILE%` 即当前用户目录，故默认路径可直接使用。
///
/// - 勾选 → 写入执行脚本的命令；
/// - 未勾选 → 清空（等价于模板里没有这一行）。
pub fn replace_run_after_placeholder(
    xml_content: &str,
    run_after: bool,
    script_path: &str,
) -> String {
    if run_after {
        xml_content.replace("%run_after%", &build_run_after_command(script_path))
    } else {
        xml_content.replace("%run_after%", "")
    }
}

/// 获取默认模板内容（← `get_default_template_content`）
///
/// 优先顺序：1. exe 目录 `unattend.xml`；2. 内置常量。
pub fn get_default_template_content() -> String {
    let template_path = base_dir().join("unattend.xml");
    if template_path.exists() {
        match std::fs::read_to_string(&template_path) {
            Ok(content) => {
                log::debug!(
                    "从根目录模板加载: {}, 长度: {}",
                    template_path.display(),
                    content.len()
                );
                return content;
            }
            Err(e) => log::debug!("读取根目录模板失败，回退内置模板: {e}"),
        }
    }
    UNATTEND_TEMPLATE.to_string()
}

/// 盘符规范化：`rstrip("\\")` 后确保以 `:` 结尾（← `deploy_unattend_xml` 内联逻辑）
fn normalize_windows_partition(windows_partition: &str) -> String {
    let mut p = windows_partition.trim_end_matches('\\').to_string();
    if !p.ends_with(':') {
        p.push(':');
    }
    p
}

/// `os.path.join(盘符, "Windows", "panther")` 等价的盘符相对拼接
/// （与 copy_tools 的 `panther_target_dir` 同一规则）
fn panther_dir(windows_partition: &str) -> String {
    format!("{}\\Windows\\panther", normalize_windows_partition(windows_partition))
}

/// 尝试创建目录（← `_try_makedirs`）
fn try_makedirs(path: &Path) -> bool {
    std::fs::create_dir_all(path).is_ok()
}

/// 部署 unattend.xml 到目标系统（← `deploy_unattend_xml`）
///
/// 流程：规范化盘符 → 建 `X:\Windows\panther` → 内存渲染（用户名/防火墙/
/// 进系统后执行脚本）→ 写入 → 同步到 `System32\sysprep\panther`（失败仅告警）。
///
/// 返回 `(成功标志, 输出文本)`。
#[allow(clippy::too_many_arguments)]
pub fn deploy_unattend_xml(
    windows_partition: &str,
    username: &str,
    _wim_path: &str,
    _image_index: i64,
    progress_callback: Option<ProgressFn<'_>>,
    disable_firewall: bool,
    run_after: bool,
    run_after_script: &str,
) -> (bool, String) {
    log::info!("开始部署 unattend.xml 到 {windows_partition}");

    if let Some(cb) = progress_callback {
        cb("正在部署应答文件...", 95);
    }

    let panther = panther_dir(windows_partition);
    let target_path = format!("{panther}\\unattend.xml");

    // 在目标系统 Windows 目录下创建 panther 文件夹
    let panther_path = PathBuf::from(&panther);
    if !panther_path.exists() {
        if let Err(e) = std::fs::create_dir_all(&panther_path) {
            log::error!("部署 unattend.xml 失败: {e}");
            return (false, e.to_string());
        }
        log::debug!("创建目录: {panther}");
    } else {
        log::debug!("目录已存在: {panther}");
    }

    // 在内存中创建 unattend.xml 内容
    let template_content = get_default_template_content();
    let xml_content = replace_username_placeholder(&template_content, username);
    let xml_content = replace_firewall_placeholder(&xml_content, disable_firewall);
    let xml_content = replace_run_after_placeholder(&xml_content, run_after, run_after_script);
    log::debug!(
        "内存中创建 unattend.xml 完成，用户名: {username}, \
         关闭防火墙: {}, 进系统后执行: {} ({})",
        if disable_firewall { "是" } else { "否" },
        if run_after { "是" } else { "否" },
        run_after_script
    );

    if let Some(cb) = progress_callback {
        cb("正在复制 unattend.xml 到目标系统...", 97);
    }

    // 写入目标系统的 panther 文件夹
    if let Err(e) = std::fs::write(&target_path, &xml_content) {
        log::error!("部署 unattend.xml 失败: {e}");
        return (false, e.to_string());
    }
    log::info!("unattend.xml 已部署到: {target_path}");

    if let Some(cb) = progress_callback {
        cb("应答文件部署完成", 98);
    }

    // 同步到 sysprep\panther 目录（兼容性，失败可忽略）
    let sysprep_panther = format!(
        "{}\\Windows\\System32\\sysprep\\panther",
        normalize_windows_partition(windows_partition)
    );
    let sysprep_parent = Path::new(&sysprep_panther)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    if sysprep_parent.exists() || try_makedirs(&sysprep_parent) {
        let sysprep_path = PathBuf::from(&sysprep_panther);
        if !sysprep_path.exists() {
            if let Err(e) = std::fs::create_dir_all(&sysprep_path) {
                log::warn!("同步到 sysprep 目录失败（可忽略）: {e}");
                log::info!("unattend.xml 部署成功");
                return (true, format!("unattend.xml 已部署到 {target_path}"));
            }
        }
        let sysprep_target = format!("{sysprep_panther}\\unattend.xml");
        match std::fs::write(&sysprep_target, &xml_content) {
            Ok(()) => log::info!("unattend.xml 已同步到: {sysprep_target}"),
            Err(e) => log::warn!("同步到 sysprep 目录失败（可忽略）: {e}"),
        }
    }

    log::info!("unattend.xml 部署成功");
    (true, format!("unattend.xml 已部署到 {target_path}"))
}

/// 执行系统优化（← `optimize_system`）
///
/// 应答文件部署与调试工具复制相互独立：应答文件失败置 `has_error`，
/// 调试工具失败仅告警不影响返回值。
#[allow(clippy::too_many_arguments)]
pub fn optimize_system(
    windows_partition: &str,
    deploy_autounattend: bool,
    admin_username: &str,
    wim_path: &str,
    image_index: i64,
    copy_debug_tools: bool,
    progress_callback: Option<ProgressFn<'_>>,
    tools_source_path: &str,
    disable_firewall: bool,
    run_after: bool,
    run_after_script: &str,
) -> (bool, String) {
    let mut all_output: Vec<String> = Vec::new();
    let mut has_error = false;

    if !deploy_autounattend && !copy_debug_tools {
        log::info!("跳过系统优化（应答文件部署与调试工具均未启用）");
        return (true, "优化跳过（均未启用）".to_string());
    }

    if let Some(cb) = progress_callback {
        cb("正在执行系统优化...", 92);
    }

    // 部署 unattend.xml（独立步骤）
    if deploy_autounattend {
        let (ok, output) = deploy_unattend_xml(
            windows_partition,
            admin_username,
            wim_path,
            image_index,
            progress_callback,
            disable_firewall,
            run_after,
            run_after_script,
        );
        all_output.push(format!("[应答文件] {output}"));
        if !ok {
            log::error!("部署应答文件失败: {output}");
            has_error = true;
        }
    }

    // 复制调试工具（独立步骤）
    if copy_debug_tools {
        let (ok_tools, output_tools) =
            copy_tools_to_panther(windows_partition, progress_callback, tools_source_path);
        all_output.push(format!("[调试工具] {output_tools}"));
        if !ok_tools {
            log::warn!("复制调试工具失败（不影响主流程）: {output_tools}");
        }
    }

    if let Some(cb) = progress_callback {
        cb("系统优化完成", 100);
    }

    log::info!("系统优化完成");
    (!has_error, all_output.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ← 模板占位符计数与替换语义
    #[test]
    fn template_placeholders() {
        // 内置模板中 %username% 出现 3 次
        assert_eq!(UNATTEND_TEMPLATE.matches("%username%").count(), 3);

        let replaced = replace_username_placeholder(UNATTEND_TEMPLATE, "testuser");
        assert!(!replaced.contains("%username%"));
        assert_eq!(replaced.matches("testuser").count(), 3);

        // 空用户名 → "admin"
        let defaulted = replace_username_placeholder(UNATTEND_TEMPLATE, "");
        assert!(!defaulted.contains("%username%"));
        assert!(defaulted.contains("<Name>admin</Name>"));
    }

    /// ← 语言占位符替换（内置模板无此占位符时保持原样）
    #[test]
    fn language_placeholders() {
        assert_eq!(
            replace_language_placeholders("L=%UILanguage% R=%UserLocale%", "zh-CN", "zh-Hans-CN"),
            "L=zh-CN R=zh-Hans-CN"
        );
        assert_eq!(
            replace_language_placeholders("no placeholders", "zh-CN", "zh-Hans-CN"),
            "no placeholders"
        );
    }

    /// ← 盘符规范化与 panther 目录拼接（盘符相对，无中间反斜杠于首段）
    #[test]
    fn panther_dir_shape() {
        assert_eq!(normalize_windows_partition("C:"), "C:");
        assert_eq!(normalize_windows_partition("C:\\"), "C:");
        assert_eq!(normalize_windows_partition("C"), "C:");
        assert_eq!(panther_dir("C:"), "C:\\Windows\\panther");
        assert_eq!(panther_dir("D:\\"), "D:\\Windows\\panther");
    }

    /// ← 输出段拼接顺序
    #[test]
    fn optimize_output_join() {
        let parts = [
            "[应答文件] unattend.xml 已部署到 C:\\Windows\\panther\\unattend.xml".to_string(),
            "[调试工具] 调试工具已复制到 C:\\Windows\\panther\\TestTools".to_string(),
        ];
        let joined = parts.join("\n");
        assert!(joined.starts_with("[应答文件] "));
        assert!(joined.contains("\n[调试工具] "));
    }

    /// ← `%run_after%`：内置模板存在该占位符，勾选后写入命令、未勾选则清空
    #[test]
    fn run_after_placeholder() {
        // 模板中 `%run_after%` 恰好出现 1 次（unattend-01.cmd 末尾）
        assert_eq!(UNATTEND_TEMPLATE.matches("%run_after%").count(), 1);

        let on = replace_run_after_placeholder(
            UNATTEND_TEMPLATE,
            true,
            r"%USERPROFILE%\Desktop\TestTools\run_after.bat",
        );
        assert!(!on.contains("%run_after%"));
        assert!(on.contains(
            "if exist \"%USERPROFILE%\\Desktop\\TestTools\\run_after.bat\" \
             start \"\" /wait \"%USERPROFILE%\\Desktop\\TestTools\\run_after.bat\""
        ));

        let off = replace_run_after_placeholder(UNATTEND_TEMPLATE, false, "ignored.bat");
        assert!(!off.contains("%run_after%"));
        assert!(!off.contains("ignored.bat"));
        // 只删掉行内容：其余模板文本保持原长度关系
        assert!(off.len() < UNATTEND_TEMPLATE.len());
    }

    /// 空脚本路径回退默认值；路径首尾空白被裁掉
    #[test]
    fn run_after_command_shape() {
        assert_eq!(
            build_run_after_command("   "),
            format!(
                "if exist \"{}\" start \"\" /wait \"{}\"",
                crate::config::settings::DEFAULT_RUN_AFTER,
                crate::config::settings::DEFAULT_RUN_AFTER
            )
        );
        assert_eq!(
            build_run_after_command(r"  D:\a b\x.bat  "),
            "if exist \"D:\\a b\\x.bat\" start \"\" /wait \"D:\\a b\\x.bat\""
        );
    }

    /// 防火墙与 run_after 两个占位符互不影响（同时替换顺序无关）
    #[test]
    fn placeholders_are_independent() {
        let xml = replace_firewall_placeholder(UNATTEND_TEMPLATE, true);
        let xml = replace_run_after_placeholder(&xml, true, r"C:\x.bat");
        assert!(!xml.contains("%disable_firwall%"));
        assert!(!xml.contains("%run_after%"));
        assert!(xml.contains("netsh advfirewall set allprofiles state off"));
        assert!(xml.contains("if exist \"C:\\x.bat\" start \"\" /wait \"C:\\x.bat\""));
    }
}
