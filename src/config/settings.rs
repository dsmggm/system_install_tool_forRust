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

use log::{info, warn};

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
    /// 安装/还原完成后的重启倒计时秒数（「系统安装完成」对
    ///话框中的倒计时控件）
    pub restart_countdown_sec: i64,
    /// 主题模式：auto / light / dark
    pub theme_mode: String,
    /// 「进系统后执行」的脚本路径（unattend.xml 中 `%run_after%` 占位符的
    /// 替换内容；默认值为随调试工具一起复制到桌面的 `run_after.bat`）
    pub run_after: String,
    /// 「进系统后执行」复选框状态（与 copy_tools / deploy_unattend 同口径）
    pub run_after_enabled: bool,
}

/// 「进系统后执行」脚本默认路径（← config.ini 中 `run_after` 的默认值）
///
/// 该脚本由 FirstLogon 阶段的 `unattend-01.cmd` 调用，此时调试工具已被
/// `FirstLogon.ps1` 移动到桌面，故默认指向桌面 `TestTools\run_after.bat`。
pub const DEFAULT_RUN_AFTER: &str = r"%USERPROFILE%\Desktop\TestTools\run_after.bat";

/// 主题模式的合法取值（config.ini `theme_mode`）
const THEME_MODES: [&str; 3] = ["auto", "light", "dark"];

/// 主题模式默认值：初始化生成的配置一律为 auto（跟随系统深浅色）
pub const DEFAULT_THEME_MODE: &str = "auto";

/// 归一化 `theme_mode`：空值 / 未知值 / 大小写不规范 一律回退 auto。
///
/// 作用有二：老版本或手工改坏的配置不会因为一个非法值把界面卡在某个
/// 主题上；`save` 时也会用归一后的值写回，让文件自我修复。
fn normalize_theme_mode(value: &str) -> String {
    let v = value.trim().to_ascii_lowercase();
    if THEME_MODES.contains(&v.as_str()) {
        v
    } else {
        DEFAULT_THEME_MODE.to_string()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            restore_enabled: false,
            countdown_sec: 60,
            plan_path: String::new(),
            auto_restart: true,
            copy_tools: false,
            deploy_unattend: false,
            disable_defender: false,
            disable_firewall: false,
            tools_source_path: String::new(),
            restart_countdown_sec: 30,
            theme_mode: DEFAULT_THEME_MODE.to_string(),
            run_after: DEFAULT_RUN_AFTER.to_string(),
            run_after_enabled: false,
        }
    }
}

impl AppConfig {
    /// 配置文件路径（exe 同目录）
    pub fn config_path(path: Option<&Path>) -> PathBuf {
        path.map(|p| p.to_path_buf())
            .unwrap_or_else(|| base_dir().join("config.ini"))
    }

    /// 加载配置（文件不存在时返回默认值）
    pub fn load(path: Option<&Path>) -> Self {
        let cfg_path = Self::config_path(path);
        let mut cfg = Self::default();

        if cfg_path.exists() {
            match Self::read_file(&cfg_path, &mut cfg) {
                Ok(_) => {
                    info!("配置已加载: {:?}", cfg_path);
                }
                Err(e) => {
                    warn!("配置读取失败，使用默认值: {}", e);
                }
            }
        } else {
            info!("配置文件不存在，使用默认值: {:?}", cfg_path);
        }

        // 确保保存默认配置
        if let Err(e) = cfg.save(path) {
            warn!("配置保存失败: {}", e);
        }

        cfg
    }

    /// 读取配置文件
    fn read_file(path: &Path, cfg: &mut Self) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("读取文件失败: {}", e))?;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }

            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() != 2 {
                continue;
            }

            let key = parts[0].trim();
            let value = parts[1].trim();

            match key {
                "restore_enabled" => cfg.restore_enabled = value.parse().unwrap_or(false),
                "countdown_sec" => cfg.countdown_sec = value.parse().unwrap_or(60),
                "plan_path" => cfg.plan_path = value.to_string(),
                "auto_restart" => cfg.auto_restart = value.parse().unwrap_or(true),
                "copy_tools" => cfg.copy_tools = value.parse().unwrap_or(false),
                "deploy_unattend" => cfg.deploy_unattend = value.parse().unwrap_or(false),
                "disable_defender" => cfg.disable_defender = value.parse().unwrap_or(false),
                "disable_firewall" => cfg.disable_firewall = value.parse().unwrap_or(false),
                "tools_source_path" => cfg.tools_source_path = value.to_string(),
                "restart_countdown_sec" => cfg.restart_countdown_sec = value.parse().unwrap_or(30),
                "theme_mode" => cfg.theme_mode = normalize_theme_mode(value),
                // 「进系统后执行」：值为脚本路径，缺省/留空时回退内置默认路径
                "run_after" => {
                    cfg.run_after = if value.is_empty() {
                        DEFAULT_RUN_AFTER.to_string()
                    } else {
                        value.to_string()
                    }
                }
                "run_after_enabled" => cfg.run_after_enabled = value.parse().unwrap_or(false),
                _ => {}
            }
        }

        Ok(())
    }

    /// 保存配置
    pub fn save(&self, path: Option<&Path>) -> Result<(), String> {
        let cfg_path = Self::config_path(path);

        let content = format!(
            "[settings]\n\
            restore_enabled = {}\n\
            countdown_sec = {}\n\
            plan_path = {}\n\
            auto_restart = {}\n\
            copy_tools = {}\n\
            deploy_unattend = {}\n\
            disable_defender = {}\n\
            disable_firewall = {}\n\
            tools_source_path = {}\n\
            restart_countdown_sec = {}\n\
            theme_mode = {}\n\
            run_after = {}\n\
            run_after_enabled = {}\n\n",
            self.restore_enabled,
            self.countdown_sec,
            self.plan_path,
            self.auto_restart,
            self.copy_tools,
            self.deploy_unattend,
            self.disable_defender,
            self.disable_firewall,
            self.tools_source_path,
            self.restart_countdown_sec,
            // 写盘前归一：非法值不落盘，文件下次加载即恢复 auto
            normalize_theme_mode(&self.theme_mode),
            self.run_after,
            self.run_after_enabled
        );

        fs::write(&cfg_path, content)
            .map_err(|e| format!("写入文件失败: {}", e))?;

        info!("配置已保存: {:?}", cfg_path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cfg_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sks_cfg_test_{}_{}.ini", tag, std::process::id()))
    }

    /// `run_after` 默认值 = 桌面 TestTools\run_after.bat，勾选状态默认关闭
    #[test]
    fn run_after_defaults() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.run_after, DEFAULT_RUN_AFTER);
        assert_eq!(cfg.run_after, r"%USERPROFILE%\Desktop\TestTools\run_after.bat");
        assert!(!cfg.run_after_enabled);
    }

    /// 保存后重新加载：run_after 路径与勾选状态保持不变
    #[test]
    fn run_after_roundtrip() {
        let path = temp_cfg_path("roundtrip");
        let mut cfg = AppConfig::default();
        cfg.run_after = r"D:\tools\我的脚本.bat".to_string();
        cfg.run_after_enabled = true;
        cfg.save(Some(&path)).expect("保存应成功");

        let loaded = AppConfig::load(Some(&path));
        assert_eq!(loaded.run_after, r"D:\tools\我的脚本.bat");
        assert!(loaded.run_after_enabled);
        let _ = fs::remove_file(&path);
    }

    /// run_after 留空时回退默认路径（避免把空命令写进 unattend.xml）
    #[test]
    fn run_after_empty_falls_back() {
        let path = temp_cfg_path("empty");
        fs::write(&path, "[settings]\nrun_after = \nrun_after_enabled = true\n").unwrap();
        let loaded = AppConfig::load(Some(&path));
        assert_eq!(loaded.run_after, DEFAULT_RUN_AFTER);
        assert!(loaded.run_after_enabled);
        let _ = fs::remove_file(&path);
    }

    /// 初始化（首次生成配置）时主题必须写 auto
    #[test]
    fn theme_init_writes_auto() {
        let path = temp_cfg_path("theme_init");
        let _ = fs::remove_file(&path);

        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.theme_mode, DEFAULT_THEME_MODE);
        assert_eq!(cfg.theme_mode, "auto");

        let content = fs::read_to_string(&path).expect("初始化应生成配置文件");
        assert!(
            content.contains("theme_mode = auto"),
            "初始化生成的 config.ini 应包含 theme_mode = auto，实际:\n{content}"
        );
        let _ = fs::remove_file(&path);
    }

    /// 空值 / 非法值回退 auto；合法值（含大小写不规范）归一后保留
    #[test]
    fn theme_mode_normalized() {
        let path = temp_cfg_path("theme_norm");

        for bad in ["", "  ", "DarkMode", "system", "0"] {
            fs::write(&path, format!("[settings]\ntheme_mode = {bad}\n")).unwrap();
            let cfg = AppConfig::load(Some(&path));
            assert_eq!(cfg.theme_mode, "auto", "非法值 {bad:?} 应回退 auto");
            // load 会回写，文件应自我修复
            let content = fs::read_to_string(&path).unwrap();
            assert!(content.contains("theme_mode = auto"), "{bad:?} 未修复: {content}");
        }

        fs::write(&path, "[settings]\ntheme_mode = DARK\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.theme_mode, "dark");

        fs::write(&path, "[settings]\ntheme_mode = Light\n").unwrap();
        let cfg = AppConfig::load(Some(&path));
        assert_eq!(cfg.theme_mode, "light");

        let _ = fs::remove_file(&path);
    }
}
