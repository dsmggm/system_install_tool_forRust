//! GUI 共享状态与后台线程 → 主线程消息类型（← `gui.py` App 实例字段）
//!
//! tkinter 版通过 `root.after(0, cb)` 把后台线程结果投递到主线程；
//! fltk 版用 `app::channel()` + `awake()` 达到同样效果：
//! 后台线程 `Sender<Message>::send()` 后 `awake()` 唤醒主循环，
//! 主循环 `Receiver::recv()` 取消息并执行（`ui::run` 中）。

use crate::app::disk_info::DiskInfo;

/// 后台线程 → 主线程消息（覆盖 gui.py 中所有 `root.after(...)` 回调）
pub enum Message {
    /// `_display_disk_info(disk_info, silent)` — 磁盘枚举结果就绪
    DiskInfoReady {
        disks: Vec<(u32, DiskInfo)>,
        silent: bool,
    },
    /// 安装项目中的普通状态/进度回调（`_update_install_progress`）
    Progress(String, i32),
    /// 安装/分区主流程完成（`_on_install_complete(ok, output)`）。
    /// 第 3 项：安装目标系统分区盘符（→ `_last_install_windows_partition`，
    /// gui.py 2085/2087 行——WIM 流程在安装前记录，供安装成功后的优化阶段
    /// 部署 unattend.xml / 复制工具使用；失败或 GHO 流程为 None）。
    InstallComplete(bool, String, Option<String>),
    /// 安装成功、优化结束后的展示（`_show_install_success(warning)`）
    InstallSuccess(Option<String>),
    /// 分区执行完成后重刷磁盘树（`_post_partition_refresh`）
    PostPartitionRefresh(Vec<(u32, DiskInfo)>),
}

/// 规划分区暂存结果（← OneClickPartitionDialog.result 的字典）
#[derive(Debug, Clone)]
pub struct PendingPartition {
    pub disk_physical: i64,
    pub disk_idx: u32,
    pub style: String,
    pub esp_enabled: bool,
    pub esp_size_mb: i64,
    pub data_parts: Vec<crate::app::scheme::bat::DataPart>,
}

/// 分区对话框返回结果（← `dialog_data` / `result`）
#[derive(Debug, Clone)]
pub struct PartitionDialogResult {
    pub style: String,
    pub esp_enabled: bool,
    pub esp_size_mb: i64,
    pub data_parts: Vec<crate::app::scheme::bat::DataPart>,
}
