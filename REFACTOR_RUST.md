# 系统安装工具 Rust 重构文档

> 版本：1.0（2026-10-20）
> 目标读者：执行重构的 AI / 开发者
> 约束：**1:1 功能对等**（严格复刻现有全部功能与交互逻辑，不新增功能，仅修复本文档"已知问题"章节列出的问题）
> 代码位置：本文档只规定重构方案，Python 代码保持原位不动；Rust 代码位置由执行者与用户确认（建议 `rust/` 子目录或仓库根目录，Python 代码保留为参考）。

---

## 0. 一句话概述

把现有 Python/tkinter 实现的 Windows 系统安装工具（v1.0.6）用 Rust + fltk-rs 完整重写，产出**单个自包含 exe**，保持所有外部接口（config.ini、BAT 方案文件、diskpart/DISM/Ghost 调用）与现有行为 100% 兼容，可在 WinPE 环境运行。

---

## 1. 项目现状

### 1.1 功能总览

| 功能域 | 说明 |
|---|---|
| 磁盘信息 | 通过 DeviceIoControl 枚举物理磁盘（型号/容量/GPT-MBR/SSD/分区/盘符），排除 U 盘 |
| 磁盘操作 | diskpart 封装：格式化、删除/新建分区、分配盘符、一键分区、初始化 |
| 一键分区 | 对话框配置（GPT/MBR、ESP、1-8 数据分区、自动均分），**预览模式**：确认后仅暂存，点"开始安装"才实际执行 |
| WIM 安装 | ISO 挂载 → DISM apply（进度 20-85%）→ 引导修复（bootsect+bcdboot）→ 系统优化部署 |
| GHO 安装 | Ghost 还原（硬盘/分区两种模式），含中文路径 8.3 短名 workaround，分区还原后自动引导修复 |
| 方案管理 | 一键分区/安装方案保存为 `.bat`（内嵌元数据块）、加载、变更追踪失效、立即保存按钮 |
| 自动还原 | 开机读取 config.ini，按方案静默加载并倒计时（可取消）自动执行安装 |
| 系统优化 | unattend.xml 部署（用户名/Defender 占位符替换）、禁用 Defender 注册表、复制调试工具 |
| 日志 | 文件日志（按天滚动、保留 30 天、DEBUG）+ GUI 日志面板（默认 INFO，可切 DEBUG/隐藏） |

### 1.2 源码清单（行数为勘察时统计）

| 文件 | 行数 | 职责 |
|---|---|---|
| `main.py` | 148 | 入口：日志初始化、管理员检查/UAC 提权、创建 GUI |
| `gui.py` | 3847 | App 主窗口 + OneClickPartitionDialog + RestartDialog + TextHandler |
| `GetDiskInfo.py` | 848 | 磁盘枚举（DeviceIoControl 全套） |
| `disk_manager.py` | 396 | diskpart 脚本执行与操作封装 |
| `sys_install.py` | 853 | DISM 应用镜像、ISO 挂载/卸载、引导修复、WIM 信息解析 |
| `sys_optimize.py` | 528 | unattend.xml 模板与部署、Defender 禁用、优化入口 |
| `copy_tools.py` | 74 | 复制 Tools 目录到目标 `Windows\panther\TestTools` |
| `bat_builder.py` | 530 | BAT 方案生成与元数据解析 |
| `config.py` | 83 | AppConfig（config.ini）读写 |
| `config.ini` | — | 客户端运行状态 |

**重要**：执行重构时必须对照阅读上述源文件；本文档列出的行号是关键行为的权威定位（以当前 main 分支 `90fee4f` 为准，若有出入以实际代码为准）。

---

## 2. 技术选型（已确认）

| 项 | 决定 |
|---|---|
| 语言 | Rust（edition 2021+） |
| GUI | **fltk-rs**（基于 FLTK，Windows 下走 GDI 绘制，无 OpenGL/WebView2 依赖，WinPE 兼容，编译为单 exe） |
| 范围 | **1:1 功能对等** |
| 目标平台 | Windows 10/11 + WinPE（x64），**必须以管理员权限运行** |
| 产出 | 单个自包含 exe（内嵌 `icon.ico`、unattend 模板等资源），版本号从 **2.0.0** 起 |
| 外部依赖进程 | `diskpart`、`dism`、`bootsect`、`bcdboot`、`format.com`、`explorer.exe`、`shutdown`、`powershell`（仅个别辅助）、`ghost64.exe`（用户提供路径）——全部与 Python 版一致，不引入新外部工具 |

### 2.1 建议依赖（可按需调整，但需说明理由）

```toml
[dependencies]
fltk = "1.4"                # GUI（启用 fltk 的 image/内嵌 icon 能力）
windows-sys = "0.59"        # Win32 API（DeviceIoControl/ShellExecuteW 等）
# 或官方 windows crate（features 按需裁剪，注意二进制体积）
encoding_rs = "0.8"         # 如需处理 OEM/ANSI 代码页
regex = "1"                 # DISM 进度解析（对应 Python re）
chrono = "0.4"              # 日志按天滚动、时间戳
# config.ini 与 BAT 元数据建议手写解析（格式简单且必须逐字节兼容），不引入 ini 序列化库
```

日志：可用 `log` + 自实现 DailyRollingFileAppender（对应 `main.py` 的 `DailyRotatingFileHandler`），或 `tracing` + 自定义 layer。**必须保留**：文件通道 DEBUG、按天滚动、保留 30 天、UTF-8。

---

## 3. 目标架构

### 3.1 模块映射

```
rust/                         （或仓库根目录，执行时与用户确认）
├── Cargo.toml
├── build.rs                  # 嵌入 icon.ico、unattend.xml 模板
├── assets/
│   ├── icon.ico              # 复制自现有
│   └── unattend.xml          # 从 sys_optimize.py UNATTEND_TEMPLATE 抽出
└── src/
    ├── main.rs               # ← main.py：日志、管理员检查/UAC 提权、启动 GUI
    ├── config.rs             # ← config.py：AppConfig、config.ini 路径解析
    ├── logging.rs            # ← main.py 日志设施 + gui.py TextHandler
    ├── disk_info.rs          # ← GetDiskInfo.py：DeviceIoControl 磁盘枚举
    ├── diskpart.rs           # ← disk_manager.py：diskpart 脚本执行与操作
    ├── install/
    │   ├── mod.rs            # ← sys_install.py：安装流程编排（进度、阶段、取消）
    │   ├── dism.rs           #    get-imageinfo / apply-image（行级进度解析）
    │   ├── iso.rs            #    ISO 挂载/卸载/找 install.wim|esd（explorer 方案）
    │   └── boot.rs           #    bootsect/bcdboot/format.com/激活分区
    ├── optimize.rs           # ← sys_optimize.py：unattend 模板、Defender 脚本
    ├── copy_tools.rs         # ← copy_tools.py
    ├── ghost.rs              # ← gui.py GHO 相关：命令构造、8.3 短名/ASCII 临时拷贝
    ├── scheme/
    │   ├── mod.rs            # 方案数据模型（Python dict 的强类型化）
    │   ├── bat.rs            # ← bat_builder.py：生成 + 解析（元数据块兼容）
    │   └── plan.rs           # ← gui.py：预览暂存 _pending_partitions、执行期组装
    └── gui/
        ├── mod.rs            # ← gui.py App：主窗口布局、全局状态、定时器
        ├── disk_panel.rs     #    磁盘树（FLTK TreeBrowser/Browser 模拟 ttk.Treeview）
        ├── install_panel.rs  #    安装面板（源/目标/选项/进度/按钮状态机）
        ├── log_panel.rs      #    日志面板（Text + 着色 tag + 级别切换）
        ├── one_click.rs      # ← OneClickPartitionDialog
        └── restart_dialog.rs # ← RestartDialog
```

### 3.2 模块映射总表

| Python | Rust 模块 | 备注 |
|---|---|---|
| `main.py` | `main.rs` + `logging.rs` | UAC 提权用 `ShellExecuteW` verb=`runas` |
| `config.py` | `config.rs` | 手写解析，格式见 §5.1 |
| `GetDiskInfo.py` | `disk_info.rs` | 见 §6.1 |
| `disk_manager.py` | `diskpart.rs` | 见 §6.2 |
| `sys_install.py` | `install/*` | 见 §6.3 |
| `sys_optimize.py` | `optimize.rs` | unattend 模板整体迁入 assets |
| `copy_tools.py` | `copy_tools.rs` | |
| `bat_builder.py` | `scheme/bat.rs` | 见 §5.2，**格式兼容是硬要求** |
| `gui.py` | `gui/*` + `scheme/plan.rs` + `ghost.rs` | 见 §7、§8 |

### 3.3 线程模型

与 tkinter 相同：**GUI 单线程**；所有耗时操作（diskpart/DISM/Ghost/磁盘扫描）跑在后台 `std::thread`，通过**消息通道**回传主线程。

- fltk-rs 主循环内用 `fltk::app::awake()` 唤醒，channel 发送 `AppMessage` 枚举（`DiskInfoArrived`、`LogLine`、`InstallProgress`、`InstallFinished`、`TaskError` 等）。
- 取消机制对应 Python 的 `cancel_flag`（安装）、轮询回调（DISM 行读取）：用 `Arc<AtomicBool>`。
- **5 秒自动刷新**（`App::AUTO_REFRESH_INTERVAL`）：`fltk::app::add_timeout3(5.0, cb)` 循环，静默刷新（不弹任何 UI、不写 INFO 日志，磁盘枚举日志策略见 §6.1）。
- **自动还原倒计时**：1 秒周期 timeout，与 Python `_countdown_tick` 逻辑一致。

---

## 4. 外部进程调用规范

所有子进程必须满足（与 Python 版一致）：

| 要求 | 实现 |
|---|---|
| 不弹黑框 | `CREATE_NO_WINDOW`（`0x08000000`） |
| 超时 | diskpart 300s、DISM apply 3600s（GUI 手动监控而非 wait timeout）、format 120s、get-imageinfo 60s、bcdboot 120s 等，逐一对照 `sys_install.py`/`disk_manager.py` |
| 管道读取 | DISM apply 必须用**逐行读 stdout** 解析进度（`Popen.stdout.readline` 对应物），GBK 输出按代码页解码 |
| 中文路径 | GHO 源路径走 §8.4 规则；unattend/工具复制路径直接用宽字符 API（Rust `std::fs` 是 UTF-16，无 Python ANSI 问题） |

---

## 5. 数据格式（硬性兼容）

### 5.1 config.ini

- 路径：打包后 = exe 同目录；未打包 = 脚本目录（对应 `config.py::get_config_path`）。Rust 实现：用 `std::env::current_exe()` 判断。
- 首次运行不存在则生成默认值（对应 `config.py` 默认值）。
- 格式：`[restore]` 单 section、`key=value`、UTF-8（无 BOM 亦可接受）、值为字符串。

```ini
[restore]
enabled=0
countdown=10
plan_path=
auto_restart=1
copy_tools=0
deploy_unattend=1
disable_defender=1
tools_source_path=Tools
```

字段语义（与 `gui.py` 启动逻辑一致）：

- `enabled`：0/1，是否开机自动还原
- `countdown`：倒计时秒数（GUI 启动弹可取消倒计时）
- `plan_path`：方案 .bat 路径
- `auto_restart`：安装成功后是否显示 30 秒重启对话框
- `copy_tools` / `deploy_unattend` / `disable_defender`：默认安装选项
- `tools_source_path`：调试工具源目录（绝对或相对 exe）

### 5.2 BAT 方案文件（生成 + 解析，100% 兼容）

**用途**：用户"一键分区"确认 + 安装设置可保存为独立可执行 BAT；也可从 BAT 恢复 GUI 方案。存量 .bat 必须能被新程序加载。

#### 5.2.1 整体结构（生成顺序，`bat_builder.py::build_combined_bat`）

```
@echo off
chcp 65001 >nul
:: ===== SKS PLAN BEGIN =====
:: key=value            ← 元数据块（见下）
:: ===== SKS PLAN END =====
title <标题>
<diskpart 段：(echo ... & echo ...) | diskpart，仅最后一个分区段保留 exit>
<盘符探测段（WIM 模式且含固定盘符分配时）>
<安装段：dism/gho + 引导修复 + 收尾>
```

- 行尾 **CRLF**，编码 **UTF-8（无 BOM）**；`chcp 65001` 保证中文 echo 可用。
- 元数据行格式：`:: key=value`，value 经 `_safe_value` 处理：**剔除 `" & | ^` 和首尾空白**（对照 `bat_builder.py`），解析时按第一个 `=` 切分。

#### 5.2.2 元数据键清单（生成时写全；解析时缺省用与 GUI 相同的默认值）

```text
# 多硬盘（新格式，与 Python 现行一致）
disk_count=N
disk_N_physical=<物理盘号>
disk_N_style=gpt|mbr
disk_N_esp_enabled=0|1
disk_N_esp_size_mb=<int>
disk_N_part_count=M
disk_N_part_size_i=<MB 或空=剩余空间>       # i 从 1 起
disk_N_part_fs_i=ntfs|fat32|exfat|refs

# 单硬盘（旧版字段，必须保留生成 + 支持解析，做向后兼容）
partition_style=gpt|mbr
disk_physical=<int>
esp_enabled=0|1
esp_size_mb=<int>
part_count=<int>
part_size_i=<MB 或空>
part_fs_i=<fs>

# 安装设置
image_path=<镜像路径>
image_index_text=<index>: <name>            # 或 gho 路径相关
format=0|1
auto_repair_boot=0|1
copy_tools=0|1
deploy_unattend=0|1
disable_defender=0|1
admin_username=<用户名>
restart_after_install=0|1
target_disk=<int>                            # 安装目标硬盘（物理号）

# 分区选择（安装目标，格式 "盘号:分区号"，如 0:0 / 0:2）
boot_partition=<n:m>
windows_partition=<n:m>

# GHO
image_type=wim|gho
gho_mode=...            # 见 §9.1 已知问题（GUI 实际存 disk|partition，旧文件可能为 load|pload；解析必须两者都接受）
ghost_path=%ProgramFiles(x86)%\ghost\ghost64.exe
gho_disk_index=<int>
gho_partition_index=<int>
```

#### 5.2.3 解析规则（`parse_scheme_data`）

1. 定位 `:: ===== SKS PLAN BEGIN =====` / `END` 行，读取之间所有 `:: k=v`。
2. **多硬盘优先**：有 `disk_count` → 按前缀构造多硬盘结构；否则回落单硬盘旧字段（`disk_physical` 等），语义等价于 `disk_count=1`。
3. 缺失键取与 GUI 默认值一致的 fallback（对照 `_load_scheme` 默认值）。

#### 5.2.4 生成段落（必须逐段对照 `bat_builder.py` 复刻）

1. **diskpart 段**：每张待分区盘一个 `echo` 组（`clean`、`convert gpt|mbr`、`create partition ...`、`format`、`assign`），组与组拼成 `(echo a & echo b) | diskpart`；**仅整个脚本最后一个分区段带 `exit`**（`append_exit` 语义）。
2. **盘符探测段**（WIM 模式）：`setlocal enabledelayedexpansion` + `(echo list volume) | diskpart | findstr NTFS`（含 ESP 时另找 FAT32），`for /f` 取列 **偏移 13** 的字符为盘符；变量 `WIN_LETTER`、`SYS_LETTER`、`ESP_LETTER`。
3. **WIM 安装段**：可选 `format.com`（经 diskpart 临时脚本，与 Python 一致）→ `dism /Apply-Image /ImageFile:"%image_path%" /Index:N /ApplyDir:%SYS_LETTER%:\`（esd 追加 `/Verify /CheckIntegrity`）→ 可选引导修复 `bootsect /nt60 %SYS_LETTER% /force /mbr` + `bcdboot %WIN_LETTER%:\Windows /s %ESP_LETTER% /f all` → 错误码检查（`if errorlevel 1` → `echo` 失败 + `pause`）→ 成功后可选 `shutdown /r /t 3`，否则 `pause`。
4. **GHO 段**：`"%gho_path%" -clone,src="..."`：
   - 硬盘模式：`dst=%gho_disk_index%` + `-sure -fx -rb`（`gho_mode` 为 load/disk）
   - 分区模式（pload）：`src="path:N"`、`dst=%gho_disk_index%:%gho_partition_index%`，还原后**追加盘符探测 + bcdboot 引导修复**（`-rb` 不带）
   - 中文路径处理：生成期替换为 8.3 短名/ASCII 临时方案的产物（见 §8.4）。

> 执行者注意：BAT 内容需要用**现有程序实际生成的样例**（GUI 保存一份多硬盘 WIM 方案、一份 GHO 方案、一份旧版单硬盘方案）做 golden file 对比测试。

---

## 6. 后端功能规格

### 6.1 磁盘枚举（`disk_info.rs` ← `GetDiskInfo.py`）

**设备范围**：`\\.\PhysicalDrive0` … `\\.\PhysicalDrive63`，`CreateFileW(GENERIC_READ, FILE_SHARE_READ|WRITE, OPEN_EXISTING)`；打开失败 = 该盘不存在。

**排除规则**：`IOCTL_STORAGE_QUERY_PROPERTY(StorageDeviceProperty)` → `BusType == 7`（USB）**或** `RemovableMedia == 1` → 排除。

**各项信息与 IOCTL**（详细结构体偏移以 `GetDiskInfo.py` 为准）：

| 信息 | IOCTL / API |
|---|---|
| 型号 | `IOCTL_STORAGE_QUERY_PROPERTY`（PropertyId=0），VendorId(offset 8, 8B) + ProductId(offset 16, 16B) 拼接，Vendor 为 Product 前缀时去重 |
| 容量 | `IOCTL_DISK_GET_LENGTH_INFO`（i64），失败回落 `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`（offset 24 的 i64） |
| 分区表类型 | `IOCTL_DISK_GET_DRIVE_LAYOUT_EX` → 首 DWORD：0=MBR、1=GPT、其他=RAW |
| 分区列表 | 同上：头 48 字节后每条 144 字节；+8 StartingOffset(i64)、+16 PartitionLength(i64)、+24 PartitionNumber(u32)、GPT 类型 GUID@+32（16B）、MBR 类型@+32(1B)；`PartitionLength<=0` 跳过 |
| 盘符映射 | `GetLogicalDriveStringsW` → `GetDriveTypeW==3(DRIVE_FIXED)` → 打开 `\\.\X:` → `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` 取首 extent 的 `DiskNumber`+`StartingOffset` |
| 盘符与分区匹配 | **同物理盘 且 |盘符起始偏移 − 分区偏移| < 1MB** |
| 卷标/文件系统/容量 | `GetVolumeInformationW`、`GetDiskFreeSpaceExW`（总容量参数） |
| SSD 判定 | `IOCTL_STORAGE_QUERY_PROPERTY`(PropertyId=6, StorageAccessAlignmentProperty) → offset 28 `TrimmedFields`(seek penalty)；失败回落型号关键词 `ssd`/`solid state` |

**GPT 类型 GUID（内存原始字节序，直接对照 `GetDiskInfo.py` 常量）**：
- ESP `28 73 2A C1 1F F8 D2 11 BA 4B 00 A0 C9 3E C9 3B`
- MSR `E0 D4 AF E3 8F 76 DA 11 8E 3E 00 50 56 C0 00 08`
- Basic `A2 A0 D0 EB E5 B9 33 44 87 C0 68 B6 B7 26 99 C7`
- Recovery `A4 BB 94 DE D1 06 40 4D A1 6A BF D5 01 79 F6 B2`

**MBR 类型**：`0x01`FAT12、`0x04/0x06`FAT16、`0x07`NTFS、`0x0B/0x0C`FAT32、`0x27`MSR/隐藏、`0xEE`GPT 保护、`0xEF`ESP。

**返回结构**（Python dict 的镜像，键为物理盘号）：`{phys: {型号, 容量(格式化文本), 容量_bytes, 可用空间_bytes, is_gpt, style_name, is_ssd, physical_disk, 分区: {"phys:idx": {分区号, offset_bytes, size_bytes, letter, size(格式化文本), format(NTFS/ESP/MSR/...), 类型, fs, free_bytes}}}}`；无分区但有盘符时按盘符合成分区记录（number=0）。

**日志策略（必须复刻）**：磁盘枚举本身是高频静默操作——
- 后台/自动刷新使用**静默枚举**，不产生 INFO/DEBUG 日志（对应 `_NullLogger` 技巧）；
- 用户主动触发的刷新/操作日志在**首次成功枚举后**才记录完整明细（模块级 `_full_log_emitted` 标志，对照 `GetDiskInfo.py` 尾部）。

**容量格式化**：TB/GB/MB 分档（对照 `GetDiskInfo.py` 的 format 辅助函数，不要自创精度）。

### 6.2 diskpart（`diskpart.rs` ← `disk_manager.py`）

- `_run_diskpart(script_text)`：写入**临时文件**（注意 `win32pipe`/`file` 与路径空格问题，Python 版用 `file=` 语法 → Rust 直接 `diskpart /s "path"`），`CREATE_NO_WINDOW`，**300s 超时**，成功判据：`returncode==0` 且输出不含 `error`（大小写不敏感），随后**删除临时文件**；返回 `(success, output)`。
- 操作封装（逐一对照实现）：`format_partition`（含 EFI 强制 FAT32 quick）、`delete_partition`（`delete partition override`）、`create_partition`（size 可空=剩余空间）、`assign_drive_letter`、`initialize_disk`。
- `_find_available_drive_letter`：**从 Z 向 A** 枚举，跳过已占用与 **A、B、C、D**。
- `one_click_partition(disk, style, esp, data_parts, fixed_letters)`：`clean` → `convert gpt|mbr` →（GPT 且勾选）ESP FAT32 label=`ESP`、盘符优先 **Z** 其他空闲反向字母 → GPT 追加 `create partition primary size=16`（MSR，不分配）→ 数据分区循环（MBR 第一个数据分区 `active`）→ 盘符动态分配：ESP 先占 Z（或其他空闲），数据分区从 **C 向前**（跳过 A/B/已占用）→ 返回 `assigned_letters` 字典（GUI 用于写 `fixed_letters` 语义，见 §8.3）。

### 6.3 安装子系统（`install/*` ← `sys_install.py`）

**WIM/ESD 信息**（`get_wim_info`）：`dism /Get-ImageInfo /ImageFile:"path"`，行级解析中文/英文键（`索引/Index`、`名称/Name`、`描述/Description`、`大小/Size`——去逗号、`bytes` 后缀），返回 `[{index, name, description, size}]`。

**ISO 挂载**（`mount_iso`，照抄策略）：
1. 路径存在性检查；记录挂载前 `GetLogicalDrives` 集合；
2. `explorer.exe "<iso路径>"` 启动；
3. 轮询新盘符：200ms × 超时（主路径 `find_new_drive_letter` 20s/30s 对照实现）→ 超时回落 `FindFirstVolumeW`/`FindNextVolumeW` + `GetVolumePathNamesForVolumeNameW` 差集；
4. 找到后**关闭挂载产生的资源管理器窗口**：`EnumWindows` 匹配类名 `CabinetWClass`/`ExploreWClass`、可见、标题含 `"X:"` → `PostMessageW(WM_CLOSE)`；
5. 返回映射记录（盘符 ↔ 挂载），供卸载复用。
- `find_wim_in_iso`：`<盘符>:\sources\install.wim` → `install.esd`。
- `unmount_iso`：`ShellExecuteW(NULL, "eject", "X:", ...)`，`hInstance > 32` 视为成功；失败记 WARNING 不中断。

**DISM 应用镜像**（`apply_image`，进度机制必须一致）：
- 命令：`dism /Apply-Image /ImageFile:"..." /Index:N /ApplyDir:X:\`；esd 追加 `/Verify /CheckIntegrity`。
- **进度解析**：逐行读 stdout，正则 `(\d+(?:\.\d+)?)\s*%`，**仅整数部分变化时**回调；进度映射到调用方给定区间（GUI 用 20→85%）。
- 取消：回调返回 false 或 `cancel_flag` 置位 → `kill()` 进程。
- 超时：3600s，Python 用手动耗时轮询（非子进程 wait 超时）——Rust 等价实现。

**其他**：`format_partition_quick`（`format.com X: /fs:ntfs /q /y`，stdin null，120s）、`repair_boot`（`bootsect /nt60 {sys} /force /mbr` 失败仅 WARNING 继续；`bcdboot {win}\Windows /s {esp} /f all` 失败=整体失败）、`set_partition_active`/`get_disk_number_from_letter`（**死代码，未被调用——不迁移**，见 §9）。

### 6.4 优化与工具（`optimize.rs` / `copy_tools.rs`）

- **unattend.xml**：整体模板从 `sys_optimize.py::UNATTEND_TEMPLATE` 抽出为 `assets/unattend.xml`（build.rs `include_str!`），保留占位符 `%username%`、`%disable_defender%`。
  - 替换：`admin_username`；defender 占位符按 `disable_defender` 注入/移除 `<RunSynchronousCommand>` 块（**对照原模板替换逻辑逐字节复刻**）。
  - 部署位置：`<系统盘>:\Windows\panther\unattend.xml`（主）+ 尽力复制 `sysprep\panther\`（失败仅 WARNING）。
- **Defender 禁用**（未开 unattend 部署时单独执行）：`Specialize.ps1` 内容 = 注册表 `DisableNotifications=1` + 6 个服务（Sense/WdBoot/WdFilter/WdNisDrv/WdNisSvc/WinDefend `Start=4`），`powershell -ExecutionPolicy Bypass` 执行（对照 `sys_optimize.py`）。
- **流程**（`optimize_system`）：deploy_unattend 失败 → 记 ERROR、`has_error=true` 继续；disable_defender 且未 deploy → 仅 WARNING（**不执行**，与 Python 一致）；copy_tools 独立执行：`<源>/Tools` → `<盘>:\Windows\panther\TestTools`（`dirs_exist_ok` 语义）。

### 6.5 config / 日志 / 主程序

- **config.rs**：`AppConfig { restore_enabled: bool, restore_countdown: int, restore_plan_path, auto_restart, copy_tools, deploy_unattend, disable_defender, tools_source_path }`；`load()` 不存在则 `save()` 默认；值解析 0/1 → bool；**读写保序稳定**。
- **logging.rs**：
  - 文件通道：`log/debug.log`（相对 exe/脚本目录），**DEBUG**，按天滚动，保留 30 天，格式 `%(asctime)s - %(levelname)s - %(message)s` 等价；滚动实现用正确的日期加一天（**修复 Python 版月末 bug，见 §9.2**）。
  - GUI 通道：级别默认 INFO，可切 DEBUG（按钮）；每条按级别着色 tag（debug 蓝 / info 灰 / warning 黄 / error 红），自动滚动到底（可见时；隐藏期间积压，显示时定位到底——对照 `_toggle_log_visible` 1521-1538）。
- **main.rs**：`IsUserAnAdmin` → 否则 `ShellExecuteW(runas, current_exe, "", ...)` 退出自身；窗口标题 `硕科斯系统安装工具 V2.0.0`；内嵌 icon。

---

## 7. GUI 规格（`gui/*` ← `gui.py`）

框架：fltk-rs。**布局、文案、控件状态规则 1:1 复刻**；FLTK 控件对应关系：

| tkinter | fltk-rs |
|---|---|
| `ttk.Treeview`（磁盘树） | `fltk::tree::Tree`（自定义扩展显示）或 `Browser` 自绘行；需支持选中回调、右键/双击无特殊需求 |
| `ttk.Combobox`（readonly） | `fltk::input::Combo`（`InputChoice`/Combo + 限制） |
| `ttk.Spinbox` | `fltk::valuator::Spinner` |
| `tk.Text`（日志） | `fltk::text::TextDisplay` + `TextBuffer`（tag 着色用 `TextDisplay::set_highlight`/样式槽，或按行前缀着色） |
| `Toplevel` 对话框 | `fltk::window::DoubleWindow`（模态：`set_modal`/`run()`） |
| `messagebox` | `fltk::dialog::message_default/alert`（等价交互即可，文案一致） |
| `pack/grid` 布局 | `fltk::group::Flex`（垂直/水平）+ 固定宽度 Group 复刻 |

### 7.1 主窗口（`App`）

- 最小尺寸 **750×550**，标题 `硕科斯分区工具`（对话框头）/ 主窗口标题见 main；icon 内嵌。
- 结构（自上而下）：
  1. **工具栏三行**：
     - 行1 左→右：`自动还原` CheckButton + 红色倒计时 Label + `当前状态：` Label + 状态文本；
     - 行2：**粗体** `当前方案：` + 蓝色方案名 Label；
     - 行3（右对齐 pack RIGHT，先 pack 者最右）：`显示日志` 切换按钮、`保存当前方案`、`加载方案`；
  2. **主体左右分栏**：左 `分区管理` Frame = 磁盘树 + `一键分区` 按钮；右 `系统安装` Frame（§7.2）；
  3. **日志面板**（底部，默认隐藏）：`隐藏日志/显示日志` 按钮、日志级别切换（INFO/DEBUG）、TextDisplay。
- **磁盘树**：两级 `磁盘 N: <型号>（SSD/HDD）  [容量]` → 分区行 `<key>  <size>  <format>  <盘符>`（盘符空显示 `-`）；预览节点灰色斜体 `(预览) ...`。
- **一键分区按钮状态机**（`_update_buttons`）：安装中 → 禁用；未选中/选中分区 → 禁用；选中磁盘 → 可用。

### 7.2 安装面板

- `安装源`：路径 Entry + `浏览...`（文件类型过滤 `*.iso *.wim *.esd *.gho` + 各单项 + 所有文件）+ `镜像:` 只读 Combobox（WIM 索引列表，项文本 `<index>: <name> <size>`，显示名截断规则对照 `_format_image_name`）。
- `目标分区` Frame（WIM 模式可见）：`系统安装硬盘:` Combobox + 预览 Label（`磁盘N: 型号 ... (剩余可用 xx GB)` 格式对照 `_update_disk_select_preview`）；分区选择按方案键 `boot_partition`/`windows_partition`（**键选择 + 硬盘选择联动**，见 §8.2）。
- `GHO 还原选项` Frame（GHO 模式可见，与目标分区互斥）：还原模式 Radio（`硬盘还原`/`分区还原`）+ `目标硬盘:`/`目标分区:` Combobox + `Ghost路径:` Entry。
- `选项` Frame：
  - WIM：`格式化目标分区`、`复制调试工具`(+路径 Entry+`浏览`)、`自动部署与系统优化`(+`用户名` Entry)、`禁用Windows Defender`、`修复引导`、`安装完成后重启`；
  - GHO：`还原后自动重启`。
  - **选项可用性规则**（`_on_install_source_changed`）：仅当镜像来自 ISO 内 `install.wim/esd` 时 才启用 copy_tools/deploy_unattend/disable_defender（且默认勾选继承 config）；直接选 wim/esd 文件时禁用并取消勾选。
- 底部：`开始安装` 按钮 + 进度条。

**GHO/WIM 模式切换**（`_set_gho_mode`）：源路径后缀 `.gho`（不区分大小写）→ GHO 模式（切换 Frame 可见性、重建镜像下拉、清空/恢复选项状态）；否则 WIM 模式。

### 7.3 一键分区对话框（`one_click.rs` ← `OneClickPartitionDialog`）

- 控件：分区表格式 Radio（GPT/MBR）、`EFI 分区` Check（默认勾选，`_set_gpt_default_esp`）+ 大小 Entry + 单位 Combo（MB/GB，默认 GB，显示 `GB (推荐)` 只在 GPT 初次状态？——以代码为准）、分区数量 Spinner（1-8，默认 3）、动态分区行（大小 Entry + 单位 + 文件系统 `ntfs/fat32/exfat/refs` + 灰色提示 Label）、底部剩余空间 Label、`保存`/`取消`。
- **联动**（§516-751 行级复刻）：
  - MBR → 强制取消 ESP 且禁用 ESP 控件；GPT → 启用；
  - ESP 勾选/大小/单位、分区行 KeyRelease、单位切换 → 实时重算 `_update_remain`：
    - 已用 = ESP + (GPT ? 16MB MSR : 0) + 手动分区合计；
    - 剩余均分给自动行（余数给最后一个自动行）；自动行输入框保持空、均分值放提示 `自动均分 X`；
    - 全手动且有剩余 → 剩余并入最后分区并回写输入框（`包含剩余空间`）；
    - 溢出 → 红色 `空间不足! 已超出 NMB`，自动行提示 `空间不足！`；
    - 显示规则：`_set_row_display` ≥1024MB 用 GB（2 位小数）否则 MB。
- **保存校验**（`_ok`）：重新计算 → ESP>0（若勾选）→ 每分区 size_mb>0（区分 `大小无效`/`大小不能为空`）→ 合计（含 MSR 16）≤ 磁盘容量 → 通过后 `result = {style, esp_enabled, esp_size_mb, data_parts:[{size_mb(最后一个自动=None), fs}]}`。
- **确认结果不是立即执行**：写入 `_pending_partitions`（含 `disk_physical`），磁盘树替换为 `(预览)` 子节点（ESP/FAT32、MSR 16MB、各数据分区，`_apply_disk_preview`），实际执行延迟到"开始安装"（§8.3）。多硬盘可暂存多个方案。

### 7.4 重启对话框（`restart_dialog.rs`）

安装成功且 `auto_restart` 开启：模态对话框，文案 `系统安装完成...30 秒后自动重启`，每秒倒计时更新；`立即重启`（`shutdown /r /t 0`，按钮置灰 + `正在重启...`）、`取消`。倒计时归零自动触发重启。

### 7.5 方案保存/加载/变更追踪（§1485-1700 区域）

- **追踪**（`_setup_scheme_tracking`）：11 个变量 `trace` + 硬盘下拉 `<<ComboboxSelected>>`（**不得用 trace_add**，避免程序赋值误触发失效）；变更 → `_on_scheme_changed`：`_applying_scheme` 加载期间与 `_is_installing` 时忽略；无方案时隐藏，否则 `当前方案：xxx（已修改，点击"立即保存"）` + 状态行提示。
- **保存**：走"另存为"对话框 → `build_combined_bat`（§5.2）→ 更新方案名/路径、清失效标记、状态 `已保存`、刷新 config 的 `plan_path`？（以代码为准：仅当勾选自动还原相关才写 config——对照 `_save_scheme` 实际行为）。
- **加载**：文件对话框 → `parse_scheme_data` → `_apply_scheme_inner` 全量回填（模式切换、变量 set、`_applying_scheme` 防抖、完成后按需刷新磁盘与下拉）→ 标记 active。
- **缺失镜像等校验**：加载后校验 `image_path` 存在，缺失弹窗提示（手动路径）；自动还原路径例外见 §9.4。

---

## 8. 核心业务流程

### 8.1 启动流程（`run_gui`）

1. 加载 config（不存在生成默认）；
2. 若 `plan_path` 存在 → `after(100ms)` **静默**加载方案（不弹文件对话框、不弹校验窗）；
3. `after(500ms)` `_startup_check`：`restore_enabled && plan_path` → 启动倒计时（`restore_countdown` 秒，每秒刷新，红色倒计时 Label）；期间用户点 `取消`/任意有效操作 → 取消倒计时（文案 `自动还原已取消`）；
4. 倒计时归零 → **静默加载方案 → 加载镜像 → `_on_install_system(skip_confirm=True)`**（跳过确认弹窗，直接进入安装线程）；
5. 初次显示磁盘树（后台首扫）。

### 8.2 分区选择与盘符语义（安装面板目标选择）

- 方案键 `boot_partition` / `windows_partition` 格式 `盘号:分区号`（**盘号 = 该硬盘在树中的序号 disk_idx，不是物理号**——对照 `_gather_scheme_data`）。
- GPT 派生键：`system` 分区 = 硬盘内 index 2（ESP=0、MSR=1），`boot` = index 0；MBR：两者都 = 0。选择逻辑对照 `_update_boot_windows_defaults`。
- 硬盘下拉选择触发键为 `ComboboxSelected`；分区下拉联动重建时保持已选值（可保留则保留，不可保留回落默认）。

### 8.3 WIM 安装主流程（`_on_install_system` → `install_system`）

> 线程：确认后 UI 进入 `_is_installing`（按钮禁用、自动刷新暂停/降级，对照实现），后台线程执行：

```text
[阶段0  确认] skip_confirm=false 时弹确认框（含暂存方案摘要）
[阶段0b 预览分区] 若 _pending_partitions 非空：
      对每个暂存方案 → diskpart.rs::one_click_partition（fixed_letters 支持）
      → 完成后 _clear_disk_preview() 清预览
[阶段1  盘符确定] 刷新磁盘信息 → 目标分区已带盘符（J:）否则自动分配
      → 按 §8.2 得 win_letter（系统分区）、boot_letter（ESP，GPT 且无盘符时
        自动找可用盘符，优先 Z，跳过 A/B/C/D）；MBR 无 ESP → boot=win
[阶段2  格式化] format=1 → format.com 快速格式化，进度 10%
[阶段3  应用镜像] dism apply，进度 20%→85%（行级解析）；ISO 先挂载
      （挂载盘符记录，结束/失败都要卸载）
[阶段4  引导修复] auto_repair_boot=1 → 进度 90%
      bootsect（失败 WARNING 继续）+ bcdboot（失败=安装失败）
      MBR：另 set active（diskpart）
[阶段5  完成] 进度 100%；成功 → 后台执行 optimize_system（unattend/defender/tools）
      → 弹"安装完成"（含是否有优化警告）
      → auto_restart=1 → RestartDialog（30s）；否则 askyesno "是否立即重启？"
失败   → status 红色 + 日志 ERROR；GHO 失败追加读取 ghost 输出/调试 log 尾部
        （对照 _read_ghost_debug_tail）；ISO 已挂载则尝试卸载
```

- 全程 `cancel` 支持点：格式化前、每阶段间、DISM 行循环内（kill 进程）。
- 进度条/状态文案阶段对照 `_on_install_progress` 的字符串（`正在分区...`、`正在应用镜像...`、`正在修复引导...` 等——以代码为准逐条复刻）。

### 8.4 GHO 安装主流程（`_on_gho_install` + `ghost.rs`）

- 模式：`gho_mode` ∈ {硬盘还原(disk/load), 分区还原(partition/pload)}；
- Ghost 命令（`ghost64.exe` 路径支持 `%ProgramFiles(x86)%` 变量展开——GUI 内 `expandvars` 语义，BAT 内由 CMD 展开）：
  - 硬盘：`-clone,src="<gho>",dst=<N> -sure -fx -rb`
  - 分区：`-clone,src="<gho>:1",dst=<N>:<M> -sure -fx`（不带 -rb），**还原完成后**盘符探测 + `bcdboot` 引导修复
- **中文路径 workaround（必须复刻，`_relative_gho_src` / `_to_short_path`）**：
  1. 取镜像所在目录 `cwd`，尝试 `GetShortPathNameW` 取 8.3 短名做相对路径；
  2. 短名不可用（如 exFAT）→ 复制/移动到 ASCII 临时名（同目录或 `%TEMP%` 带随机名），还原后清理；
  3. Ghost 以 ANSI 代码页解析命令行 → 所有路径必须 ASCII 可表示，否则失败回退方案对照原代码。
- 硬盘还原目标 = `gho_disk_index`（下拉值=物理盘号）；分区还原 `gho_partition_index`（`Disk:` 下分区号，格式对照 `_gather_scheme_data`）。
- 失败尾部输出读取：Ghost 调试输出文件尾部（`_read_ghost_debug_tail`），追加到日志面板。

### 8.5 自动刷新（`_refresh_disk_info` / `_display_disk_info`）

- 周期 5s；静默后台枚举；结果与当前 `disk_data` 比对：
  - **增量更新**（`_update_tree_incremental`，§2789-2871）：按物理号匹配磁盘节点；有 `_pending_partitions` 的磁盘**跳过分区刷新保留预览**；分区盘符变化 → 更新硬盘下拉；找不到节点/新增分区 → 整树重建；
  - 首次/空数据 → `_rebuild_tree`。
- 刷新后 `_apply_disk_preview` 重放预览节点。

---

## 9. 已知问题与修复决策（1:1 范围内允许的唯一改动）

| # | 问题 | 定位 | 决策 |
|---|---|---|---|
| 1 | **gho_mode 语义不一致**：GUI 保存 `disk`/`partition`，`build_gho_install_section` 判断 `== "pload"` → GUI 保存的"分区还原"方案生成 BAT 后**实际执行硬盘还原** | `gui.py` `_gather_scheme_data` ↔ `bat_builder.py:471` | **修复**：解析器同时接受 `disk\|load\|partition\|pload`；生成器判断 `is_pload = mode ∈ {pload, partition}`；写出值保持与传入一致（不改字段格式，新旧文件互通） |
| 2 | `DailyRotatingFileHandler._next_midnight` 用 `replace(day=day+1)`，月末抛 ValueError 滚动失败 | `main.py` | **修复**：用 `chrono` 日期 +1 天计算下次滚动 |
| 3 | `select_wim_file()` 死代码（未调用） | `sys_install.py:667` | 不迁移 |
| 4 | `get_disk_number_from_letter` / `set_partition_active`（sys_install 版）死代码 | `sys_install.py:867,911` | 不迁移（diskpart 内 active 逻辑按 §6.2 保留） |
| 5 | 自动还原路径镜像缺失时弹**模态阻塞** messagebox，无人值守场景卡死 | `gui.py` `_startup_check`/静默加载 | **修复**：自动还原分支不弹模态框，改为状态行红字 + 日志 ERROR + 取消倒计时；手动加载仍弹窗 |
| 6 | 其余一切行为差异 | — | 视为缺陷，按 Python 实现对齐（Python 是唯一真值） |

---

## 10. 实施里程碑（建议执行顺序）

每步完成后必须 `cargo build` + `cargo clippy`（warnings=deny）通过，并对照 Python 行为自测：

1. **骨架 + 后端基础**：Cargo 工程、`main.rs`（管理员检查/UAC）、`logging.rs`、`config.rs`（含默认生成）。
2. **磁盘枚举**：`disk_info.rs`，写一个 `--dump-disks` 调试开关输出与 Python 版相同的 dict 文本，**双版本比对**。
3. **diskpart 封装**：`diskpart.rs` 全操作 + 一键分区（不含 GUI），dry-run 输出脚本与 Python 生成的 diskpart 脚本 diff。
4. **安装子系统**：`install/*`（DISM/ISO/boot）、`optimize.rs`、`copy_tools.rs`。
5. **方案层**：`scheme/bat.rs` 生成+解析（golden file 测试：用旧程序产出的 .bat 样例做解析回归）、`scheme/plan.rs` 暂存逻辑、`ghost.rs`。
6. **GUI**：主窗口骨架 → 磁盘树 + 自动刷新 + 预览 → 安装面板与模式切换 → 一键分区对话框 → 方案保存/加载/追踪 → 倒计时/自动还原 → 日志面板 → 重启对话框。
7. **端到端**：§11 验收清单全量执行；`build.rs` 内嵌 icon/模板；产出发布 exe。

---

## 11. 验收标准

### 11.1 兼容性（硬性）

- [ ] 能加载**旧程序生成的** .bat 方案（单硬盘旧格式、多硬盘新格式、GHO 方案）并正确回填 GUI；
- [ ] 新程序生成的 .bat 可被**旧程序**加载（元数据键集、`_safe_value` 规则、CRLF、段落结构一致）；
- [ ] 新程序生成的 .bat 可直接在 CMD/WinPE 执行完成"分区+装系统"（WIM 与 GHO 各验一次）；
- [ ] config.ini 与旧版互换读写；缺省文件首次运行自动生成；
- [ ] unattend.xml 输出与旧程序字节级一致（同参数）。

### 11.2 功能验收清单（对照 §1）

- [ ] 磁盘列表：GPT/MBR/容量/型号/SSD/分区/盘符显示一致；U 盘被排除；5s 静默刷新且有预览时保留预览节点；
- [ ] 一键分区：对话框全部联动/校验/预览/暂存→安装时执行；多硬盘多方案；
- [ ] WIM 安装：ISO 挂载（新盘符探测、explorer 窗口关闭）、索引列表、进度 20-85%、引导修复、失败路径与 ISO 卸载；
- [ ] GHO 安装：两种模式、中文路径 workaround、失败尾部日志；
- [ ] 方案保存/加载/变更失效追踪/立即保存；
- [ ] 自动还原：倒计时、取消、静默加载、镜像缺失非阻塞处理；
- [ ] 优化与工具部署、Defender 禁用分支逻辑；
- [ ] 日志：文件按天滚动保留 30 天、GUI 着色与级别切换、隐藏/显示定位；
- [ ] 非管理员启动触发 UAC 提权；重启对话框 30s 倒计时；
- [ ] **WinPE 实测**：exe 在 WinPE 下正常显示与运行（fltk/GDI 无黑屏）、diskpart/DISM 全流程可用。

### 11.3 工程质量

- [ ] `cargo clippy` 无警告；核心解析（BAT 元数据、DISM 输出、进度）有单元测试；
- [ ] 无 unwrap 直接 panic 于 IO/子进程路径（错误上抛到日志与状态栏）；
- [ ] 发布产物 = 单 exe，图标、版本信息（2.0.0）完整。

---

## 附录 A：关键行为速查（文件:行号，锚定 `90fee4f`）

| 行为 | 位置 |
|---|---|
| 日志初始化 / 按天滚动 / 30 天 | `main.py:24-109` |
| 管理员检查与 UAC | `main.py`（IsUserAnAdmin / ShellExecuteW runas） |
| 磁盘枚举 IOCTL 细节 | `GetDiskInfo.py`（各 ioctl helper + 底部日志策略） |
| diskpart 执行与超时 | `disk_manager.py::_run_diskpart` |
| 一键分区（含盘符分配） | `disk_manager.py::one_click_partition` |
| DISM apply 进度解析 | `sys_install.py::apply_image` |
| ISO 挂载/卸载/关窗口 | `sys_install.py::mount_iso / _find_new_drive_letter / unmount_iso` |
| 引导修复 | `sys_install.py::repair_boot` |
| unattend 模板与替换 | `sys_optimize.py::UNATTEND_TEMPLATE / deploy_unattend` |
| BAT 元数据与段生成 | `bat_builder.py::build_combined_bat / build_diskpart_section / build_gho_install_section / parse_scheme_data` |
| 一键分区对话框 | `gui.py:380-843` |
| 重启对话框 | `gui.py:846-948` |
| 方案保存/加载/追踪 | `gui.py:1485-1700` 区域 |
| GHO 模式切换 | `gui.py::_set_gho_mode` |
| 安装主流程 | `gui.py::_on_install_system / _on_gho_install` |
| 磁盘树增量刷新 | `gui.py:2789-2871` |
| 预览节点 | `gui.py:2901-2962` |
| 剩余空间均分 | `gui.py::OneClickPartitionDialog._update_remain`（652-751） |
| 8.3 短名 workaround | `gui.py::_relative_gho_src / _to_short_path` |
| 自动还原启动 | `gui.py::_startup_check / _countdown_tick`（run_gui 尾部） |
