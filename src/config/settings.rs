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
            theme_mode: "auto".to_string(),
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
                "theme_mode" => cfg.theme_mode = value.to_string(),
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
            theme_mode = {}\n\n",
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
            self.theme_mode
        );

        fs::write(&cfg_path, content)
            .map_err(|e| format!("写入文件失败: {}", e))?;

        info!("配置已保存: {:?}", cfg_path);
        Ok(())
    }
}