//! 磁盘信息枚举（← GetDiskInfo.py）
//!
//! 通过 DeviceIoControl 获取物理磁盘的型号、容量、分区表类型与分区详情，
//! 枚举 PhysicalDrive0~63，排除 USB 总线与可移动介质。
//!
//! 日志策略（复刻 Python `_disk_logger` / `_disk_logged` 技巧）：
//! 磁盘枚举是高频静默操作 —— 首次成功枚举写完整明细（DEBUG），
//! 之后的自动刷新全部静默。
//!
//! **非管理员（普通用户）运行**：Python 原版以 `GENERIC_READ` 打开
//! `\\.\PhysicalDriveN`，普通用户会被系统拒绝（ERROR_ACCESS_DENIED）→ 枚举结果为空。
//! 本模块在 `GENERIC_READ` 失败时**降级**为 `FILE_READ_ATTRIBUTES` 只读属性权限重开，
//! 该权限普通用户即可获得，且足以完成全部只读查询：
//! `IOCTL_STORAGE_QUERY_PROPERTY`（型号/总线/可移动/SSD）、
//! `IOCTL_DISK_GET_DRIVE_LAYOUT_EX`（分区表与分区列表）、
//! `IOCTL_DISK_GET_DRIVE_GEOMETRY`（磁盘总容量）、
//! `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`（盘符→磁盘映射）。
//! 解析逻辑与 Python 原版逐字节一致，两种权限下结果相同。

// GUI 接入前的临时豁免（里程碑 6 启用）
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW,
    GetVolumeInformationW, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// 是否已完成首次完整磁盘日志（对应 Python `_disk_logged`）
static DISK_LOGGED: AtomicBool = AtomicBool::new(false);

/// 是否已提示过"非管理员受限模式"（只提示一次，避免 5s 自动刷新刷屏）
static RESTRICTED_WARNED: AtomicBool = AtomicBool::new(false);

/// 首次枚举前日志真实写入，之后静默（对照 `_disk_logger` / `_NullLogger`）
macro_rules! disk_debug {
    ($($arg:tt)*) => {
        if !DISK_LOGGED.load(Ordering::Relaxed) {
            log::debug!($($arg)*);
        }
    };
}

// ======================== 常量（与 GetDiskInfo.py 一致） ========================

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_DISK_GET_DRIVE_LAYOUT_EX: u32 = 0x0007_0050;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_4060;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00A0;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

/// GetDriveTypeW: 本地固定磁盘
const DRIVE_FIXED: u32 = 3;
/// BusType == 7 为 USB
const BUS_TYPE_USB: u32 = 7;

const DRIVE_LAYOUT_HEADER_SIZE: usize = 48;
const PART_ENTRY_SIZE: usize = 144;

// ======================== 数据结构 ========================

/// 单个分区信息（键为 `phys:idx`，idx 为枚举序号而非分区号）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartitionInfo {
    /// 格式化容量文本，如 "120.00GB"
    pub size: String,
    pub size_bytes: i64,
    /// 分区类型名（ESP/MSR/Basic Data/NTFS/...）
    pub ptype: String,
    /// 文件系统（仅该分区有盘符时可得，否则空串）
    pub format: String,
    /// 盘符小写字母，无则空串
    pub letter: String,
    /// diskpart 分区号（从 1 起）
    pub number: u32,
}

/// 单个物理磁盘信息（结构与 GetDiskInfo.main() 返回的 dict 逐键对应）
#[derive(Debug, Clone, PartialEq)]
pub struct DiskInfo {
    pub model: String,
    pub size: String,
    pub size_bytes: i64,
    /// 分区表类型：GPT / MBR / RAW / Unknown
    pub style: String,
    pub available: String,
    pub available_bytes: i64,
    pub physical_disk: u32,
    /// None = 无法判断（JSON 中为 null）
    pub is_ssd: Option<bool>,
    /// 分区列表（键 `phys:idx`）；对应 Python 中无分区时 `分区 = 0`
    pub partitions: Vec<(String, PartitionInfo)>,
}

/// 盘符 → 物理磁盘 + 起始偏移（对应 Python `_mappings()` 返回值）
#[derive(Debug, Clone)]
struct DriveMapping {
    letter: char,
    disk: u32,
    offset: i64,
}

// ======================== 底层封装 ========================

/// 打开设备句柄（← Python `_open`）
///
/// 访问权限策略（与 Python 原版唯一的差异点，其余解析完全一致）：
/// 1. 先按 Python 原版用 `GENERIC_READ` 打开（管理员 / 有磁盘读权限时成功）；
/// 2. 失败且错误码为 `ERROR_ACCESS_DENIED`（普通用户无磁盘读权限）时，
///    降级为 `FILE_READ_ATTRIBUTES`（只读属性权限）重试 —— 普通用户即可获得，
///    且足以完成本模块全部只读查询。
/// 3. 其它错误（如设备不存在的 `ERROR_FILE_NOT_FOUND`）直接跳过，与 Python 一致。
///
/// 返回 `(句柄, 是否受限模式)`；受限模式 = 仅以 `FILE_READ_ATTRIBUTES` 打开。
fn open_device(path: &str) -> Option<(HANDLE, bool)> {
    if let Some(h) = create_file(path, GENERIC_READ) {
        return Some((h, false));
    }
    // SAFETY: 紧接 CreateFileW 失败后读取线程的最后错误码（中间无其它 Win32 调用）
    let err = unsafe { GetLastError() };
    if err != ERROR_ACCESS_DENIED {
        return None;
    }
    let h = create_file(path, FILE_READ_ATTRIBUTES)?;
    Some((h, true))
}

/// CreateFileW 封装：共享读写 + OPEN_EXISTING（← Python `_open` 的参数组合）
fn create_file(path: &str, access: u32) -> Option<HANDLE> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 路径为合法 NUL 结尾宽字符串，其余参数为标准打开标志
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        None
    } else {
        Some(h)
    }
}

/// DeviceIoControl 输出缓冲（输入为空）
fn ioctl_out(h: HANDLE, code: u32, out_size: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; out_size];
    let mut returned = 0u32;
    // SAFETY: out 缓冲 out_size 字节，returned 为有效 out 参数
    let ok = unsafe {
        DeviceIoControl(
            h,
            code,
            std::ptr::null(),
            0,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            out_size as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(out)
    }
}

/// DeviceIoControl 输出缓冲（带 12 字节 STORAGE_PROPERTY_QUERY 输入）
fn ioctl_query_property(h: HANDLE, property_id: u32, query_type: u32, out_size: usize) -> Option<Vec<u8>> {
    let mut input = [0u8; 12];
    input[0..4].copy_from_slice(&property_id.to_le_bytes());
    input[4..8].copy_from_slice(&query_type.to_le_bytes());
    let mut out = vec![0u8; out_size];
    let mut returned = 0u32;
    // SAFETY: 输入 12 字节（STORAGE_PROPERTY_QUERY），输出缓冲有效
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            input.as_ptr() as *const core::ffi::c_void,
            input.len() as u32,
            out.as_mut_ptr() as *mut core::ffi::c_void,
            out_size as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(out)
    }
}

fn read_u32(raw: &[u8], off: usize) -> u32 {
    if off + 4 > raw.len() {
        return 0;
    }
    u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
}

fn read_i64(raw: &[u8], off: usize) -> i64 {
    if off + 8 > raw.len() {
        return 0;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&raw[off..off + 8]);
    i64::from_le_bytes(b)
}

/// 从原始缓冲区读取以 NUL 结尾的 ASCII 字符串（对应 Python `_read_str`）
fn read_ascii_str(raw: &[u8], off: usize) -> String {
    if off == 0 || off >= raw.len() {
        return String::new();
    }
    let Some(rel) = raw[off..].iter().position(|&b| b == 0) else {
        return String::new();
    };
    // Python: decode('ascii', errors='ignore').strip() —— 非 ASCII 字节丢弃
    let filtered: Vec<u8> = raw[off..off + rel]
        .iter()
        .copied()
        .filter(|b| b.is_ascii())
        .collect();
    String::from_utf8_lossy(&filtered).trim().to_string()
}

// ======================== 查询函数 ========================

/// STORAGE_DEVICE_DESCRIPTOR 解析结果（← Python `STORAGE_DEVICE_DESCRIPTOR`）
#[derive(Debug, Clone, Default, PartialEq)]
struct DeviceDescriptor {
    /// VendorId（偏移 12 指向的 ASCII 串）
    vendor: String,
    /// ProductId（偏移 16 指向的 ASCII 串）
    product: String,
    /// BusType（偏移 28）
    bus_type: u32,
    /// RemovableMedia（偏移 9 的字节，非 0 即为可移动介质）
    removable: bool,
}

/// 解析 STORAGE_DEVICE_DESCRIPTOR 原始字节（纯函数，便于单测）
///
/// 偏移对照 Python `STORAGE_DEVICE_DESCRIPTOR`：
/// `VendorIdOffset`@12、`ProductIdOffset`@16、`BusType`@28、`RemovableMedia`@9。
fn parse_device_descriptor(raw: &[u8]) -> DeviceDescriptor {
    DeviceDescriptor {
        vendor: read_ascii_str(raw, read_u32(raw, 12) as usize),
        product: read_ascii_str(raw, read_u32(raw, 16) as usize),
        bus_type: read_u32(raw, 28),
        removable: raw.get(9).copied().unwrap_or(0) != 0,
    }
}

/// 磁盘型号：VendorId + ProductId 拼接，Vendor 为 Product 前缀时去重
fn model(h: HANDLE) -> String {
    let Some(raw) = ioctl_query_property(h, 0, 0, 40 + 512) else {
        return "Unknown".to_string();
    };
    let desc = parse_device_descriptor(&raw);
    if !desc.vendor.is_empty() && !desc.product.is_empty() {
        if desc
            .product
            .to_uppercase()
            .starts_with(&desc.vendor.to_uppercase())
        {
            return desc.product;
        }
        return format!("{} {}", desc.vendor, desc.product);
    }
    if !desc.product.is_empty() {
        desc.product
    } else if !desc.vendor.is_empty() {
        desc.vendor
    } else {
        "Unknown".to_string()
    }
}

/// 总线类型（0=Unknown ... 7=USB ...），失败返回 0
fn bus_type(h: HANDLE) -> u32 {
    ioctl_query_property(h, 0, 0, 40 + 512)
        .map(|raw| parse_device_descriptor(&raw).bus_type)
        .unwrap_or(0)
}

/// RemovableMedia 标志（第二个排除信号）
fn is_removable(h: HANDLE) -> bool {
    ioctl_query_property(h, 0, 0, 40 + 512)
        .map(|raw| parse_device_descriptor(&raw).removable)
        .unwrap_or(false)
}

/// SSD 判定：StorageAccessAlignmentProperty(offset 28 seek penalty) → 型号关键词回落。
/// 返回 None = 无法判断。日志文案对照 `_is_ssd`。
fn is_ssd(h: HANDLE, model_name: &str) -> Option<bool> {
    // 方法1: DeviceIoControl
    if let Some(raw) = ioctl_query_property(h, 6, 0, 1024) {
        // 偏移 28 = DeviceSeekPenalty：0=SSD，1=HDD
        let seek_penalty = read_u32(&raw, 28);
        if seek_penalty == 0 {
            disk_debug!("_is_ssd: DeviceIoControl检测到无寻道延迟，判断为SSD");
            return Some(true);
        }
        disk_debug!("_is_ssd: DeviceIoControl检测到有寻道延迟，判断为HDD");
        return Some(false);
    }

    // 方法2: 型号关键词
    if !model_name.is_empty() && model_name != "Unknown" {
        let up = model_name.to_uppercase();
        disk_debug!("_is_ssd: 使用型号关键词检测，model='{}'", model_name);

        const SSD_KEYWORDS: [&str; 28] = [
            "SSD", "NVME", "NVM", "M.2", "PCIE", "PCI-E", "SOLID STATE", "ADATA", "KINGSTON",
            "CRUCIAL", "SAMSUNG SSD", "WD BLUE", "INTEL SSD", "WD BLACK", "SEAGATE FIRECUDA",
            "TOSHIBA TR200", "KINGSPEC", "FORESEE", "GLOWAY", "MAXSUN", "BIWIN", "TEAMGROUP",
            "LEXAR", "PATRIOT", "SK HYNIX", "SANDISK", "MICRON", "",
        ];
        for kw in SSD_KEYWORDS {
            if !kw.is_empty() && up.contains(kw) {
                disk_debug!("_is_ssd: 匹配到 SSD 关键词 '{kw}'，返回 True");
                return Some(true);
            }
        }
        const HDD_KEYWORDS: [&str; 16] = [
            "HDD", "HARD DISK", "7200", "5400", "ST1000DM", "ST2000DM", "WD10EZEX", "WD20EZRZ",
            "WD40EZRZ", "ST4000DM", "DT01ACA", "MK7575GS", "HTS5450", "WD5000LPVX",
            "TOSHIBA MQ04ABF100", "",
        ];
        for kw in HDD_KEYWORDS {
            if !kw.is_empty() && up.contains(kw) {
                disk_debug!("_is_ssd: 匹配到 HDD 关键词 '{kw}'，返回 False");
                return Some(false);
            }
        }
    }

    disk_debug!("_is_ssd: 所有检测方法都无法判断，返回 None");
    None
}

/// 磁盘总字节数：IOCTL_DISK_GET_LENGTH_INFO → IOCTL_DISK_GET_DRIVE_GEOMETRY_EX(offset 24)
///
/// 两个控制码的权限要求不同（这是"非管理员也能拿到精确容量"的关键）：
///
/// - `0x00074060`（LENGTH_INFO）为 `FILE_READ_ACCESS` → **需要管理员权限**；
/// - `0x000700A0`（GEOMETRY）为 `FILE_ANY_ACCESS` → **普通用户即可查询**，
///   返回的缓冲区是 `DISK_GEOMETRY_EX` 布局，容量为偏移 24 的 `LARGE_INTEGER`。
///
/// 因此普通用户下方案一失败、方案二命中，得到的容量与管理员下完全一致。
fn size(h: HANDLE) -> i64 {
    // 方案一：IOCTL_DISK_GET_LENGTH_INFO（FILE_READ_ACCESS）
    let length = ioctl_out(h, IOCTL_DISK_GET_LENGTH_INFO, 512).map(|raw| read_i64(&raw, 0));
    // 方案二：IOCTL_DISK_GET_DRIVE_GEOMETRY（FILE_ANY_ACCESS），仅在方案一失败时才发起
    let geometry = if length.is_none() {
        ioctl_out(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, 512).map(|raw| read_i64(&raw, 24))
    } else {
        None
    };
    pick_size(length, geometry)
}

/// 容量取值优先级：LENGTH_INFO → GEOMETRY → 0（纯函数，便于单测；← Python `_size`）
fn pick_size(length: Option<i64>, geometry: Option<i64>) -> i64 {
    length.or(geometry).unwrap_or(0)
}

/// 分区表类型：GPT / MBR / RAW / Unknown
fn style(h: HANDLE) -> String {
    let Some(raw) = ioctl_out(h, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, 16384) else {
        return "Unknown".to_string();
    };
    match read_u32(&raw, 0) {
        0 => "MBR".to_string(),
        1 => "GPT".to_string(),
        _ => "RAW".to_string(),
    }
}

// GPT 分区类型 GUID（磁盘原始字节序，对照 GetDiskInfo.py 常量）
const GPT_ESP: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
    0x3B,
];
const GPT_MSR: [u8; 16] = [
    0xE0, 0xD4, 0xAF, 0xE3, 0x8F, 0x76, 0xDA, 0x11, 0x8E, 0x3E, 0x00, 0x50, 0x56, 0xC0, 0x00,
    0x08,
];
const GPT_BASIC: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
    0xC7,
];
const GPT_RECOVERY: [u8; 16] = [
    0xA4, 0xBB, 0x94, 0xDE, 0xD1, 0x06, 0x40, 0x4D, 0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xF6,
    0xB2,
];

/// MBR 分区类型 → 名称（对照 `_MBR_TYPES`，未知名返回 `0xNN`）
fn mbr_type_name(t: u8) -> String {
    match t {
        0x01 => "FAT12",
        0x04 | 0x06 => "FAT16",
        0x07 => "NTFS",
        0x0B | 0x0C => "FAT32",
        0x27 => "MSR",
        0xEE => "GPT Protective",
        0xEF => "ESP",
        other => return format!("0x{other:02X}"),
    }
    .to_string()
}

/// 分区条目（← Python `_partition_list` 返回的 dict）
#[derive(Debug, Clone, PartialEq)]
struct PartEntry {
    /// 起始偏移（字节）
    offset: i64,
    /// 分区长度（字节）
    length: i64,
    /// 分区类型名（ESP/MSR/Basic Data/NTFS/...）
    ptype: String,
    /// 分区编号（diskpart 中的分区序号）
    number: u32,
}

/// 解析 DRIVE_LAYOUT_INFORMATION_EX 原始字节（纯函数，便于单测；← Python `_partition_list`）
///
/// 结构：头 48 字节（`PartitionStyle`@0、`PartitionCount`@4），
/// 之后每条 `PARTITION_INFORMATION_EX` 144 字节：
/// `StartingOffset`@+8、`PartitionLength`@+16、`PartitionNumber`@+24、
/// 类型 GUID（GPT）/`PartitionType` 字节（MBR）@+32。
fn parse_layout(raw: &[u8]) -> Vec<PartEntry> {
    let mut out = Vec::new();
    let style_v = read_u32(raw, 0);
    let count = read_u32(raw, 4);
    for i in 0..count {
        let base = DRIVE_LAYOUT_HEADER_SIZE + i as usize * PART_ENTRY_SIZE;
        if base + PART_ENTRY_SIZE > raw.len() {
            break;
        }
        let starting_offset = read_i64(raw, base + 8);
        let partition_length = read_i64(raw, base + 16);
        let partition_number = read_u32(raw, base + 24);
        if partition_length <= 0 {
            continue;
        }
        let ptype = if style_v == 1 {
            let mut guid = [0u8; 16];
            guid.copy_from_slice(&raw[base + 32..base + 48]);
            if guid == GPT_ESP {
                "ESP"
            } else if guid == GPT_MSR {
                "MSR"
            } else if guid == GPT_BASIC {
                "Basic Data"
            } else if guid == GPT_RECOVERY {
                "Recovery"
            } else {
                "Unknown"
            }
            .to_string()
        } else if style_v == 0 {
            mbr_type_name(raw[base + 32])
        } else {
            String::new()
        };
        out.push(PartEntry {
            offset: starting_offset,
            length: partition_length,
            ptype,
            number: partition_number,
        });
    }
    out
}

/// 分区列表（含 ESP/MSR 等无盘符分区），跳过长度 ≤ 0 的空槽
fn partition_list(h: HANDLE) -> Vec<(i64, i64, String, u32)> {
    let Some(raw) = ioctl_out(h, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, 16384) else {
        return Vec::new();
    };
    parse_layout(&raw)
        .into_iter()
        .map(|e| (e.offset, e.length, e.ptype, e.number))
        .collect()
}

/// 盘符 → 物理磁盘 + 偏移映射（仅 DRIVE_FIXED）
fn mappings() -> Vec<DriveMapping> {
    let mut maps = Vec::new();
    for d in drive_strings(&logical_drive_strings()) {
        // SAFETY: d 形如 "C:\"，NUL 结尾由函数内部处理
        if unsafe { GetDriveTypeW(wide(&d).as_ptr()) } != DRIVE_FIXED {
            continue;
        }
        let ltr = d.chars().next().unwrap().to_ascii_lowercase();
        let vol_path = format!("\\\\.\\{ltr}:");
        let Some((h, _restricted)) = open_device(&vol_path) else {
            continue;
        };
        if let Some(raw) = ioctl_out(h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, 1024) {
            let num_extents = read_u32(&raw, 0);
            if num_extents > 0 {
                // DISK_EXTENT 起始于偏移 8（对齐）：DiskNumber@8, StartingOffset@16
                let disk = read_u32(&raw, 8);
                let offset = read_i64(&raw, 16);
                maps.push(DriveMapping {
                    letter: ltr,
                    disk,
                    offset,
                });
            }
        }
        // SAFETY: 句柄来自 open_device，必然有效
        unsafe {
            CloseHandle(h);
        }
    }
    maps
}

/// 读取系统盘符列表（`GetLogicalDriveStringsW` 的多字符串格式 `"C:\0D:\0"`）。
///
/// `GetLogicalDriveStringsW` 在缓冲区不足时返回**所需长度**（而不是错误），
/// 此时必须按需扩容重试 —— 否则盘符较多时会静默丢失全部盘符映射，
/// 导致所有分区都查不到盘符（分区列表的"盘符"列全空）。
fn logical_drive_strings() -> String {
    let mut size = 512usize;
    // 最多重试 4 次（每次按 API 返回的所需长度扩容，实际一次即可）
    for _ in 0..4 {
        let mut buf = vec![0u16; size];
        // SAFETY: buf 为 size 个宽字符的有效缓冲
        let n = unsafe { GetLogicalDriveStringsW(size as u32, buf.as_mut_ptr()) };
        if n == 0 {
            return String::new();
        }
        let n = n as usize;
        if n <= size {
            return buf[..n]
                .iter()
                .map(|&c| char::from_u32(c as u32).unwrap_or('\0'))
                .collect();
        }
        // 缓冲区不足：n 即所需长度，扩容（+2 留出双 NUL 结尾余量）后重试
        size = n + 2;
    }
    String::new()
}

/// 多字符串盘符列表 → 各个盘符路径（`"C:\"` / `"D:\"`），纯函数便于单测
fn drive_strings(text: &str) -> Vec<String> {
    text.split('\0')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 盘符的文件系统与分区总容量（失败 → ("RAW", 0)）
fn volume_info(ltr: char) -> (String, i64) {
    let root = format!("{}:\\", ltr.to_ascii_uppercase());
    let mut fs = vec![0u16; 256];
    // SAFETY: root NUL 结尾宽字符串，输出缓冲 256 宽字符
    let ok = unsafe {
        GetVolumeInformationW(
            wide(&root).as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs.as_mut_ptr(),
            256,
        )
    };
    let ft = if ok != 0 {
        let len = fs.iter().position(|&c| c == 0).unwrap_or(256);
        fs[..len]
            .iter()
            .map(|&c| char::from_u32(c as u32).unwrap_or('\0'))
            .collect::<String>()
    } else {
        "RAW".to_string()
    };
    let mut total: u64 = 0;
    // SAFETY: total 为有效 out 参数
    let ok2 = unsafe {
        GetDiskFreeSpaceExW(
            wide(&root).as_ptr(),
            std::ptr::null_mut(),
            &mut total as *mut u64,
            std::ptr::null_mut(),
        )
    };
    let sz = if ok2 != 0 { total as i64 } else { 0 };
    (ft, sz)
}

/// 字节数 → 人类可读容量（与 Python `_fmt` 分档一致）
pub fn fmt_size(n: i64) -> String {
    if n <= 0 {
        return "0B".to_string();
    }
    let gb = n as f64 / (1024f64.powi(3));
    if gb >= 1.0 {
        return format!("{gb:.2}GB");
    }
    let mb = n as f64 / (1024f64.powi(2));
    if mb >= 1.0 {
        return format!("{mb:.2}MB");
    }
    format!("{n}B")
}

/// Option<bool> 的 Python 文本表示（True/False/None）
fn py_bool(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "True",
        Some(false) => "False",
        None => "None",
    }
}

/// 主入口：获取所有物理磁盘完整信息（对应 GetDiskInfo.main()）。
///
/// 完成后置位 DISK_LOGGED，后续刷新静默（不再写磁盘明细日志）。
pub fn enumerate() -> Vec<(u32, DiskInfo)> {
    // 首次枚举标记（后续 5s 自动刷新静默，见模块顶部说明）
    let first_run = !DISK_LOGGED.load(Ordering::Relaxed);
    disk_debug!("GetDiskInfo.main() 开始");
    let mut result: Vec<(u32, DiskInfo)> = Vec::new();
    let maps = mappings();
    disk_debug!("盘符映射: {:?}", maps);

    for phys in 0..64u32 {
        let path = format!("\\\\.\\PhysicalDrive{phys}");
        let Some((h, restricted)) = open_device(&path) else {
            continue;
        };
        if restricted && !RESTRICTED_WARNED.swap(true, Ordering::Relaxed) {
            // 仅提示一次：普通用户可正常查看磁盘信息，但写操作（分区/格式化）仍需管理员
            log::warn!(
                "当前进程无管理员权限：磁盘信息已降级为只读属性权限获取（FILE_READ_ATTRIBUTES）；\
                 分区/格式化/引导修复等写操作仍需以管理员身份运行"
            );
        }
        // 过滤 USB / 可移动设备（BusType==7 或 RemovableMedia==1）
        let bt = bus_type(h);
        let removable = is_removable(h);
        if bt == BUS_TYPE_USB || removable {
            disk_debug!(
                "物理磁盘 {phys}: 总线类型={bt}, 可移动={}，跳过",
                py_bool(Some(removable))
            );
            // SAFETY: 句柄来自 open_device，必然有效
            unsafe { CloseHandle(h) };
            continue;
        }
        let model_name = model(h);
        let sz = size(h);
        let st = style(h);
        let parts = partition_list(h);
        let is_ssd = is_ssd(h, &model_name);
        // SAFETY: 句柄来自 open_device，必然有效
        unsafe { CloseHandle(h) };
        disk_debug!(
            "物理磁盘 {phys}: model='{}', size={sz}, style='{st}', is_ssd={}, bus={bt}",
            model_name,
            py_bool(is_ssd)
        );

        // 逐分区匹配盘符（同一物理盘 + 偏移差 < 1MB；取首个匹配）
        let mut partitions: Vec<(String, PartitionInfo)> = Vec::new();
        for (pi, (offset, length, ptype, number)) in parts.iter().enumerate() {
            let mut letter = '\0';
            let mut fmt = String::new();
            for m in &maps {
                if m.disk == phys && (m.offset - *offset).abs() < 1024 * 1024 {
                    letter = m.letter;
                    fmt = volume_info(m.letter).0;
                    break;
                }
            }
            let key = format!("{phys}:{pi}");
            partitions.push((
                key,
                PartitionInfo {
                    size: fmt_size(*length),
                    size_bytes: *length,
                    ptype: ptype.clone(),
                    format: fmt,
                    letter: if letter == '\0' {
                        String::new()
                    } else {
                        letter.to_string()
                    },
                    number: *number,
                },
            ));
        }

        // 备用：无分区但盘符映射中有该盘的盘符 → 用盘符合成分区记录
        if parts.is_empty() {
            for m in &maps {
                if m.disk == phys {
                    let (ft, vs) = volume_info(m.letter);
                    let key = format!("{phys}:{}", partitions.len());
                    partitions.push((
                        key,
                        PartitionInfo {
                            size: fmt_size(vs),
                            size_bytes: vs,
                            ptype: "Basic Data".to_string(),
                            format: ft,
                            letter: m.letter.to_string(),
                            number: 0,
                        },
                    ));
                }
            }
        }

        let used: i64 = parts.iter().map(|p| p.1).sum();
        let available = sz - used;

        disk_debug!("添加磁盘 {phys}: is_ssd={}", py_bool(is_ssd));
        result.push((
            phys,
            DiskInfo {
                model: model_name,
                size: fmt_size(sz),
                size_bytes: sz,
                style: st,
                available: fmt_size(available),
                available_bytes: available,
                physical_disk: phys,
                is_ssd,
                partitions,
            },
        ));
    }

    disk_debug!("GetDiskInfo.main() 完成，共 {} 个磁盘", result.len());
    // 一个物理磁盘都没枚举到：必须让用户看见原因，而不是显示一个空列表。
    // 只在首次枚举时提示，避免 5s 自动刷新刷屏。
    if result.is_empty() && first_run {
        log::warn!(
            "未枚举到任何物理磁盘：可能是当前进程权限不足（磁盘读权限被拒绝，\
             且只读属性权限同样不可用）或磁盘被其它程序独占"
        );
    }
    // 首次启动刷新完成，关闭后续刷新的磁盘详情日志
    DISK_LOGGED.store(true, Ordering::Relaxed);
    result
}

// ======================== JSON dump（--dump-disks，对照 GetDiskInfo.py 末尾 json.dumps） ========================

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 磁盘枚举结果 → 与 `python GetDiskInfo.py` 相同结构的 JSON 文本（indent=4）。
pub fn to_json(disks: &[(u32, DiskInfo)]) -> String {
    let mut s = String::from("{\n");
    for (i, (_, d)) in disks.iter().enumerate() {
        s.push_str(&format!("    \"{}\": {{\n", d.physical_disk));
        s.push_str(&format!("        \"型号\": \"{}\",\n", json_escape(&d.model)));
        s.push_str(&format!("        \"容量\": \"{}\",\n", json_escape(&d.size)));
        s.push_str(&format!("        \"容量_bytes\": {},\n", d.size_bytes));
        s.push_str(&format!(
            "        \"分区类型\": \"{}\",\n",
            json_escape(&d.style)
        ));
        s.push_str(&format!(
            "        \"可用空间\": \"{}\",\n",
            json_escape(&d.available)
        ));
        s.push_str(&format!("        \"可用空间_bytes\": {},\n", d.available_bytes));
        s.push_str(&format!("        \"physical_disk\": {},\n", d.physical_disk));
        match d.is_ssd {
            Some(v) => s.push_str(&format!("        \"is_ssd\": {v},\n")),
            None => s.push_str("        \"is_ssd\": null,\n"),
        }
        if d.partitions.is_empty() {
            // Python: 无分区时 "分区": 0
            s.push_str("        \"分区\": 0\n");
        } else {
            s.push_str("        \"分区\": {\n");
            for (j, (key, p)) in d.partitions.iter().enumerate() {
                s.push_str(&format!("            \"{key}\": {{\n"));
                s.push_str(&format!(
                    "                \"size\": \"{}\",\n",
                    json_escape(&p.size)
                ));
                s.push_str(&format!("                \"size_bytes\": {},\n", p.size_bytes));
                s.push_str(&format!(
                    "                \"type\": \"{}\",\n",
                    json_escape(&p.ptype)
                ));
                s.push_str(&format!(
                    "                \"format\": \"{}\",\n",
                    json_escape(&p.format)
                ));
                s.push_str(&format!(
                    "                \"letter\": \"{}\",\n",
                    json_escape(&p.letter)
                ));
                s.push_str(&format!("                \"number\": {}\n", p.number));
                s.push_str("            }");
                if j + 1 == d.partitions.len() {
                    s.push('\n');
                } else {
                    s.push_str(",\n");
                }
            }
            s.push_str("        }\n");
        }
        s.push_str("    }");
        if i + 1 == disks.len() {
            s.push('\n');
        } else {
            s.push_str(",\n");
        }
    }
    s.push('}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 测试辅助：构造原始缓冲区 ----------

    /// 构造 STORAGE_DEVICE_DESCRIPTOR 原始字节（VendorIdOffset@12、ProductIdOffset@16、
    /// RemovableMedia@9、BusType@28）
    fn make_descriptor(vendor: &str, product: &str, bus: u32, removable: bool) -> Vec<u8> {
        let mut raw = vec![0u8; 40];
        raw[9] = u8::from(removable);
        raw[28..32].copy_from_slice(&bus.to_le_bytes());
        let mut vendor_off = 0u32;
        if !vendor.is_empty() {
            vendor_off = raw.len() as u32;
            raw.extend_from_slice(vendor.as_bytes());
            raw.push(0);
        }
        let mut product_off = 0u32;
        if !product.is_empty() {
            product_off = raw.len() as u32;
            raw.extend_from_slice(product.as_bytes());
            raw.push(0);
        }
        raw[12..16].copy_from_slice(&vendor_off.to_le_bytes());
        raw[16..20].copy_from_slice(&product_off.to_le_bytes());
        raw
    }

    /// 构造 DRIVE_LAYOUT_INFORMATION_EX 原始字节（头 48 + 每条 144）
    fn make_layout(style: u32, parts: &[(i64, i64, u32, [u8; 16])]) -> Vec<u8> {
        let mut raw = vec![0u8; DRIVE_LAYOUT_HEADER_SIZE + parts.len() * PART_ENTRY_SIZE];
        raw[0..4].copy_from_slice(&style.to_le_bytes());
        raw[4..8].copy_from_slice(&(parts.len() as u32).to_le_bytes());
        for (i, (offset, length, number, guid)) in parts.iter().enumerate() {
            let base = DRIVE_LAYOUT_HEADER_SIZE + i * PART_ENTRY_SIZE;
            raw[base + 8..base + 16].copy_from_slice(&offset.to_le_bytes());
            raw[base + 16..base + 24].copy_from_slice(&length.to_le_bytes());
            raw[base + 24..base + 28].copy_from_slice(&number.to_le_bytes());
            raw[base + 32..base + 48].copy_from_slice(guid);
        }
        raw
    }

    // ---------- 纯逻辑单测 ----------

    /// 容量分档与 Python `_fmt` 一致
    #[test]
    fn fmt_size_matches_python() {
        assert_eq!(fmt_size(0), "0B");
        assert_eq!(fmt_size(-5), "0B");
        assert_eq!(fmt_size(500_107_862_016), "465.76GB");
        assert_eq!(fmt_size(524_288_000), "500.00MB");
        assert_eq!(fmt_size(999), "999B");
        assert_eq!(fmt_size(1024 * 1024), "1.00MB");
        // mb < 1 → 直接字节数
        assert_eq!(fmt_size(1024 * 1024 - 1), "1048575B");
    }

    /// MBR 类型名与未知格式 `0xNN` 对照 `_MBR_TYPES`
    #[test]
    fn mbr_type_names() {
        assert_eq!(mbr_type_name(0x07), "NTFS");
        assert_eq!(mbr_type_name(0xEE), "GPT Protective");
        assert_eq!(mbr_type_name(0x0B), "FAT32");
        assert_eq!(mbr_type_name(0x83), "0x83");
    }

    /// 容量取值优先级：LENGTH_INFO → GEOMETRY → 0（← Python `_size`）
    #[test]
    fn pick_size_priority() {
        assert_eq!(pick_size(Some(100), Some(200)), 100);
        assert_eq!(pick_size(None, Some(200)), 200);
        assert_eq!(pick_size(None, None), 0);
        // LENGTH_INFO 成功但值为 0 时仍以它为准（与 Python 直接 return 一致）
        assert_eq!(pick_size(Some(0), Some(200)), 0);
    }

    /// STORAGE_DEVICE_DESCRIPTOR 偏移解析（← Python `STORAGE_DEVICE_DESCRIPTOR`）
    #[test]
    fn device_descriptor_offsets() {
        let raw = make_descriptor("SAMSUNG", "SAMSUNG SSD 860", 11, false);
        let d = parse_device_descriptor(&raw);
        assert_eq!(d.vendor, "SAMSUNG");
        assert_eq!(d.product, "SAMSUNG SSD 860");
        assert_eq!(d.bus_type, 11);
        assert!(!d.removable);

        // 无厂商（VendorIdOffset=0）时厂商为空串
        let raw = make_descriptor("", "ZT00D212", 17, false);
        let d = parse_device_descriptor(&raw);
        assert_eq!(d.vendor, "");
        assert_eq!(d.product, "ZT00D212");
        assert_eq!(d.bus_type, 17);

        // 可移动介质：RemovableMedia@9
        let raw = make_descriptor("", "USB DISK", BUS_TYPE_USB, true);
        let d = parse_device_descriptor(&raw);
        assert!(d.removable);
        assert_eq!(d.bus_type, BUS_TYPE_USB);

        // 空缓冲区不得 panic
        let d = parse_device_descriptor(&[]);
        assert_eq!(d.vendor, "");
        assert_eq!(d.product, "");
        assert_eq!(d.bus_type, 0);
        assert!(!d.removable);
    }

    /// GPT 布局解析（← Python `_partition_list`）
    #[test]
    fn parse_layout_gpt() {
        let raw = make_layout(
            1,
            &[
                (2_097_152, 943_718_400, 1, GPT_ESP),
                (945_815_552, 16_777_216, 2, GPT_MSR),
                (962_592_768, 214_749_413_376, 3, GPT_BASIC),
                (215_712_006_144, 296_397_118_976, 4, GPT_RECOVERY),
            ],
        );
        let parts = parse_layout(&raw);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].ptype, "ESP");
        assert_eq!(parts[0].offset, 2_097_152);
        assert_eq!(parts[0].length, 943_718_400);
        assert_eq!(parts[0].number, 1);
        assert_eq!(parts[1].ptype, "MSR");
        assert_eq!(parts[2].ptype, "Basic Data");
        assert_eq!(parts[3].ptype, "Recovery");
        assert_eq!(parts[3].number, 4);
    }

    /// MBR 布局解析 + 长度 ≤ 0 的空槽跳过（← Python `_partition_list`）
    #[test]
    fn parse_layout_mbr_skips_empty() {
        let mut guid = [0u8; 16];
        guid[0] = 0x07; // MBR NTFS 类型字节
        let raw = make_layout(0, &[(1_048_576, 0, 1, guid), (1_048_576, 104_857_600, 2, guid)]);
        let parts = parse_layout(&raw);
        assert_eq!(parts.len(), 1, "长度为 0 的空槽应被跳过");
        assert_eq!(parts[0].ptype, "NTFS");
        assert_eq!(parts[0].length, 104_857_600);
        assert_eq!(parts[0].number, 2);
    }

    /// 缓冲区截断时按实际长度截断解析，不得越界 panic
    #[test]
    fn parse_layout_truncated_is_safe() {
        let raw = make_layout(1, &[(0, 100, 1, GPT_ESP)]);
        let parts = parse_layout(&raw[..60]);
        assert!(parts.is_empty(), "条目不完整时应跳过");
        assert!(parse_layout(&[]).is_empty());
    }

    /// 多字符串盘符列表解析（`GetLogicalDriveStringsW` 的 "C:\0D:\0" 格式）
    #[test]
    fn drive_strings_parsing() {
        assert_eq!(
            drive_strings("C:\\\0D:\\\0"),
            vec!["C:\\".to_string(), "D:\\".to_string()]
        );
        // 末尾双 NUL（API 返回值含结尾 NUL）不得产生空项
        assert_eq!(drive_strings("C:\\\0"), vec!["C:\\".to_string()]);
        // 空输入 / 全空 → 无盘符
        assert!(drive_strings("").is_empty());
        assert!(drive_strings("\0\0").is_empty());
    }

    /// 盘符列表读取不得 panic；本机必然存在 C 盘（固定磁盘）
    #[test]
    fn logical_drive_strings_is_usable() {
        let text = logical_drive_strings();
        let drives = drive_strings(&text);
        assert!(
            drives.iter().any(|d| d.starts_with('C')),
            "应枚举到 C 盘，实际: {drives:?}"
        );
    }

    // ---------- 真实设备（只读）冒烟测试 ----------

    /// 降级策略：GENERIC_READ 被拒（ERROR_ACCESS_DENIED）时降级为 FILE_READ_ATTRIBUTES
    ///
    /// 无物理磁盘的机器自动跳过。
    #[test]
    fn open_device_falls_back_to_attributes() {
        let path = "\\\\.\\PhysicalDrive0";
        let expected_restricted = match create_file(path, GENERIC_READ) {
            Some(h) => {
                // SAFETY: 句柄来自 create_file，必然有效
                unsafe { CloseHandle(h) };
                false
            }
            None => {
                // SAFETY: 紧接 CreateFileW 失败后读取最后错误码
                let err = unsafe { GetLastError() };
                if err != ERROR_ACCESS_DENIED {
                    return; // 设备不存在等其它原因 → 跳过
                }
                true
            }
        };
        let Some((h, restricted)) = open_device(path) else {
            panic!("降级打开应成功");
        };
        // SAFETY: 句柄来自 open_device，必然有效
        unsafe { CloseHandle(h) };
        assert_eq!(
            restricted, expected_restricted,
            "受限标记应与 GENERIC_READ 是否成功一致"
        );
    }

    /// 容量回落路径：GEOMETRY(FILE_ANY_ACCESS) 无需管理员权限即可返回精确容量，
    /// 且与 LENGTH_INFO 的结果一致
    #[test]
    fn geometry_fallback_matches_length_info() {
        let path = "\\\\.\\PhysicalDrive0";
        let Some((h, _)) = open_device(path) else {
            return; // 无物理磁盘 → 跳过
        };
        let geo = ioctl_out(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, 512).map(|raw| read_i64(&raw, 24));
        let len = ioctl_out(h, IOCTL_DISK_GET_LENGTH_INFO, 512).map(|raw| read_i64(&raw, 0));
        // SAFETY: 句柄来自 open_device，必然有效
        unsafe { CloseHandle(h) };

        let geo = geo.expect("GEOMETRY 查询应成功（FILE_ANY_ACCESS，普通用户亦可）");
        assert!(geo > 0, "容量必须为正数");
        if let Some(len) = len {
            assert_eq!(len, geo, "两种控制码得到的容量必须一致");
        }
    }

    /// 枚举不得 panic；普通用户（无管理员权限）下也必须能枚举出磁盘
    #[test]
    fn enumerate_does_not_panic() {
        let disks = enumerate();
        for (phys, d) in &disks {
            assert_eq!(*phys, d.physical_disk);
            assert!(!d.model.is_empty());
            assert!(!d.size.is_empty());
        }
    }

    /// 打印与 `python GetDiskInfo.py` 同结构的 JSON（对照 `--dump-disks` 调试开关）
    ///
    /// 手动运行：`cargo test dump_disks_json -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn dump_disks_json() {
        let disks = enumerate();
        println!("{}", to_json(&disks));
    }

    /// JSON 转义与结构关键点（空分区 = 0、is_ssd null），并用 serde 严格解析防尾逗号回归
    #[test]
    fn json_dump_shape() {
        let disks = vec![(
            0,
            DiskInfo {
                model: "Test \"盘\"".to_string(),
                size: "1.00GB".to_string(),
                size_bytes: 1024 * 1024 * 1024,
                style: "GPT".to_string(),
                available: "0B".to_string(),
                available_bytes: 0,
                physical_disk: 0,
                is_ssd: None,
                partitions: vec![],
            },
        )];
        let json = to_json(&disks);
        assert!(json.contains("\"型号\": \"Test \\\"盘\\\"\""));
        assert!(json.contains("\"分区\": 0"));
        assert!(json.contains("\"is_ssd\": null"));
        assert!(json.starts_with("{\n"));
        assert!(json.ends_with('}'));
        serde_json::from_str::<serde_json::Value>(&json).expect("空分区 JSON 必须严格合法");

        // 多分区：字段逗号 / 分区分隔逗号不得出现在 number 之后
        let p = |letter: &str, number: u32| PartitionInfo {
            size: "1.00GB".into(),
            size_bytes: 1024 * 1024 * 1024,
            ptype: "Basic Data".into(),
            format: "NTFS".into(),
            letter: letter.into(),
            number,
        };
        let mut d = disks[0].1.clone();
        d.partitions = vec![
            ("0:0".to_string(), p("c", 1)),
            ("0:1".to_string(), p("d", 2)),
        ];
        let json = to_json(&[(0, d)]);
        let v: serde_json::Value = serde_json::from_str(&json).expect("多分区 JSON 必须严格合法");
        let parts = &v["0"]["分区"];
        assert_eq!(parts["0:0"]["number"], 1);
        assert_eq!(parts["0:1"]["letter"], "d");
    }
}
