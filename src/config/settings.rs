//! config.ini 读写（← config.py::AppConfig）
//!
//! 格式：
//! - 单 section `[settings]`，`key = value`（等号两侧空格），UTF-8 无 BOM，段尾空行；
//! - 仅识别 `[settings]` 段，其它段的键一律忽略（按默认值运行）；
//! - 解析容忍等号两侧空白（等价 configparser）；
//! - 路径 = exe 同目录（对应 get_config_path 的打包语义）；
//! - 文件不存在 → 生成默认值并保存。

// 里程碑 6（GUI）接入前部分 API 尚未被引用，最终会全部启用
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use log::{error, info, warn};

use crate::utils::base_dir;

/// 客户端运行状态（默认值与 config.py 完全一致）
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// 自动还原服务开关（默认关闭）
    pub restore_enabled: bool,
    /// 自动执行倒计时秒数
    pub countdown_sec: i64,
    /// 当前方案文件路径
    pub plan_path: String,
    /// 安装完成后自动重启
    pub auto_restart: bool,
    /// 复制调试工具
    pub copy_tools: bool,
    /// 自动部署与系统优化
    pub deploy_unattend: bool,
    /// 禁用 Windows Defender
    pub disable_defender: bool,
    /// 关闭 Windows 防火墙
    pub disable_firewall: bool,
    /// 调试工具源路径（无默认值，需用户配置）
    pub tools_source_path: String,
    /// 安装/还原完成后的重启倒计时秒数（「系统安装完成」对话框）
    pub restart_countdown_sec: i64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            restore_enabled: false,
            countdown_sec: 30,
            plan_path: String::new(),
            auto_restart: false,
            copy_tools: false,
            deploy_unattend: false,
            disable_defender: false,
            disable_firewall: false,
            tools_source_path: String::new(),
            restart_countdown_sec: 30,
        }
    }
}

/// config.ini 路径：exe 同目录。
pub fn get_config_path() -> PathBuf {
    base_dir().join("config.ini")
}

/// 等价 configparser 的 getboolean：接受 1/0/true/false/yes/no/on/off（大小写不敏感）。
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl AppConfig {
    /// 从 config.ini 加载；文件不存在则返回默认值并自动生成。
    pub fn load(path: Option<&Path>) -> AppConfig {
        let path = path.map(|p| p.to_path_buf()).unwrap_or_else(get_config_path);
        let mut cfg = AppConfig::default();
        if path.exists() {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let text = text.trim_start_matches('\u{feff}'); // 容忍 BOM
                    let map = parse_ini(text);
                    if let Some(v) = get_str(&map, "enabled") {
                        match parse_bool(&v) {
                            Some(b) => cfg.restore_enabled = b,
                            None => warn!("config.ini enabled 值无效 '{v}'，使用默认值"),
                        }
                    }
                    if let Some(v) = get_str(&map, "countdown") {
                        match v.trim().parse::<i64>() {
                            Ok(n) => cfg.countdown_sec = n,
                            Err(_) => warn!("config.ini countdown 值无效 '{v}'，使用默认值 30"),
                        }
                    }
                    if let Some(v) = get_str(&map, "plan_path") {
                        cfg.plan_path = v;
                    }
                    cfg.auto_restart = get_bool(&map, "auto_restart", false);
                    cfg.copy_tools = get_bool(&map, "copy_tools", false);
                    cfg.deploy_unattend = get_bool(&map, "deploy_unattend", false);
                    cfg.disable_defender = get_bool(&map, "disable_defender", false);
                    cfg.disable_firewall = get_bool(&map, "disable_firewall", false);
                    if let Some(v) = get_str(&map, "tools_source_path") {
                        cfg.tools_source_path = v;
                    }
                    if let Some(v) = get_str(&map, "restart_countdown") {
                        match v.trim().parse::<i64>() {
                            Ok(n) if n >= 1 => cfg.restart_countdown_sec = n,
                            _ => warn!("config.ini restart_countdown 值无效 '{v}'，使用默认值 30"),
                        }
                    }
                    info!(
                        "config.ini 加载成功: restore={}, {}s, plan='{}', \
                         auto_restart={}, copy_tools={}, deploy_unattend={}, \
                         disable_defender={}, disable_firewall={}, restart_countdown={}s",
                        cfg.restore_enabled,
                        cfg.countdown_sec,
                        cfg.plan_path,
                        cfg.auto_restart,
                        cfg.copy_tools,
                        cfg.deploy_unattend,
                        cfg.disable_defender,
                        cfg.disable_firewall,
                        cfg.restart_countdown_sec
                    );
                }
                Err(e) => warn!("读取 config.ini 失败，使用默认值: {e}"),
            }
        } else {
            info!("config.ini 不存在，自动初始化（默认关闭自动还原服务）");
            cfg.save(Some(&path));
        }
        cfg
    }

    /// 将当前配置写入 config.ini。
    /// 带中文注释（`#` 行，configparser/parse_ini 读取时都会忽略）；
    /// 每次保存都重写整份文件，注释因此常驻。
    pub fn save(&self, path: Option<&Path>) -> PathBuf {
        let path = path.map(|p| p.to_path_buf()).unwrap_or_else(get_config_path);
        let bool_str = |b: bool| if b { "1" } else { "0" };
        // configparser 在 Windows 文本模式写出 → 行尾必须是 CRLF
        let lines = [
            "[settings]".to_string(),
            "# 自动还原服务开关（1=启动后自动执行还原，0=关闭）".to_string(),
            format!("enabled = {}", bool_str(self.restore_enabled)),
            "# 自动还原执行倒计时（秒）".to_string(),
            format!("countdown = {}", self.countdown_sec),
            "# 当前方案文件路径".to_string(),
            format!("plan_path = {}", self.plan_path),
            "# 安装完成后自动重启（1=完成后弹倒计时对话框重启，0=询问是否重启）".to_string(),
            format!("auto_restart = {}", bool_str(self.auto_restart)),
            "# 复制调试工具（1=开，0=关）".to_string(),
            format!("copy_tools = {}", bool_str(self.copy_tools)),
            "# 自动部署与系统优化（1=开，0=关）".to_string(),
            format!("deploy_unattend = {}", bool_str(self.deploy_unattend)),
            "# 禁用 Windows Defender（1=开，0=关）".to_string(),
            format!("disable_defender = {}", bool_str(self.disable_defender)),
            "# 关闭 Windows 防火墙（1=开，0=关）".to_string(),
            format!("disable_firewall = {}", bool_str(self.disable_firewall)),
            "# 调试工具源路径".to_string(),
            format!("tools_source_path = {}", self.tools_source_path),
            "# 安装/还原完成后的重启倒计时（秒）".to_string(),
            format!("restart_countdown = {}", self.restart_countdown_sec),
            String::new(), // 段尾空行（configparser.write 在 section 后加空行）
        ];
        let content = lines.join("\r\n") + "\r\n";
        if let Err(e) = fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new("."))) {
            error!("写入 config.ini 失败: {e}");
            return path;
        }
        match fs::write(&path, content) {
            Ok(()) => info!("config.ini 已保存: {}", path.display()),
            Err(e) => error!("写入 config.ini 失败: {e}"),
        }
        path
    }
}

/// 解析 `key = value` 单 section 文本为 map。
/// 仅收集 `[settings]` 段内的键，其它段的键一律忽略。
fn parse_ini(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_settings = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            let name = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim();
            in_settings = name.eq_ignore_ascii_case("settings");
            continue;
        }
        if in_settings {
            if let Some((k, v)) = line.split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

fn get_str(map: &[(String, String)], key: &str) -> Option<String> {
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn get_bool(map: &[(String, String)], key: &str, fallback: bool) -> bool {
    match get_str(map, key) {
        Some(v) => match parse_bool(&v) {
            Some(b) => b,
            None => {
                warn!("config.ini {key} 值无效 '{v}'，使用默认值");
                fallback
            }
        },
        None => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cfg(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sks_cfg_test_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("config.ini")
    }

    /// 生成内容：CRLF + 段尾空行 + 每个键的中文注释 + restart_countdown。
    #[test]
    fn save_writes_comments_and_restart_countdown() {
        let path = temp_cfg("fmt");
        let cfg = AppConfig::default();
        cfg.save(Some(&path));
        let text = fs::read_to_string(&path).unwrap();
        let expected = "[settings]\r\n\
            # 自动还原服务开关（1=启动后自动执行还原，0=关闭）\r\n\
            enabled = 0\r\n\
            # 自动还原执行倒计时（秒）\r\n\
            countdown = 30\r\n\
            # 当前方案文件路径\r\n\
            plan_path = \r\n\
            # 安装完成后自动重启（1=完成后弹倒计时对话框重启，0=询问是否重启）\r\n\
            auto_restart = 0\r\n\
            # 复制调试工具（1=开，0=关）\r\n\
            copy_tools = 0\r\n\
            # 自动部署与系统优化（1=开，0=关）\r\n\
            deploy_unattend = 0\r\n\
            # 禁用 Windows Defender（1=开，0=关）\r\n\
            disable_defender = 0\r\n\
            # 关闭 Windows 防火墙（1=开，0=关）\r\n\
            disable_firewall = 0\r\n\
            # 调试工具源路径\r\n\
            tools_source_path = \r\n\
            # 安装/还原完成后的重启倒计时（秒）\r\n\
            restart_countdown = 30\r\n\r\n";
        assert_eq!(text, expected);
        let _ = fs::remove_file(&path);
    }

    /// restart_countdown 解析：合法值生效；非法/<=0 回落默认 30。
    /// 这里用新格式段名 `[settings]`（解析不区分 section 名，新旧均可）。
    #[test]
    fn load_restart_countdown_variants() {
        let path = temp_cfg("rcd");
        fs::write(&path, "[settings]\r\nrestart_countdown = 5\r\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.restart_countdown_sec, 5);

        fs::write(&path, "[settings]\r\nrestart_countdown = abc\r\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.restart_countdown_sec, 30);

        fs::write(&path, "[settings]\r\nrestart_countdown = 0\r\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.restart_countdown_sec, 30);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_missing_generates_default_file() {
        let path = temp_cfg("missing");
        assert!(!path.exists());
        let cfg = AppConfig::load(Some(&path));
        assert!(path.exists());
        assert!(!cfg.restore_enabled);
        assert_eq!(cfg.countdown_sec, 30);
        assert_eq!(cfg.plan_path, "");
        let _ = fs::remove_file(&path);
    }

    /// bool 变体（0/1、yes/true、off/FALSE）在 `[settings]` 下可正确读取；
    /// save→load 往返一致。
    #[test]
    fn load_settings_section_and_bool_variants() {
        let path = temp_cfg("variants");
        let content = "[settings]\r\n\
            enabled = 1\r\n\
            countdown = 10\r\n\
            plan_path = D:\\plans\\demo.bat\r\n\
            auto_restart = true\r\n\
            copy_tools = yes\r\n\
            deploy_unattend = 0\r\n\
            disable_defender = FALSE\r\n\
            tools_source_path = D:\\Tools\r\n";
        fs::write(&path, content).unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert!(cfg.restore_enabled);
        assert_eq!(cfg.countdown_sec, 10);
        assert_eq!(cfg.plan_path, "D:\\plans\\demo.bat");
        assert!(cfg.auto_restart);
        assert!(cfg.copy_tools);
        assert!(!cfg.deploy_unattend);
        assert!(!cfg.disable_defender);
        assert_eq!(cfg.tools_source_path, "D:\\Tools");

        // 往返：save（[settings] + 注释）后再 load 结果一致
        cfg.save(Some(&path));
        let cfg2 = AppConfig::load(Some(&path));
        assert_eq!(cfg.plan_path, cfg2.plan_path);
        assert_eq!(cfg.countdown_sec, cfg2.countdown_sec);
        assert!(cfg2.restore_enabled && cfg2.auto_restart && cfg2.copy_tools);
        let _ = fs::remove_file(&path);
    }

    /// `[settings]` 之外的段一律忽略 → 键按默认值处理。
    #[test]
    fn load_unknown_section_is_ignored() {
        let path = temp_cfg("unknown");
        let content = "[legacy]\r\n\
            enabled = 1\r\n\
            countdown = 10\r\n\
            plan_path = D:\\plans\\demo.bat\r\n";
        fs::write(&path, content).unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert!(!cfg.restore_enabled);
        assert_eq!(cfg.countdown_sec, 30);
        assert_eq!(cfg.plan_path, "");
        let _ = fs::remove_file(&path);
    }

    /// 缺失键回落默认值（等价 configparser 的 fallback）。
    #[test]
    fn load_missing_keys_uses_fallbacks() {
        let path = temp_cfg("partial");
        fs::write(&path, "[settings]\nenabled = 1\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert!(cfg.restore_enabled);
        assert_eq!(cfg.countdown_sec, 30);
        assert_eq!(cfg.tools_source_path, "");
        let _ = fs::remove_file(&path);
    }
}
