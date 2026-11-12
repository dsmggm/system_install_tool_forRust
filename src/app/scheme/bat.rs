//! 方案 BAT 文件生成与解析（← `bat_builder.py`，文档 §5.2）
//!
//! 生成顺序：`@echo off` → `chcp 65001` → 元数据块 → title → diskpart 段
//! → 盘符检测段 → 安装段 → 结尾；整体 CRLF、UTF-8 无 BOM。
//!
//! **已知问题 §9.1 修复**：`gho_mode` 判断改为 `is_pload = mode ∈ {pload, partition}`
//! （GUI 实际写 `disk`/`partition`，Python 生成器仅判断 `== "pload"`
//! 导致"分区还原"方案生成 BAT 后实为硬盘还原）。

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

/// 元数据起止标记
const META_BEGIN: &str = ":: ===== SKS PLAN BEGIN =====";
const META_END: &str = ":: ===== SKS PLAN END =====";

/// 单块数据分区（← `data_parts` 元素 `{size_mb, fs}`）
#[derive(Debug, Clone, PartialEq)]
pub struct DataPart {
    /// None = 使用剩余空间（元数据写 0）
    pub size_mb: Option<i64>,
    pub fs: String,
}

impl Default for DataPart {
    fn default() -> Self {
        Self {
            size_mb: None,
            fs: "ntfs".to_string(),
        }
    }
}

/// 单块硬盘分区方案（← `partitions[i]`）
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionPlan {
    pub disk_physical: i64,
    pub style: String,
    pub esp_enabled: bool,
    pub esp_size_mb: i64,
    pub data_parts: Vec<DataPart>,
}

impl Default for PartitionPlan {
    fn default() -> Self {
        Self {
            disk_physical: 0,
            style: "gpt".to_string(),
            esp_enabled: false,
            esp_size_mb: 300,
            data_parts: Vec::new(),
        }
    }
}

/// 安装设置（← `install` 字典）
#[derive(Debug, Clone, PartialEq)]
pub struct InstallSettings {
    pub image_path: String,
    pub image_index_text: String,
    pub format: bool,
    pub auto_repair_boot: bool,
    pub copy_tools: bool,
    pub deploy_unattend: bool,
    pub disable_defender: bool,
    /// 关闭 Windows 防火墙（写入 unattend.xml 的 `%disable_firwall%`）
    pub disable_firewall: bool,
    /// 进系统后执行脚本（写入 unattend.xml 的 `%run_after%`）
    ///
    /// 脚本路径本身存放在 config.ini 的 `run_after` 键（全局配置，不随方案走），
    /// 方案里只记录"是否执行"。
    pub run_after: bool,
    pub admin_username: String,
    pub restart_after_install: bool,
    pub target_disk: String,
    pub boot_partition: String,
    pub windows_partition: String,
    pub image_type: String,
    pub gho_mode: String,
    pub ghost_path: String,
    pub gho_disk_index: String,
    pub gho_partition_index: String,
    /// GHO 分区还原后是否修复引导（默认 false：引导修复是可选操作）
    pub gho_repair_boot: bool,
}

impl Default for InstallSettings {
    fn default() -> Self {
        Self {
            image_path: String::new(),
            image_index_text: String::new(),
            format: false,
            auto_repair_boot: true,
            copy_tools: false,
            deploy_unattend: false,
            disable_defender: false,
            disable_firewall: false,
            run_after: false,
            admin_username: "admin".to_string(),
            restart_after_install: false,
            target_disk: String::new(),
            boot_partition: String::new(),
            windows_partition: String::new(),
            image_type: "wim".to_string(),
            gho_mode: "load".to_string(),
            ghost_path: r"%ProgramFiles(x86)%\ghost\ghost64.exe".to_string(),
            gho_disk_index: "0".to_string(),
            gho_partition_index: "0".to_string(),
            gho_repair_boot: true,
        }
    }
}

/// 完整方案数据（← `_gather_scheme_data` 返回结构）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchemeData {
    pub partitions: Vec<PartitionPlan>,
    pub install: InstallSettings,
}

/// 转义元数据值，防 BAT 注入（← `_safe_value`：移除 `" & | ^` 并 strip）
pub fn safe_value(value: &str) -> String {
    value
        .replace(['"', '&', '|', '^'], "")
        .trim()
        .to_string()
}

fn bool01(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

/// 生成方案元数据注释块（← `build_plan_metadata`，含旧版兼容字段）
pub fn build_plan_metadata(scheme: &SchemeData) -> String {
    let mut lines: Vec<String> = vec![META_BEGIN.to_string()];

    // ---- 分区方案（多硬盘） ----
    lines.push(format!(":: disk_count={}", scheme.partitions.len()));
    for (d_idx, p) in scheme.partitions.iter().enumerate() {
        let prefix = format!("disk_{}", d_idx + 1);
        lines.push(format!(":: {prefix}_physical={}", p.disk_physical));
        lines.push(format!(":: {prefix}_style={}", p.style));
        lines.push(format!(":: {prefix}_esp_enabled={}", bool01(p.esp_enabled)));
        lines.push(format!(":: {prefix}_esp_size_mb={}", p.esp_size_mb));
        lines.push(format!(":: {prefix}_part_count={}", p.data_parts.len()));
        for (i, dp) in p.data_parts.iter().enumerate() {
            let size_mb = dp.size_mb.unwrap_or(0);
            lines.push(format!(":: {prefix}_part_size_{}={size_mb}", i + 1));
            lines.push(format!(":: {prefix}_part_fs_{}={}", i + 1, dp.fs));
        }
    }

    // ---- 旧版兼容：单硬盘字段 ----
    if let Some(p0) = scheme.partitions.first() {
        lines.push(format!(":: partition_style={}", p0.style));
        lines.push(format!(":: disk_physical={}", p0.disk_physical));
        lines.push(format!(":: esp_enabled={}", bool01(p0.esp_enabled)));
        lines.push(format!(":: esp_size_mb={}", p0.esp_size_mb));
        lines.push(format!(":: part_count={}", p0.data_parts.len()));
        for (i, dp) in p0.data_parts.iter().enumerate() {
            let size_mb = dp.size_mb.unwrap_or(0);
            lines.push(format!(":: part_size_{}={size_mb}", i + 1));
            lines.push(format!(":: part_fs_{}={}", i + 1, dp.fs));
        }
    }

    // ---- 安装设置 ----
    let inst = &scheme.install;
    lines.push(format!(":: image_path={}", safe_value(&inst.image_path)));
    lines.push(format!(
        ":: image_index_text={}",
        safe_value(&inst.image_index_text)
    ));
    lines.push(format!(":: format={}", bool01(inst.format)));
    lines.push(format!(":: auto_repair_boot={}", bool01(inst.auto_repair_boot)));
    lines.push(format!(":: copy_tools={}", bool01(inst.copy_tools)));
    lines.push(format!(":: deploy_unattend={}", bool01(inst.deploy_unattend)));
    lines.push(format!(
        ":: disable_defender={}",
        bool01(inst.disable_defender)
    ));
    lines.push(format!(
        ":: disable_firewall={}",
        bool01(inst.disable_firewall)
    ));
    lines.push(format!(":: run_after={}", bool01(inst.run_after)));
    lines.push(format!(
        ":: admin_username={}",
        safe_value(&inst.admin_username)
    ));
    lines.push(format!(
        ":: restart_after_install={}",
        bool01(inst.restart_after_install)
    ));
    lines.push(format!(":: target_disk={}", safe_value(&inst.target_disk)));
    lines.push(format!(
        ":: boot_partition={}",
        safe_value(&inst.boot_partition)
    ));
    lines.push(format!(
        ":: windows_partition={}",
        safe_value(&inst.windows_partition)
    ));

    // ---- GHO 设置 ----
    lines.push(format!(":: image_type={}", safe_value(&inst.image_type)));
    lines.push(format!(":: gho_mode={}", safe_value(&inst.gho_mode)));
    lines.push(format!(":: ghost_path={}", safe_value(&inst.ghost_path)));
    lines.push(format!(
        ":: gho_disk_index={}",
        safe_value(&inst.gho_disk_index)
    ));
    lines.push(format!(
        ":: gho_partition_index={}",
        safe_value(&inst.gho_partition_index)
    ));
    lines.push(format!(":: gho_repair_boot={}", bool01(inst.gho_repair_boot)));

    lines.push(META_END.to_string());
    lines.join("\r\n") + "\r\n"
}

/// 解析 `key=value`（← `content.partition("=")`：首个 `=` 处切分，value 不 trim）
fn parse_kv(kv: &mut Vec<(String, String)>, content: &str) {
    if let Some(idx) = content.find('=') {
        let key = content[..idx].trim().to_string();
        let value = content[idx + 1..].to_string();
        kv.push((key, value));
    }
}

/// 从 BAT 内容解析方案元数据（← `parse_plan_metadata`；无元数据块返回 None）
pub fn parse_plan_metadata(bat_text: &str) -> Option<SchemeData> {
    let normalized = bat_text.replace("\r\n", "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();

    let mut begin_idx: Option<usize> = None;
    let mut end_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        if stripped == META_BEGIN {
            begin_idx = Some(i);
        } else if stripped == META_END && begin_idx.is_some() {
            end_idx = Some(i);
            break;
        }
    }
    let (begin_idx, end_idx) = match (begin_idx, end_idx) {
        (Some(b), Some(e)) => (b, e),
        _ => {
            log::warn!("BAT 中未找到方案元数据块");
            return None;
        }
    };

    let mut kv: Vec<(String, String)> = Vec::new();
    for line in &lines[begin_idx + 1..end_idx] {
        let stripped = line.trim();
        if !stripped.starts_with(":: ") {
            continue;
        }
        parse_kv(&mut kv, &stripped[3..]);
    }

    if kv.is_empty() {
        log::warn!("方案元数据块为空");
        return None;
    }

    let get = |key: &str| -> Option<&str> {
        kv.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    let get_str = |key: &str, default: &str| -> String {
        get(key).unwrap_or(default).to_string()
    };
    let get_int = |key: &str, default: i64| -> i64 {
        get(key).and_then(|v| v.parse::<i64>().ok()).unwrap_or(default)
    };

    // ---- 单硬盘解析闭包（← `_parse_one_disk`） ----
    let parse_one_disk = |prefix: &str| -> PartitionPlan {
        let part_count = get_int(&format!("{prefix}_part_count"), 0);
        let mut data_parts: Vec<DataPart> = Vec::new();
        for i in 1..=part_count {
            let size_mb_str = get_str(&format!("{prefix}_part_size_{i}"), "0");
            let size_mb = if size_mb_str.is_empty() || size_mb_str == "0" {
                None
            } else {
                size_mb_str.parse::<i64>().ok()
            };
            let fs = get_str(&format!("{prefix}_part_fs_{i}"), "ntfs");
            data_parts.push(DataPart { size_mb, fs });
        }
        PartitionPlan {
            disk_physical: get_int(&format!("{prefix}_physical"), 0),
            style: get_str(&format!("{prefix}_style"), "gpt"),
            esp_enabled: get_str(&format!("{prefix}_esp_enabled"), "0") == "1",
            esp_size_mb: get_int(&format!("{prefix}_esp_size_mb"), 300),
            data_parts,
        }
    };

    // ---- 分区方案：优先多硬盘，否则回退单硬盘 ----
    let mut partitions: Vec<PartitionPlan> = Vec::new();
    if get("disk_count").is_some() {
        let disk_count = get_int("disk_count", 0);
        for d in 1..=disk_count {
            let p = parse_one_disk(&format!("disk_{d}"));
            if !p.data_parts.is_empty() {
                partitions.push(p);
            }
        }
    } else {
        let part_count = get_int("part_count", 0);
        let mut data_parts: Vec<DataPart> = Vec::new();
        for i in 1..=part_count {
            let size_mb_str = get_str(&format!("part_size_{i}"), "0");
            let size_mb = if size_mb_str.is_empty() || size_mb_str == "0" {
                None
            } else {
                size_mb_str.parse::<i64>().ok()
            };
            let fs = get_str(&format!("part_fs_{i}"), "ntfs");
            data_parts.push(DataPart { size_mb, fs });
        }
        if part_count > 0 {
            partitions.push(PartitionPlan {
                disk_physical: get_int("disk_physical", 0),
                style: get_str("partition_style", "gpt"),
                esp_enabled: get_str("esp_enabled", "0") == "1",
                esp_size_mb: get_int("esp_size_mb", 300),
                data_parts,
            });
        }
    }

    // ---- 安装设置 ----
    let install = InstallSettings {
        image_path: get_str("image_path", ""),
        image_index_text: get_str("image_index_text", ""),
        format: get_str("format", "0") == "1",
        auto_repair_boot: get_str("auto_repair_boot", "1") == "1",
        copy_tools: get_str("copy_tools", "0") == "1",
        deploy_unattend: get_str("deploy_unattend", "0") == "1",
        disable_defender: get_str("disable_defender", "0") == "1",
        // 旧方案文件没有此字段，默认 false
        disable_firewall: get_str("disable_firewall", "0") == "1",
        // 旧方案文件没有此字段，默认 false（不执行）
        run_after: get_str("run_after", "0") == "1",
        admin_username: get_str("admin_username", "admin"),
        restart_after_install: get_str("restart_after_install", "0") == "1",
        target_disk: get_str("target_disk", ""),
        boot_partition: get_str("boot_partition", ""),
        windows_partition: get_str("windows_partition", ""),
        image_type: get_str("image_type", "wim"),
        gho_mode: get_str("gho_mode", "load"),
        ghost_path: get_str("ghost_path", r"%ProgramFiles(x86)%\ghost\ghost64.exe"),
        gho_disk_index: get_str("gho_disk_index", "0"),
        gho_partition_index: get_str("gho_partition_index", "0"),
        // 旧方案文件没有此字段时默认 true（与 UI 初始态、InstallSettings 缺省一致：
        // 分区还原后修复引导默认勾选）。
        gho_repair_boot: get_str("gho_repair_boot", "1") == "1",
    };

    log::info!(
        "方案元数据解析成功: 硬盘数={}, 镜像='{}'",
        partitions.len(),
        install.image_path
    );
    Some(SchemeData {
        partitions,
        install,
    })
}

/// 生成分区命令的 echo 管道部分（← `build_diskpart_section`）
pub fn build_diskpart_section(partition: &PartitionPlan, append_exit: bool) -> String {
    let style = partition.style.as_str();
    let mut commands: Vec<String> = vec![
        format!("select disk {}", partition.disk_physical),
        "clean".to_string(),
        format!("convert {style}"),
    ];

    if partition.esp_enabled && style == "gpt" {
        commands.push(format!("create partition efi size={}", partition.esp_size_mb));
        commands.push("format fs=fat32 quick label=\"ESP\"".to_string());
        commands.push("assign".to_string());
    }

    if style == "gpt" {
        commands.push("create partition msr size=16".to_string());
    }

    for (part_idx, part) in partition.data_parts.iter().enumerate() {
        if let Some(size_mb) = part.size_mb {
            commands.push(format!("create partition primary size={size_mb}"));
        } else {
            commands.push("create partition primary".to_string());
        }
        commands.push(format!("format fs={} quick", part.fs));
        if style == "mbr" && part_idx == 0 {
            commands.push("active".to_string());
        }
        commands.push("assign".to_string());
    }

    if append_exit {
        commands.push("exit".to_string());
    }

    commands
        .iter()
        .map(|c| format!("echo {c}"))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 盘符检测 BAT 代码段（← `_build_diskpart_letter_detection`）
fn build_diskpart_letter_detection(has_esp: bool) -> Vec<String> {
    let mut lines: Vec<String> = vec![
        "setlocal enabledelayedexpansion".to_string(),
        "set \"SYS_LETTER=\"".to_string(),
        "set \"ESP_LETTER=\"".to_string(),
        "(echo list volume) | diskpart | findstr /i \"NTFS\" > %temp%\\sks_dp.txt 2>nul".to_string(),
        "for /f \"usebackq tokens=*\" %%a in (\"%temp%\\sks_dp.txt\") do (".to_string(),
        "    if not defined SYS_LETTER (".to_string(),
        "        set \"L=%%a\"".to_string(),
        "        set \"DL=!L:~13,1!\"".to_string(),
        "        if not \"!DL!\"==\"\" if not \"!DL!\"==\" \" set \"SYS_LETTER=!DL!\"".to_string(),
        "    )".to_string(),
        ")".to_string(),
        "del \"%temp%\\sks_dp.txt\" 2>nul".to_string(),
    ];

    if has_esp {
        lines.push(
            "(echo list volume) | diskpart | findstr /i \"FAT32\" > %temp%\\sks_dp2.txt 2>nul"
                .to_string(),
        );
        lines.push("for /f \"usebackq tokens=*\" %%a in (\"%temp%\\sks_dp2.txt\") do (".to_string());
        lines.push("    if not defined ESP_LETTER (".to_string());
        lines.push("        set \"L=%%a\"".to_string());
        lines.push("        set \"DL=!L:~13,1!\"".to_string());
        lines.push(
            "        if not \"!DL!\"==\"\" if not \"!DL!\"==\" \" set \"ESP_LETTER=!DL!\"".to_string(),
        );
        lines.push("    )".to_string());
        lines.push(")".to_string());
        lines.push("del \"%temp%\\sks_dp2.txt\" 2>nul".to_string());
    }

    lines
}

/// 盘符检测完整段（← `build_letter_detection_section`）
pub fn build_letter_detection_section(has_esp: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("echo [1.5] 正在检测分区盘符...".to_string());
    lines.extend(build_diskpart_letter_detection(has_esp));

    lines.push("if not defined SYS_LETTER (".to_string());
    lines.push("    echo [错误] 未找到 NTFS 系统分区".to_string());
    lines.push("    pause".to_string());
    lines.push("    exit /b 1".to_string());
    lines.push(")".to_string());

    if has_esp {
        lines.push("if not defined ESP_LETTER (".to_string());
        lines.push("    echo [警告] 未找到 FAT32 ESP 分区，引导修复可能失败".to_string());
        lines.push(")".to_string());
    } else {
        lines.push("if not defined ESP_LETTER set \"ESP_LETTER=%SYS_LETTER%\"".to_string());
    }

    lines.push("echo 引导分区: %ESP_LETTER%".to_string());
    lines.push("echo 系统分区: %SYS_LETTER%".to_string());
    lines.push("echo.".to_string());
    lines.join("\r\n")
}

/// 解析镜像索引文本首段（← `int(idx_text.split(":")[0])`；失败回退 1）
fn parse_image_index(idx_text: &str) -> i64 {
    match idx_text.split(':').next() {
        Some(first) => first.trim().parse::<i64>().unwrap_or_else(|_| {
            log::warn!("镜像索引解析失败: '{idx_text}', 回退为 1");
            1
        }),
        None => 1,
    }
}

/// 规范化镜像路径（← `image_path.replace('"',"").replace("/","\\")`）
fn normalize_image_path(path: &str) -> String {
    path.replace('"', "").replace('/', "\\")
}

/// 把镜像路径拆成（目录, 文件名）。
///
/// 目录末尾带反斜杠（如 `W:\dir\`）；路径不含盘符/目录时目录为空串、
/// 文件名即原样。用于 GHO 还原 BAT：先 `cd /d "<dir>"` 再用**纯文件名**
/// 调用 Ghost——PE 环境下用户选的盘符（如 W:）可能不存在，用绝对路径
/// 会直接失败；cd 进目录后只写文件名，Ghost 从当前目录解析。
fn split_image_path(path: &str) -> (String, String) {
    let p = normalize_image_path(path);
    // 去掉首尾引号（normalize 已经去掉双引号，这里再保险一次）
    let p = p.trim_matches('"');
    let trimmed = p.trim();
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }
    match trimmed.rfind('\\') {
        Some(idx) => {
            let dir = &trimmed[..=idx];
            let name = &trimmed[idx + 1..];
            (dir.to_string(), name.to_string())
        }
        None => (String::new(), trimmed.to_string()),
    }
}

/// 系统安装命令段（← `build_install_section`）
pub fn build_install_section(inst: &InstallSettings) -> String {
    let image_path = normalize_image_path(&inst.image_path);
    let image_index = parse_image_index(&inst.image_index_text);

    let mut lines: Vec<String> = Vec::new();

    if inst.format {
        lines.push("echo [2a] 正在格式化 Windows 分区...".to_string());
        lines.push("echo select volume=%SYS_LETTER% > \"%TEMP%\\fmt.txt\"".to_string());
        lines.push("echo format quick fs=ntfs >> \"%TEMP%\\fmt.txt\"".to_string());
        lines.push("diskpart /s \"%TEMP%\\fmt.txt\"".to_string());
        lines.push("del \"%TEMP%\\fmt.txt\"".to_string());
        lines.push("if errorlevel 1 (".to_string());
        lines.push("    echo [错误] 格式化失败".to_string());
        lines.push("    pause".to_string());
        lines.push("    exit /b 1".to_string());
        lines.push(")".to_string());
        lines.push(String::new());
    }

    lines.push("echo [2b] 正在应用 WIM/ESD 镜像...".to_string());
    lines.push(format!(
        "dism /Apply-Image /ImageFile:\"{image_path}\" /Index:{image_index} /ApplyDir=%SYS_LETTER%:\\"
    ));
    lines.push("if errorlevel 1 (".to_string());
    lines.push("    echo [错误] 镜像应用失败".to_string());
    lines.push("    pause".to_string());
    lines.push("    exit /b 1".to_string());
    lines.push(")".to_string());
    lines.push(String::new());

    if inst.auto_repair_boot {
        lines.push("echo [3] 正在修复系统引导...".to_string());
        lines.push("if defined ESP_LETTER (".to_string());
        lines.push("    echo 引导分区=%ESP_LETTER% 系统分区=%SYS_LETTER%".to_string());
        lines.push("    bootsect /nt60 %ESP_LETTER%: /force".to_string());
        lines.push("    bcdboot %SYS_LETTER%:\\Windows /s %ESP_LETTER%: /f all".to_string());
        lines.push(") else (".to_string());
        lines.push("    echo [警告] 无 ESP 分区，跳过引导修复".to_string());
        lines.push(")".to_string());
        lines.push("if errorlevel 1 (".to_string());
        lines.push("    echo [错误] 引导修复失败".to_string());
        lines.push("    pause".to_string());
        lines.push("    exit /b 1".to_string());
        lines.push(")".to_string());
        lines.push(String::new());
    }

    lines.push("echo.".to_string());
    lines.push("echo [完成] 系统安装成功".to_string());
    if inst.restart_after_install {
        lines.push("echo 即将自动重启...".to_string());
        lines.push("shutdown /r /t 3".to_string());
    } else {
        lines.push("echo 请手动重启计算机。".to_string());
        lines.push("pause".to_string());
    }

    lines.join("\r\n")
}

/// GHO 模式是否为分区还原（← §9.1 修复：`mode ∈ {pload, partition}`）
pub fn gho_is_pload(mode: &str) -> bool {
    matches!(mode.to_lowercase().as_str(), "pload" | "partition")
}

/// GHO 还原命令段（← `build_gho_install_section`）
///
/// 【GHO 路径处理】PE 环境下用户选的镜像盘符（如 W:）可能不存在，
/// 直接用绝对路径调 Ghost 会失败。做法：把路径拆成「目录 + 文件名」，
/// 先 `cd /d "<目录>"` 再用**纯文件名**调 Ghost——Ghost 从当前目录解析，
/// 盘符无关。Ghost 命令里只写文件名，不写绝对路径。
pub fn build_gho_install_section(inst: &InstallSettings) -> String {
    let (image_dir, image_name) = split_image_path(&inst.image_path);
    let ghost_path = &inst.ghost_path;
    let is_pload = gho_is_pload(&inst.gho_mode);

    // Ghost 磁盘编号 = physical_disk + 1
    let ghost_dst = inst
        .gho_disk_index
        .parse::<i64>()
        .map(|d| d + 1)
        .unwrap_or(1);
    let ghost_part = inst
        .gho_partition_index
        .parse::<i64>()
        .map(|p| p + 1)
        .unwrap_or(1);

    let clone = if is_pload {
        format!("-clone,mode=pload,src=\"{image_name}:1\",dst={ghost_dst}:{ghost_part}")
    } else {
        format!("-clone,mode=load,src=\"{image_name}\",dst={ghost_dst}")
    };

    // 单分区模式后执行 bcdboot，不加重启参数；全盘模式按用户选择
    let reboot_flag = if inst.restart_after_install && !is_pload {
        "-rb"
    } else {
        ""
    };

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("echo [1] 正在还原 GHO 镜像到磁盘 {ghost_dst}..."));
    // 先 cd 进镜像目录（PE 下盘符可能无效，cd 后只用文件名）
    if !image_dir.is_empty() {
        lines.push(format!("cd /d \"{image_dir}\""));
    }
    let mut ghost_cmd = format!("\"{ghost_path}\" {clone} -sure -fx");
    if !reboot_flag.is_empty() {
        ghost_cmd.push_str(&format!(" {reboot_flag}"));
    }
    lines.push(ghost_cmd);
    lines.push("if errorlevel 1 (".to_string());
    lines.push("    echo [错误] GHO 还原失败".to_string());
    lines.push("    pause".to_string());
    lines.push("    exit /b 1".to_string());
    lines.push(")".to_string());
    lines.push(String::new());

    if is_pload && inst.gho_repair_boot {
        lines.push("echo [2] 正在修复系统引导...".to_string());
        lines.extend(build_diskpart_letter_detection(true));
        lines.push("if defined ESP_LETTER (".to_string());
        lines.push("    echo 引导分区=%ESP_LETTER% 系统分区=%SYS_LETTER%".to_string());
        lines.push("    bootsect /nt60 %ESP_LETTER%: /force".to_string());
        lines.push("    bcdboot %SYS_LETTER%:\\Windows /s %ESP_LETTER%: /f all".to_string());
        lines.push(") else (".to_string());
        lines.push("    echo [警告] 无 ESP 分区，跳过引导修复".to_string());
        lines.push(")".to_string());
        lines.push("if errorlevel 1 (".to_string());
        lines.push("    echo [错误] 引导修复失败".to_string());
        lines.push("    pause".to_string());
        lines.push("    exit /b 1".to_string());
        lines.push(")".to_string());
        lines.push(String::new());
    }

    lines.push("echo.".to_string());
    lines.push("echo [完成] GHO 还原成功".to_string());
    if inst.restart_after_install && !is_pload {
        lines.push("echo 即将自动重启...".to_string());
        lines.push("shutdown /r /t 3".to_string());
    } else {
        lines.push("echo 请手动重启计算机。".to_string());
        lines.push("pause".to_string());
    }

    lines.join("\r\n")
}

/// 生成完整方案 BAT（← `build_combined_bat`）
pub fn build_combined_bat(scheme: &SchemeData) -> String {
    let title = "正在还原系统";
    let is_gho = scheme.install.image_type.to_lowercase() == "gho";

    log::info!(
        "生成方案 BAT: 含分区={}, 硬盘数={}, 模式={}",
        if scheme.partitions.is_empty() { "否" } else { "是" },
        scheme.partitions.len(),
        if is_gho { "GHO" } else { "WIM" }
    );

    let meta_block = build_plan_metadata(scheme);

    let mut parts: Vec<String> = vec![
        "@echo off".to_string(),
        "chcp 65001 >nul".to_string(),
        meta_block,
        format!("title {title}"),
        "echo ========================================".to_string(),
        format!("echo   {title}"),
        "echo ========================================".to_string(),
        "echo.".to_string(),
    ];

    if !scheme.partitions.is_empty() {
        parts.push("echo [1] 正在执行规划分区...".to_string());
        parts.push("(".to_string());
        let last = scheme.partitions.len() - 1;
        for (i, partition) in scheme.partitions.iter().enumerate() {
            parts.push(build_diskpart_section(partition, i == last));
        }
        parts.push(") | diskpart".to_string());
        parts.push("if errorlevel 1 (".to_string());
        parts.push("    echo [错误] 分区失败".to_string());
        parts.push("    pause".to_string());
        parts.push("    exit /b 1".to_string());
        parts.push(")".to_string());
        parts.push("echo [完成] 分区成功".to_string());
        parts.push("echo.".to_string());

        let first_part = &scheme.partitions[0];
        let has_esp = first_part.esp_enabled && first_part.style == "gpt";
        if !is_gho {
            parts.push(build_letter_detection_section(has_esp));
        }
        parts.push(format!(
            "echo [2] 开始{}...",
            if is_gho { "GHO 还原" } else { "安装系统" }
        ));
        if is_gho {
            parts.push(build_gho_install_section(&scheme.install));
        } else {
            parts.push(build_install_section(&scheme.install));
        }
    } else {
        if !is_gho {
            parts.push(build_letter_detection_section(false));
        }
        parts.push(format!(
            "echo [1] 开始{}...",
            if is_gho { "GHO 还原" } else { "安装系统" }
        ));
        if is_gho {
            parts.push(build_gho_install_section(&scheme.install));
        } else {
            parts.push(build_install_section(&scheme.install));
        }
    }

    parts.join("\r\n") + "\r\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_scheme() -> SchemeData {
        let install = InstallSettings {
            image_path: r"D:\iso\sources\install.wim".to_string(),
            image_index_text: "2: Windows 11 Pro".to_string(),
            format: true,
            copy_tools: true,
            deploy_unattend: true,
            disable_defender: true,
            run_after: true,
            admin_username: "user1".to_string(),
            restart_after_install: true,
            target_disk: "0".to_string(),
            boot_partition: "0:1".to_string(),
            windows_partition: "0:2".to_string(),
            ..Default::default()
        };
        SchemeData {
            partitions: vec![PartitionPlan {
                disk_physical: 0,
                style: "gpt".to_string(),
                esp_enabled: true,
                esp_size_mb: 300,
                data_parts: vec![DataPart {
                    size_mb: None,
                    fs: "ntfs".to_string(),
                }],
            }],
            install,
        }
    }

    /// ← 元数据布局：BEGIN/END 标记 + 关键行 + CRLF 结尾
    #[test]
    fn metadata_layout() {
        let meta = build_plan_metadata(&sample_scheme());
        assert!(meta.starts_with(":: ===== SKS PLAN BEGIN =====\r\n"));
        assert!(meta.ends_with(":: ===== SKS PLAN END =====\r\n"));
        assert!(meta.contains(":: disk_count=1\r\n"));
        assert!(meta.contains(":: disk_1_physical=0\r\n"));
        assert!(meta.contains(":: disk_1_style=gpt\r\n"));
        assert!(meta.contains(":: disk_1_esp_enabled=1\r\n"));
        assert!(meta.contains(":: disk_1_part_count=1\r\n"));
        assert!(meta.contains(":: disk_1_part_size_1=0\r\n"));
        assert!(meta.contains(":: disk_1_part_fs_1=ntfs\r\n"));
        // 旧版兼容字段
        assert!(meta.contains(":: partition_style=gpt\r\n"));
        assert!(meta.contains(":: part_count=1\r\n"));
        assert!(meta.contains(":: image_type=wim\r\n"));
        assert!(meta.contains(":: admin_username=user1\r\n"));
        // 进系统后执行（新增字段）
        assert!(meta.contains(":: run_after=1\r\n"));
    }

    /// ← `_safe_value`：移除 `" & | ^` 并 strip
    #[test]
    fn safe_value_rules() {
        assert_eq!(safe_value("a\"b&c|d^e"), "abcde");
        assert_eq!(safe_value("  spaced  "), "spaced");
        assert_eq!(safe_value("C:\\path\\x.wim"), "C:\\path\\x.wim");
    }

    /// ← 生成→解析往返一致（多硬盘亦成立）
    #[test]
    fn metadata_roundtrip() {
        let mut scheme = sample_scheme();
        scheme.partitions.push(PartitionPlan {
            disk_physical: 1,
            style: "mbr".to_string(),
            esp_enabled: false,
            esp_size_mb: 300,
            data_parts: vec![
                DataPart {
                    size_mb: Some(50000),
                    fs: "ntfs".to_string(),
                },
                DataPart {
                    size_mb: None,
                    fs: "exfat".to_string(),
                },
            ],
        });
        let bat = build_combined_bat(&scheme);
        let parsed = parse_plan_metadata(&bat).expect("应解析出元数据");
        assert_eq!(parsed, scheme);
    }

    /// 旧版单硬盘字段（无 disk_count）回退解析
    #[test]
    fn legacy_single_disk_parse() {
        let bat = "@echo off\n\
            :: ===== SKS PLAN BEGIN =====\n\
            :: partition_style=mbr\n\
            :: disk_physical=2\n\
            :: esp_enabled=0\n\
            :: part_count=1\n\
            :: part_size_1=0\n\
            :: part_fs_1=ntfs\n\
            :: image_path=C:\\x.wim\n\
            :: ===== SKS PLAN END =====\n";
        let parsed = parse_plan_metadata(bat).expect("旧版格式应可解析");
        assert_eq!(parsed.partitions.len(), 1);
        assert_eq!(parsed.partitions[0].style, "mbr");
        assert_eq!(parsed.partitions[0].disk_physical, 2);
        assert_eq!(parsed.partitions[0].data_parts[0].size_mb, None);
        assert_eq!(parsed.install.image_path, "C:\\x.wim");
        // auto_repair_boot 缺省为真（与 Python fallback 一致）
        assert!(parsed.install.auto_repair_boot);
        // gho_repair_boot 缺省为真（分区还原后修复引导默认勾选）
        assert!(parsed.install.gho_repair_boot);
        // 旧方案无 run_after 字段 → 默认不执行
        assert!(!parsed.install.run_after);
    }

    /// 无元数据块返回 None
    #[test]
    fn missing_metadata_returns_none() {
        assert!(parse_plan_metadata("@echo off\r\necho hi\r\n").is_none());
        assert!(parse_plan_metadata(":: ===== SKS PLAN BEGIN =====\r\n").is_none());
    }

    /// ← diskpart 段：GPT+ESP、MSR、最后一段 exit
    #[test]
    fn diskpart_section_gpt() {
        let p = PartitionPlan {
            disk_physical: 0,
            style: "gpt".to_string(),
            esp_enabled: true,
            esp_size_mb: 300,
            data_parts: vec![DataPart {
                size_mb: None,
                fs: "ntfs".to_string(),
            }],
        };
        let section = build_diskpart_section(&p, true);
        assert!(section.starts_with("echo select disk 0\r\necho clean\r\necho convert gpt"));
        assert!(section.contains("echo create partition efi size=300"));
        assert!(section.contains("echo format fs=fat32 quick label=\"ESP\""));
        assert!(section.contains("echo create partition msr size=16"));
        assert!(section.ends_with("echo exit"));
        // 无 append_exit 时不带 exit
        assert!(!build_diskpart_section(&p, false).contains("echo exit"));
    }

    /// ← diskpart 段：MBR 首数据分区 active，第二分区无 active
    #[test]
    fn diskpart_section_mbr_active() {
        let p = PartitionPlan {
            disk_physical: 1,
            style: "mbr".to_string(),
            esp_enabled: true,
            esp_size_mb: 300,
            data_parts: vec![
                DataPart {
                    size_mb: Some(60000),
                    fs: "ntfs".to_string(),
                },
                DataPart {
                    size_mb: None,
                    fs: "ntfs".to_string(),
                },
            ],
        };
        let section = build_diskpart_section(&p, false);
        // MBR + esp_enabled：不生成 efi/msr
        assert!(!section.contains("create partition efi"));
        assert!(!section.contains("create partition msr"));
        assert!(section.contains("echo create partition primary size=60000"));
        // 第一个数据分区 active
        assert!(section.contains("echo active"));
        assert_eq!(section.matches("echo active").count(), 1);
        assert!(section.contains("echo create partition primary\r\necho format fs=ntfs quick"));
    }

    /// ← 盘符检测段：WIM 模式必需结构
    #[test]
    fn letter_detection_shapes() {
        let with_esp = build_letter_detection_section(true);
        assert!(with_esp.starts_with("echo [1.5] 正在检测分区盘符..."));
        assert!(with_esp.contains("setlocal enabledelayedexpansion"));
        assert!(with_esp.contains("findstr /i \"NTFS\""));
        assert!(with_esp.contains("findstr /i \"FAT32\""));
        assert!(with_esp.contains("set \"DL=!L:~13,1!\""));
        assert!(with_esp.contains("if not defined ESP_LETTER ("));

        let no_esp = build_letter_detection_section(false);
        assert!(!no_esp.contains("findstr /i \"FAT32\""));
        assert!(no_esp.contains("if not defined ESP_LETTER set \"ESP_LETTER=%SYS_LETTER%\""));
    }

    /// ← 安装段：格式化块 + dism + bcdboot + 重启
    #[test]
    fn install_section_shapes() {
        let inst = sample_scheme().install;
        let section = build_install_section(&inst);
        assert!(section.contains("echo [2a] 正在格式化 Windows 分区..."));
        assert!(section.contains("echo select volume=%SYS_LETTER% > \"%TEMP%\\fmt.txt\""));
        assert!(section.contains(
            "dism /Apply-Image /ImageFile:\"D:\\iso\\sources\\install.wim\" /Index:2 /ApplyDir=%SYS_LETTER%:\\"
        ));
        assert!(section.contains("bootsect /nt60 %ESP_LETTER%: /force"));
        assert!(section.contains("bcdboot %SYS_LETTER%:\\Windows /s %ESP_LETTER%: /f all"));
        assert!(section.contains("shutdown /r /t 3"));
    }

    /// ← 镜像索引解析：`"2: name"` → 2；非法 → 1
    #[test]
    fn image_index_parse() {
        assert_eq!(parse_image_index("2: Windows 11 Pro"), 2);
        assert_eq!(parse_image_index("1:"), 1);
        assert_eq!(parse_image_index(""), 1);
        assert_eq!(parse_image_index("abc"), 1);
        assert_eq!(parse_image_index("  3 : x"), 3);
    }

    /// ← GHO 全盘模式：`mode=load`，无 bcdboot，按需 -rb
    #[test]
    fn gho_load_shapes() {
        let inst = InstallSettings {
            image_type: "gho".to_string(),
            gho_mode: "disk".to_string(),
            image_path: r"D:\backup\sys.gho".to_string(),
            gho_disk_index: "1".to_string(),
            gho_partition_index: "0".to_string(),
            restart_after_install: true,
            ..Default::default()
        };

        let section = build_gho_install_section(&inst);
        // disk 模式：ghost_dst = 1 + 1 = 2
        assert!(section.contains("echo [1] 正在还原 GHO 镜像到磁盘 2..."));
        // 路径拆成目录+文件名，cd 进目录后再调 Ghost（只用纯文件名）
        assert!(section.contains("cd /d \"D:\\backup\\\""));
        assert!(section.contains(
            "\"%ProgramFiles(x86)%\\ghost\\ghost64.exe\" -clone,mode=load,src=\"sys.gho\",dst=2 -sure -fx -rb"
        ));
        assert!(!section.contains("bcdboot"));
        assert!(section.contains("shutdown /r /t 3"));
    }

    /// ← §9.1 修复：`partition` 亦识别为分区还原（pload 语义）
    #[test]
    fn gho_pload_semantics_fix() {
        assert!(gho_is_pload("pload"));
        assert!(gho_is_pload("partition"));
        assert!(gho_is_pload("Partition"));
        assert!(!gho_is_pload("load"));
        assert!(!gho_is_pload("disk"));

        let inst = InstallSettings {
            image_type: "gho".to_string(),
            gho_mode: "partition".to_string(), // GUI 实际写值
            image_path: r"D:\backup\sys.gho".to_string(),
            gho_disk_index: "0".to_string(),
            gho_partition_index: "2".to_string(),
            restart_after_install: true,
            gho_repair_boot: true, // 引导修复开启时才生成 bcdboot 段
            ..Default::default()
        };

        let section = build_gho_install_section(&inst);
        // pload：src="name:1"，dst=1:3（0+1, 2+1），追加盘符检测 + bcdboot
        assert!(section.contains("cd /d \"D:\\backup\\\""));
        assert!(section.contains("-clone,mode=pload,src=\"sys.gho:1\",dst=1:3"));
        assert!(section.contains("bcdboot %SYS_LETTER%:\\Windows"));
        // pload 不追加 -rb，且不 shutdown
        assert!(!section.contains("-rb"));
        assert!(!section.contains("shutdown /r /t 3"));
        assert!(section.contains("echo 请手动重启计算机。"));

        // 引导修复关闭时不生成 bcdboot 段
        let mut inst_off = inst.clone();
        inst_off.gho_repair_boot = false;
        let section_off = build_gho_install_section(&inst_off);
        assert!(!section_off.contains("bcdboot"));
        assert!(!section_off.contains("正在修复系统引导"));
    }

    /// ← 完整 BAT 头与结尾
    #[test]
    fn combined_bat_header_footer() {
        let bat = build_combined_bat(&sample_scheme());
        assert!(bat.starts_with("@echo off\r\nchcp 65001 >nul\r\n"));
        assert!(bat.contains("title 正在还原系统"));
        assert!(bat.contains("echo [1] 正在执行规划分区..."));
        assert!(bat.contains(") | diskpart"));
        assert!(bat.contains("echo [2] 开始安装系统..."));
        assert!(bat.contains("echo [完成] 系统安装成功"));
        assert!(bat.ends_with("\r\n"));
        // UTF-8 无 BOM：首字节为 '@'
        assert_eq!(bat.as_bytes()[0], b'@');
    }

    /// ← 无分区方案：跳过 diskpart 段，直接从盘符检测 + 安装段开始
    #[test]
    fn combined_bat_no_partitions() {
        let mut scheme = sample_scheme();
        scheme.partitions.clear();
        let bat = build_combined_bat(&scheme);
        assert!(!bat.contains("echo [1] 正在执行规划分区..."));
        assert!(!bat.contains("echo clean"));
        assert!(bat.contains("echo [1.5] 正在检测分区盘符..."));
        assert!(bat.contains("echo [1] 开始安装系统..."));
    }

    /// ← GHO 模式完整 BAT：分区后不插入 WIM 盘符检测段
    #[test]
    fn combined_bat_gho_skips_letter_detection() {
        let mut scheme = sample_scheme();
        scheme.install.image_type = "gho".to_string();
        scheme.install.gho_mode = "disk".to_string();
        let bat = build_combined_bat(&scheme);
        assert!(bat.contains("echo [2] 开始GHO 还原..."));
        // 分区后应直接进入 GHO 段（无 [1.5] 检测）
        assert!(!bat.contains("[1.5] 正在检测分区盘符"));
    }
}
