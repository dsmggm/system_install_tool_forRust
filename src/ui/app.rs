//! 主窗口与全局状态（← `gui.py::App`）
//!
//! 布局、文案、控件状态规则 1:1 复刻（文档 §7）。fltk 布局用 Flex 近似，
//! 不追求像素级一致（用户决定）。
//!
//! 线程模型：GUI 单线程；后台耗时操作（磁盘枚举 / diskpart / DISM）跑
//! `std::thread`，结果经 `app::channel` 的 `Message` 回传主循环处理。

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use fltk::button::{Button, CheckButton, RadioRoundButton};
use fltk::enums::FrameType;
use fltk::frame::Frame;
use fltk::group::{Flex, Group};
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::misc::Progress;
use fltk::prelude::*;
use crate::ui::select_choice_exact;
use fltk::tree::Tree;
use fltk::window::DoubleWindow;
use fltk::{app, dialog};

use crate::app::disk_info::DiskInfo;
use crate::app::diskpart::{self, DataPartSpec};
use crate::app::install::boot;
use crate::app::install::iso;
use crate::app::scheme::bat::{
    build_combined_bat, parse_plan_metadata, InstallSettings, PartitionPlan, SchemeData,
};
use crate::config::settings::AppConfig;

use super::ghost::relative_gho_src;
use super::log_panel::LogPanel;
use super::state::{Message, PendingPartition};
use super::theme;
use super::{app_icon, one_click, restart};

/// 自动刷新间隔（秒），对应 `App.AUTO_REFRESH_INTERVAL`
const AUTO_REFRESH_INTERVAL: f64 = 5.0;

/// 磁盘树中每个节点的元数据
///
/// 【去掉选中功能】树已是纯展示列表，分区节点不再需要元数据
/// （"一键分区"按钮直接绑定物理盘号，预览节点用 `Preview` 标记）。
#[derive(Clone)]
enum NodeKind {
    Disk {
        /// 物理磁盘号（行内"一键分区"按钮回收/预览定位用它）
        phys: u32,
    },
    Preview,
}

struct AppUi {
    win: DoubleWindow,
    /// 根列 Flex（承载 toolbar / main / log_group，用于日志显隐后重排）。
    root: Flex,
    /// 主体行 Flex（左列 + 右列的容器，窗口宽度变化时重设左列宽度）。
    main_col: Flex,
    /// 左侧磁盘/分区列（窗口宽度变化时按比例调整宽度）。
    left_col: Flex,
    /// 右侧安装列 Flex（承载各分组，用于模式切换时折叠/展开分组）。
    right_col: Flex,
    /// 「选项」分组 Flex（承载 WIM/GHO 两套选项，用于模式切换时折叠/展开）。
    options_group: Flex,
    // 工具栏
    restore_cb: CheckButton,
    countdown_label: Frame,
    log_toggle_btn: Button,
    save_scheme_btn: Button,
    load_scheme_btn: Button,
    // 磁盘树
    tree: Tree,
    // 每个硬盘行内的"一键分区"按钮（FLTK tree-as-container）
    disk_buttons: Vec<Button>,
    // 安装源
    source_path_in: Input,
    browse_source_btn: Button,
    image_choice: Choice,
    // 目标分区
    target_group: Flex,
    disk_choice: Choice,
    partition_preview: Frame,
    // GHO 面板
    gho_group: Flex,
    gho_mode_disk_rb: RadioRoundButton,
    gho_mode_part_rb: RadioRoundButton,
    gho_disk_group: Flex,
    gho_disk_choice: Choice,
    gho_part_group: Flex,
    gho_part_choice: Choice,
    ghost_path_in: Input,
    // 选项（WIM）
    wim_options_group: Flex,
    format_cb: CheckButton,
    copy_tools_cb: CheckButton,
    tools_path_in: Input,
    browse_tools_btn: Button,
    deploy_cb: CheckButton,
    admin_user_in: Input,
    defender_cb: CheckButton,
    firewall_cb: CheckButton,
    repair_boot_cb: CheckButton,
    restart_cb: CheckButton,
    // 选项（GHO）
    gho_options_group: Flex,
    gho_restart_cb: CheckButton,
    gho_repair_boot_cb: CheckButton,
    // 底部
    install_btn: Button,
    progress: Progress,
    current_scheme_label: Frame,
    // 日志
    log_group: Flex,
    log_panel: LogPanel,
}

/// 主应用状态（← `App` 实例字段）
pub struct App {
    ui: AppUi,
    cfg: AppConfig,

    // 磁盘数据：物理磁盘号 → 信息
    disk_data: Vec<(u32, DiskInfo)>,
    // 主磁盘下拉框每项对应的物理磁盘号（与 disk_choice 顺序一致）
    disk_combo_index: Vec<u32>,
    // 树节点元数据：fltk 树路径 → 节点信息
    node_data: HashMap<String, NodeKind>,

    is_gho_mode: bool,
    is_installing: bool,
    install_cancelled: Arc<AtomicBool>,

    // 预览方案（多硬盘）
    pending: Vec<PendingPartition>,

    // 方案跟踪
    scheme_active: bool,
    applying_scheme: bool,
    current_scheme: String,

    // GHO 方案暂存目标（物理盘号 / 实际分区号）
    scheme_gho_disk: Option<String>,
    scheme_gho_partition: Option<String>,

    // 自动还原倒计时
    countdown_id: Option<app::TimeoutHandle>,
    countdown_remaining: i64,
    countdown_running: bool,

    // 安装后优化所需
    last_install_windows_letter: Option<String>,
    last_install_admin_username: String,
    last_install_image_index: i64,

    current_wim_path: Option<String>,
    current_mount_path: Option<String>,

    first_load: bool,

    /// 展开日志时窗口向下增高的像素数（收起时按相同值缩回）。
    log_grown: i32,
}

impl App {
    // ==================== 构建 ====================

    pub fn new(config: AppConfig) -> Self {
        let mut app = app::App::default();
        app.set_scheme(app::Scheme::Gtk);
        // 统一字体 / 强调色 / 全局底色（必须在创建控件前调用）
        theme::init();

        // 窗口尺寸按屏幕工作区推算，并**始终小于屏幕**（低分辨率设备也能完整显示）：
        //   - 理想尺寸：屏幕宽 -140 / 高 -240，再限制在 [760,1240] × [560,860]；
        //   - 若屏幕本身很小，则取「屏幕 - 边距」，绝不超出可视范围；
        //   - 初始位置居中，避免 x=100/y=100 把窗口顶出屏幕底部。
        let (screen_w, screen_h) = app::screen_size();
        let sw = screen_w as i32;
        let sh = screen_h as i32;
        let avail_w = (sw - 80).max(360);
        let avail_h = (sh - 120).max(300);
        let win_w = (sw - 140).clamp(760, 1240).min(avail_w);
        let win_h = (sh - 240).clamp(560, 860).min(avail_h);
        let win_x = ((sw - win_w) / 2).max(0);
        let win_y = ((sh - win_h) / 2).max(0);

        let mut win = DoubleWindow::new(win_x, win_y, win_w, win_h, &*crate::app_base_title());
        win.make_resizable(true);
        // 最小尺寸同样按实际窗口收紧：小屏上不允许再放大到超出屏幕。
        // 宽度下限 760 时右列仍有 401px；屏幕本身更窄时只能取屏幕可用宽度。
        win.size_range(win_w.min(760), win_h.min(560), 0, 0);
        win.set_color(theme::rgb(theme::BG));
        // 设置窗口图标（标题栏 + 任务栏）
        win.set_icon(Some(app_icon()));
        win.begin();

        // ---------- 根布局（纵向：页头 / 主体 / 页脚 / 日志）----------
        let mut root = Flex::default().column().size_of_parent();
        root.set_margin(0);
        root.set_spacing(0);
        root.set_color(theme::rgb(theme::BG));

        // ===== 页头：自动还原开关（左）/ 倒计时 / 日志开关 =====
        let header_ui = super::layout::create_header(&config);
        let restore_cb = header_ui.restore_cb;
        let countdown_label = header_ui.countdown_label;
        let log_toggle_btn = header_ui.log_toggle_btn;
        root.fixed(&header_ui.header, 48);

        // ===== 主体：左（磁盘与分区）/ 右（安装设置卡片列）=====
        let mut main = Flex::default().row();
        main.set_margin(14);
        main.set_spacing(14);
        main.set_color(theme::rgb(theme::BG));

        // —— 左：磁盘与分区卡片 ——
        let disk_tree_ui = super::layout::create_disk_tree();
        let left = disk_tree_ui.left_col;
        let tree = disk_tree_ui.tree;
        // 左列宽度随窗口收缩：右侧卡片列需要至少 401px（最宽一行固定控件
        // 381 + 卡片内边距 20），窗口变窄时先压缩左侧磁盘树，避免右侧
        // 复选框被裁切；窗口够宽时左列固定 470px。
        let left_w = (win_w - 443).clamp(300, 470);
        main.fixed(&left, left_w);

        // —— 右：设置卡片列 ——
        let mut right = Flex::default().column();
        theme::style_plain(&mut right);
        right.set_spacing(10);

        // —— 安装源（高度 100 = 内边距 20 + 标题 18 + 间距 10 + 2 行 × 26）——
        let install_source_ui = super::layout::create_install_source();
        let source_group = install_source_ui.source_group;
        let source_path_in = install_source_ui.source_path_in;
        let browse_source_btn = install_source_ui.browse_source_btn;
        let image_choice = install_source_ui.image_choice;
        right.fixed(&source_group, 100);

        // —— 目标分区（高度 98 = 内边距 20 + 标题 18 + 间距 10 + 26 + 24）——
        let target_partition_ui = super::layout::create_target_partition();
        let target_group = target_partition_ui.target_group;
        let disk_choice = target_partition_ui.disk_choice;
        let partition_preview = target_partition_ui.partition_preview;
        right.fixed(&target_group, 98);

        // —— GHO 还原选项（高度 132 = 内边距 20 + 标题 18 + 3 行 × 26 + 2 间距 10）
        //    「目标硬盘」与「目标分区」两行互斥显示，同一时刻只有 3 行可见。
        let gho_options_ui = super::layout::create_gho_options();
        let gho_group = gho_options_ui.gho_group;
        let gho_mode_disk_rb = gho_options_ui.gho_mode_disk_rb;
        let gho_mode_part_rb = gho_options_ui.gho_mode_part_rb;
        let gho_disk_group = gho_options_ui.gho_disk_group;
        let gho_disk_choice = gho_options_ui.gho_disk_choice;
        let gho_part_group = gho_options_ui.gho_part_group;
        let gho_part_choice = gho_options_ui.gho_part_choice;
        let ghost_path_in = gho_options_ui.ghost_path_in;
        right.fixed(&gho_group, 132);

        // —— 选项（WIM 高 162 = 内边距 20 + 标题 18 + 间距 5 + 4 行组 119）——
        let mut options_group = Flex::default().column();
        theme::style_card(&mut options_group);
        let options_title = theme::title("选项");
        options_group.fixed(&options_title, 18);

        let wim_options_ui = super::layout::create_wim_options(&config);
        let wim_options_group = wim_options_ui.wim_options_group;
        let format_cb = wim_options_ui.format_cb;
        let repair_boot_cb = wim_options_ui.repair_boot_cb;
        let restart_cb = wim_options_ui.restart_cb;
        let defender_cb = wim_options_ui.defender_cb;
        let firewall_cb = wim_options_ui.firewall_cb;
        let copy_tools_cb = wim_options_ui.copy_tools_cb;
        let tools_path_in = wim_options_ui.tools_path_in;
        let browse_tools_btn = wim_options_ui.browse_tools_btn;
        let deploy_cb = wim_options_ui.deploy_cb;
        let admin_user_in = wim_options_ui.admin_user_in;

        // GHO 模式下的选项（与 WIM 选项互斥显示）
        let mut gho_options_group = Flex::default().column();
        gho_options_group.set_frame(FrameType::NoBox);
        gho_options_group.set_color(theme::rgb(theme::CARD));
        gho_options_group.set_margins(0, 0, 0, 0);
        gho_options_group.set_spacing(5);
        let mut gr_row = Flex::default().row();
        gr_row.set_color(theme::rgb(theme::CARD));
        let mut gho_restart_cb = CheckButton::default().with_label("还原后自动重启");
        gho_restart_cb.set_checked(config.auto_restart);
        theme::style_check(&mut gho_restart_cb);
        gr_row.fixed(&gho_restart_cb, 140);
        let mut gho_repair_boot_cb = CheckButton::default().with_label("分区还原后修复引导");
        // 默认勾选：分区还原后 GHO 只保留原 MBR/引导扇区，BCD 通常已失效，
        // 与 WIM「修复引导」同口径——需要时才修，但默认开启。
        gho_repair_boot_cb.set(true);
        theme::style_check(&mut gho_repair_boot_cb);
        gr_row.fixed(&gho_repair_boot_cb, 168);
        let mut gr_spacer = Frame::default();
        gr_spacer.set_color(theme::rgb(theme::CARD));
        gr_row.fixed(&gr_spacer, 1);
        gr_row.end();
        gho_options_group.fixed(&gr_row, 26);
        gho_options_group.end();
        gho_options_group.hide();

        options_group.fixed(&wim_options_group, 119);
        options_group.fixed(&gho_options_group, 26);
        let _options_spacer = Frame::default();
        options_group.end();
        right.fixed(&options_group, 162);

        // 占位：吸收右侧剩余空间（避免最后一张卡片被拉伸）
        let _right_spacer = Frame::default();
        right.end();
        main.end();

        // ===== 页脚：进度条 + 当前方案 + 方案操作 + 开始安装 =====
        // 高度 62 = 上下内边距 18 + 进度条 8 + 间距 6 + 按钮行 30
        let footer_ui = super::layout::create_footer();
        let progress = footer_ui.progress;
        let _scheme_head = footer_ui.scheme_head;
        let current_scheme_label = footer_ui.current_scheme_label;
        let save_scheme_btn = footer_ui.save_scheme_btn;
        let load_scheme_btn = footer_ui.load_scheme_btn;
        let install_btn = footer_ui.install_btn;
        root.fixed(&footer_ui.footer, 62);

        // ---------- 日志面板（默认隐藏）----------
        // 高度 140 = 上下内边距 18 + 标题 18 + 间距 4 + 文本区 100
        let mut log_group = Flex::default().column();
        log_group.set_frame(FrameType::FlatBox);
        log_group.set_color(theme::rgb(theme::CARD));
        log_group.set_margins(12, 8, 12, 10);
        log_group.set_spacing(4);
        let log_title = theme::title("运行日志");
        log_group.fixed(&log_title, 18);
        let log_panel = LogPanel::new(0, 0, 0, 0);
        log_group.end();
        log_group.hide();
        root.fixed(&log_group, 0);

        root.end();
        win.end();
        win.resizable(&root);

        let mut this = App {
            ui: AppUi {
                win,
                root,
                main_col: main,
                left_col: left,
                right_col: right,
                options_group,
                restore_cb,
                countdown_label,
                log_toggle_btn,
                save_scheme_btn,
                load_scheme_btn,
                tree,
                disk_buttons: Vec::new(),
                source_path_in,
                browse_source_btn,
                image_choice,
                target_group,
                disk_choice,
                partition_preview,
                gho_group,
                gho_mode_disk_rb,
                gho_mode_part_rb,
                gho_disk_group,
                gho_disk_choice,
                gho_part_group,
                gho_part_choice,
                ghost_path_in,
                wim_options_group,
                format_cb,
                copy_tools_cb,
                tools_path_in,
                browse_tools_btn,
                deploy_cb,
                admin_user_in,
                defender_cb,
                firewall_cb,
                repair_boot_cb,
                restart_cb,
                gho_options_group,
                gho_restart_cb,
                gho_repair_boot_cb,
                install_btn,
                progress,
                current_scheme_label,
                log_group,
                log_panel,
            },
            cfg: config,
            disk_data: Vec::new(),
            disk_combo_index: Vec::new(),
            node_data: HashMap::new(),
            is_gho_mode: false,
            is_installing: false,
            install_cancelled: Arc::new(AtomicBool::new(false)),
            pending: Vec::new(),
            scheme_active: false,
            applying_scheme: false,
            current_scheme: String::new(),
            scheme_gho_disk: None,
            scheme_gho_partition: None,
            countdown_id: None,
            countdown_remaining: 0,
            countdown_running: false,
            last_install_windows_letter: None,
            last_install_admin_username: "admin".to_string(),
            last_install_image_index: 1,
            current_wim_path: None,
            current_mount_path: None,
            first_load: true,
            log_grown: 0,
        };
        this.wire_callbacks();
        this.restore_from_config_at_startup();
        // 归一化初始模式的分组显隐/高度（托盘默认 WIM 模式）。
        this.set_gho_mode(this.is_gho_mode);
        // FLTK 的 Flex 仅在收到 resize 事件时才会布局嵌套子控件；
        // 首次显示前显式触发一次，避免所有控件停留在 (0,0,0,0)。
        this.ui.root.layout();
        this.ui.win.show();
        this.ui.root.layout();
        this
    }

    /// 绑定所有控件回调（一次性）。
    ///
    /// 「一键分区」按钮改为每个硬盘行内各挂一个（FLTK tree-as-container，
    /// 见 `rebuild_tree`），回调在按钮创建时逐个绑定；树选择回调随旧的
    /// 单一按钮一并移除（选中态仅用于刷新结果恢复，不需要响应）。
    fn wire_callbacks(&mut self) {
        // 窗口尺寸变化时重算左列宽度：左列随窗口收缩（最小 300），
        // 右侧卡片列始终保留约 470px，避免用户把窗口拉到最小后右列被裁切。
        let mut main_col = self.ui.main_col.clone();
        let left_col = self.ui.left_col.clone();
        let root = self.ui.root.clone();
        self.ui.win.resize_callback(move |w, _, _, _, _| {
            // 注意：必须在水平方向的 main（row）上设置 fixed，
            // 在左列（column）上调用只会固定子控件高度。
            main_col.fixed(&left_col, (w.w() - 443).clamp(300, 470));
            root.layout();
        });

        // 工具栏
        self.ui.log_toggle_btn.set_callback(|b| {
            // 克隆句柄进 App::with（闭包须 'static 以支持嵌套排队）
            let mut bc = b.clone();
            App::with(move |a| {
                let on = bc.label() == "隐藏日志";
                a.on_toggle_log_visible();
                bc.set_label(if on { "显示日志" } else { "隐藏日志" });
            });
        });
        self.ui.save_scheme_btn.set_callback(|_| App::with(|a| a.save_scheme()));
        self.ui.load_scheme_btn.set_callback(|_| App::with(|a| a.load_scheme("", false)));
        self.ui.restore_cb.set_callback(|_| App::with(|a| a.on_toggle_restore()));
        // 安装源
        self.ui.browse_source_btn.set_callback(|_| App::with(|a| a.on_browse_source()));
        // 路径字段已设为只读：只读 Fl_Input 仍会在鼠标释放时触发回调
        // （Fl_Input.cxx: if (readonly()) do_callback(FL_REASON_RELEASED)），
        // 若保留该回调，用户点击路径框就会静默作废已加载的方案并重新挂载镜像。
        // 路径现在只能由「浏览...」(on_browse_source) 与 apply_scheme 写入，
        // 二者均已显式调用 load_wim_info，故不再需要此回调。
        self.ui.image_choice.set_callback(|_| {
            App::with(|a| {
                if !a.applying_scheme {
                    a.invalidate_scheme();
                }
                a.update_install_button_state();
            })
        });
        // 目标分区
        self.ui.disk_choice.set_callback(|_| App::with(|a| a.on_disk_select()));
        // GHO
        self.ui.gho_mode_disk_rb.set_callback(|_| App::with(|a| a.on_gho_mode_changed()));
        self.ui.gho_mode_part_rb.set_callback(|_| App::with(|a| a.on_gho_mode_changed()));
        self.ui.gho_disk_choice.set_callback(|_| App::with(|a| a.on_gho_target_selected()));
        self.ui.gho_part_choice.set_callback(|_| App::with(|a| a.on_gho_target_selected()));
        self.ui.ghost_path_in.set_callback(|_| {
            App::with(|a| {
                if !a.applying_scheme {
                    a.invalidate_scheme();
                }
            })
        });
        // 选项
        self.ui.copy_tools_cb.set_callback(|_| App::with(|a| a.on_toggle_copy_tools()));
        self.ui.deploy_cb.set_callback(|_| App::with(|a| a.on_toggle_deploy_unattend()));
        self.ui.defender_cb.set_callback(|_| App::with(|a| a.on_toggle_disable_defender()));
        self.ui.firewall_cb.set_callback(|_| App::with(|a| a.on_toggle_disable_firewall()));
        self.ui.restart_cb.set_callback(|_| App::with(|a| a.on_toggle_auto_restart()));
        self.ui.gho_restart_cb.set_callback(|_| App::with(|a| a.on_toggle_auto_restart()));
        self.ui.browse_tools_btn.set_callback(|_| App::with(|a| a.on_browse_tools_source()));
        self.ui.tools_path_in.set_callback(|i| {
            let v = i.value();
            App::with(move |a| a.on_tools_source_path_changed(&v));
        });
        // 安装
        self.ui.install_btn.set_callback(|_| App::with(|a| a.on_install_system(false)));
    }

    // ==================== 启动流程（§8.1）====================

    fn restore_from_config_at_startup(&mut self) {
        let plan = self.cfg.plan_path.clone();
        if !plan.is_empty() && Path::new(&plan).exists() {
            log::info!("启动时自动加载方案: {plan}");
            self.load_scheme(&plan, true);
        } else if !plan.is_empty() {
            self.current_scheme = Path::new(&plan)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.ui.current_scheme_label.set_label(&self.current_scheme);
        }
        self.refresh_disk_info(false);
        self.schedule_next_refresh();
        // after(500ms) startup_check
        app::add_timeout3(0.5, |_| App::with(|a| a.startup_check()));
    }

    fn startup_check(&mut self) {
        if self.cfg.restore_enabled && !self.cfg.plan_path.is_empty() {
            log::info!(
                "启动检查: 自动还原已开启, 方案='{}', 启动倒计时",
                self.cfg.plan_path
            );
            self.start_countdown(self.cfg.countdown_sec);
            self.set_status("自动还原服务已开启");
            self.set_restore_state(true);
        } else {
            log::info!(
                "启动检查: 自动还原={}, 不启动倒计时",
                if self.cfg.restore_enabled {
                    "开启但无方案"
                } else {
                    "关闭"
                }
            );
            self.set_restore_state(false);
        }
    }

    // ==================== 工具栏状态 ====================

    fn set_status(&mut self, s: &str) {
        self.set_window_title(s);
    }

    /// 根据自动还原服务状态刷新右上角开关的文案与颜色：
    ///   - 运行中 → 绿色 `▶自动还原服务运行中`
    ///   - 已停止 → 红色 `⏸自动还原服务已停止`
    fn set_restore_state(&mut self, running: bool) {
        if running {
            self.ui.restore_cb.set_label("▶自动还原服务运行中");
            self.ui.restore_cb.set_label_color(theme::rgb(theme::SUCCESS));
        } else {
            self.ui.restore_cb.set_label("⏸自动还原服务已停止");
            self.ui.restore_cb.set_label_color(theme::rgb(theme::DANGER));
        }
    }

    /// 更新窗口标题：`硕科斯系统安装工具 V{version} - {status}`
    fn set_window_title(&mut self, status: &str) {
        let title = format!("{} - {}", crate::app_base_title(), status);
        self.ui.win.set_label(&title);
    }

    fn set_countdown_text(&mut self, s: &str) {
        self.ui.countdown_label.set_label(s);
    }

    // ==================== 自动刷新 ====================

    fn schedule_next_refresh(&mut self) {
        app::add_timeout3(AUTO_REFRESH_INTERVAL, |_| {
            App::with(|a| {
                a.refresh_disk_info(true);
                a.schedule_next_refresh();
            })
        });
    }

    fn refresh_disk_info(&mut self, silent: bool) {
        if !silent {
            log::info!("正在读取硬盘信息...");
            self.set_status("正在读取硬盘信息...");
        }
        let tx = super::tx();
        std::thread::spawn(move || {
            let disks = crate::app::disk_info::enumerate();
            tx.send(Message::DiskInfoReady { disks, silent });
        });
    }

    // ==================== 磁盘树 ====================

    fn display_disk_info(&mut self, disks: Vec<(u32, DiskInfo)>, silent: bool) {
        // 【数据不变则跳过重建】每 5s 的静默刷新若与上次结果完全一致，
        // 整棵树（含行内"一键分区"按钮）原样保留：
        // - 消除按钮销毁/重建与用户点击的竞态（撞上重建的点击会丢失）；
        // - 消除对话框打开期间嵌套重建删除回调栈上按钮的 use-after-free。
        // 非静默刷新（安装后/手动）仍无条件重建，保证界面与磁盘强同步。
        if silent && self.disk_data == disks {
            log::debug!("磁盘数据与上次一致，跳过树重建");
            return;
        }
        if self.is_gho_mode {
            self.refresh_gho_targets();
        }
        self.update_install_button_state();
        if !silent {
            self.set_status("就绪");
        }
        self.disk_data = disks;
        self.rebuild_tree();
    }

    fn rebuild_tree(&mut self) {
        // 重建会整棵清空，节点 iid 变化；先记下当前选中项的稳定标识，
        // 重建后恢复（← Python `_rebuild_tree` 的 selected_phys / selected_part_key，
        // gui.py:2696-2707 与 2771-2776）。否则每 5 秒一次的自动刷新会把选中
        // 态清掉，"一键分区"按钮看着是亮的但点了没反应。
        //
        // 【去掉选中功能】树已是纯展示（TreeSelect::None），不再有选中态
        // 需要跨重建恢复，快照/恢复逻辑已整段删除。
        // 上一轮挂在树行内的"一键分区"按钮是树的子控件，`Tree::clear()` 只删
        // 树条目不删子控件，必须先手动销毁，否则残留成幽灵按钮可点击。
        //
        // 【必须用 FLTK 的延迟删除】cfltk 的 `widget_delete` 是**立即** C++
        // `delete`（cfl_macros.h WIDGET_DEFINE），fltk-rs 公开的
        // `WidgetBase::delete`/`app::delete_widget` 都走它。而本函数常在按钮
        // 自己的回调栈内执行：一键分区「保存」后重建、对话框打开期间自动刷新
        // 触发重建——立即 delete 会把仍在执行 handle()/callback() 的按钮就地
        // 释放 → use-after-free（表现为对话框随机消失、后续点击失灵）。
        // `Fl::delete_widget()` 把指针交给 FLTK 垃圾箱，在当前回调分派结束后
        // 统一销毁，回调栈展开时对象仍然有效。
        for b in std::mem::take(&mut self.ui.disk_buttons) {
            unsafe {
                fltk_sys::fl::Fl_delete_widget(
                    b.as_widget_ptr() as *mut fltk_sys::fl::Fl_Widget
                );
            }
            // Rust 侧 wrapper 正常 drop：只释放 WidgetTracker 观察对象，
            // 不会碰已进入垃圾箱的 widget 本体。
        }
        // 行路径缓存与对齐树宽随之失效（行按钮已全部销毁）
        self.ui.tree.clear();
        self.node_data.clear();
        if self.disk_data.is_empty() {
            self.ui.tree.add("/未检测到硬盘信息");
            // 飞【Bug 1 修复】见 rebuild_tree 末尾：改树内容必须显式重绘
            self.ui.tree.redraw();
            self.ui.left_col.redraw();
            return;
        }
        // FLTK tree-as-container：必须先把树的组上下文设为当前（begin），
        // 按钮构造时才会成为树的子控件；先于 begin 创建会挂到别的组上。
        self.ui.tree.begin();
        // 阶段一：先建硬盘行（基础标签）与行内按钮；node_data 与分区子行挪到
        // 阶段二 —— 右对齐补格会改变硬盘行标签（= 树路径），见 align_disk_row_buttons。
        let mut rows: Vec<(u32, DiskInfo, String)> = Vec::new();
        for (disk_idx, info) in self.disk_data.clone() {
            let disk_type = match info.is_ssd {
                Some(true) => "（SSD）",
                Some(false) => "（HDD）",
                None => "",
            };
            let disk_text = format!(
                "磁盘 {disk_idx}: {} {disk_type}  [{}]",
                info.model, info.size
            );
            let disk_path = format!("/{disk_text}");
            self.ui.tree.add(&disk_path);
            // —— 每个硬盘行内一个“一键分区”按钮 ——
            // item_draw_mode 需为 LabelAndWidget：默认的 DrawDefault 会用子
            // 控件**替换**节点文本（磁盘名消失）；置 LabelAndWidget 后标签与
            // 控件同行并存，控件高度被强制等于行高（draw 处 resize 覆盖预设值）。
            // 按钮放在标签**之前**（左侧）：`set_widget` 把控件挂在标签左边，
            // 与 FLTK 的 LabelAndWidget 布局一致。
            let mut btn = theme::tree_button("一键分区");
            btn.set_size(64, 18);
            if self.is_installing {
                btn.deactivate();
            }
            theme::add_hover_effect(&mut btn, theme::PRIMARY_SOFT);
            // 物理盘号先复制出来：回调闭包须 'static（嵌套排队），不能借用循环变量
            let phys = info.physical_disk;
            btn.set_callback(move |_| App::with(move |a| a.on_one_click(disk_idx, phys)));
            if let Some(mut item) = self.ui.tree.find_item(&disk_path) {
                item.set_widget(&btn);
            }
            self.ui.disk_buttons.push(btn);
            rows.push((disk_idx, info, disk_path));
        }
        // 阶段二：node_data 建键 + 分区子行（先用基础路径，右对齐补格后统一改名）
        for (_, info, disk_path) in &rows {
            self.node_data.insert(
                disk_path.clone(),
                NodeKind::Disk {
                    phys: info.physical_disk,
                },
            );
            for (part_key, part) in &info.partitions {
                let letter_display = if part.letter.is_empty() {
                    "-".to_string()
                } else {
                    part.letter.to_uppercase()
                };
                let part_text = format!(
                    "{part_key}    {}    {}    {letter_display}",
                    part.size, part.format
                );
                self.ui.tree.add(&format!("{disk_path}/{part_text}"));
            }
        }
        self.ui.tree.end();
        // 【陷阱】`Fl_Group::end()` 会把 current group 恢复为**树的父组**（左列
        // Flex）而不是清空。这个残留值会让下一次在回调里构造的顶层窗口
        // （one_click / restart 对话框）变成它的子窗口——表现为"点了没弹框"。
        // 事件循环期间 current 应为空，这里显式清掉。
        Group::set_current(None::<&Group>);
        // 【Bug 2 修复】彻底禁用收缩/展开：遍历整棵树把所有节点强制展开
        // （分区子行永远可见，配合 `set_show_collapse(false)` 无法再收起）。
        let mut cursor = self.ui.tree.first();
        while let Some(mut item) = cursor {
            if !item.is_open() {
                item.open();
            }
            cursor = self.ui.tree.next(&item);
        }
        self.apply_disk_preview();
        self.update_disk_combo();
        self.first_load = false;
        // 【Bug 1 修复】软件刚打开时树区域空白、需点击一下才显示：
        // 启动首次加载发生在窗口布局完成前后（后台线程回传 → 消息泵），
        // 此时仅 add() 内容不保证触发窗口重绘标记。这里显式给树和左列
        // 置 damage 标志，使首屏直接画出列表。
        self.ui.tree.redraw();
        self.ui.left_col.redraw();
    }

    /// 启用/禁用所有硬盘行内的"一键分区"按钮。
    fn set_disk_buttons_enabled(&mut self, enabled: bool) {
        for b in &mut self.ui.disk_buttons {
            if enabled {
                b.activate();
            } else {
                b.deactivate();
            }
        }
    }

    // ==================== 磁盘预览（§7.3）====================

    fn apply_disk_preview(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        for pp in self.pending.clone() {
            // 找到该物理磁盘的树路径
            let Some(disk_path) = self.disk_path_by_phys(pp.disk_physical) else {
                continue;
            };
            // 清除原有分区子节点（保留磁盘节点，仅删除其下）
            self.clear_children_visual(&disk_path);
            let disk_item = self.ui.tree.find_item(&disk_path);
            if let Some(item) = disk_item {
                let _ = self.ui.tree.select(&disk_path, false);
                let _ = item;
            }
            let mut add_child = |text: &str| {
                let p = format!("{disk_path}/{text}");
                self.ui.tree.add(&p);
                self.node_data.insert(p, NodeKind::Preview);
            };
            if pp.esp_enabled && pp.style == "gpt" {
                add_child(&format!(
                    "(预览) ESP    {}    FAT32    盘符自动",
                    fmt_size_mb(pp.esp_size_mb)
                ));
            }
            if pp.style == "gpt" {
                add_child(&format!("(预览) MSR    {}    -    -", fmt_size_mb(16)));
            }
            for (i, part) in pp.data_parts.iter().enumerate() {
                let size_text = match part.size_mb {
                    Some(v) => fmt_size_mb(v),
                    None => "剩余空间".to_string(),
                };
                add_child(&format!(
                    "(预览) 分区{}    {size_text}    {}    盘符自动",
                    i + 1,
                    part.fs
                ));
            }
        }
        // 预览集变化后同步 GHO「目标分区」候选（预览优先）
        self.refresh_gho_targets();
    }

    fn clear_children_visual(&mut self, disk_path: &str) {
        let prefix = format!("{disk_path}/");
        let to_remove: Vec<String> = self
            .node_data
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for k in &to_remove {
            self.node_data.remove(k);
        }
        // fltk 树：删除磁盘节点下所有子节点 —— 通过移除磁盘节点再重建。
        // 简化：删除磁盘节点下所有 item（Tree::clear_children）
        if let Some(item) = self.ui.tree.find_item(disk_path) {
            self.ui.tree.clear_children(&item);
        }
    }

    fn disk_path_by_phys(&self, phys: i64) -> Option<String> {
        for (path, kind) in &self.node_data {
            if let NodeKind::Disk { phys: p, .. } = kind {
                if *p as i64 == phys {
                    return Some(path.clone());
                }
            }
        }
        None
    }

    fn clear_disk_preview(&mut self) {
        self.pending.clear();
        // 预览清除后，GHO「目标分区」候选退回实际分区
        self.refresh_gho_targets();
    }

    // ==================== 硬盘下拉框 ====================

    fn update_disk_combo(&mut self) {
        let mut displays: Vec<String> = Vec::new();
        let mut index_map: Vec<u32> = Vec::new();
        for (disk_idx, info) in &self.disk_data {
            let mut display = format!("磁盘{disk_idx}: {} {}", info.model, info.size);
            if !info.style.is_empty() {
                display.push_str(&format!(" ({})", info.style));
            }
            displays.push(display);
            index_map.push(*disk_idx);
        }
        self.disk_combo_index = index_map.clone();
        // 注意：必须在 clear() 之前读取当前选中文本。
        // Fl_Choice 的 clear() 会把选中项重置为空（choice() == None），
        // 若在 clear 之后再读，「保留已选磁盘」分支就永远拿不到旧值、形同死代码。
        let cur_idx = extract_disk_idx(&self.ui.disk_choice.choice().unwrap_or_default());
        let cur_num = cur_idx.parse::<u32>().ok();
        self.ui.disk_choice.clear();
        if displays.is_empty() {
            // Fl_Choice 不能有空字符串条目（会被 FLTK 当作菜单终结符丢弃），
            // 无磁盘时直接留空菜单：choice() == None，与原来的 set_value("") 等价。
            self.ui.partition_preview.set_label("");
            return;
        }
        for d in &displays {
            self.ui.disk_choice.add_choice(d);
        }
        // 保留已选磁盘（按序号匹配），否则优先带 EFI 的磁盘
        if let Some(n) = cur_num {
            let pos = index_map.iter().position(|i| *i == n).unwrap_or(0);
            self.ui.disk_choice.set_value(pos as i32);
        } else {
            let efi = self.find_first_disk_with_efi()
                .parse::<u32>()
                .ok();
            let target = match efi {
                Some(n) => index_map.iter().position(|i| *i == n),
                None => Some(0),
            };
            if let Some(pos) = target {
                self.ui.disk_choice.set_value(pos as i32);
            }
        }
        self.derive_partition_keys();
    }

    fn find_first_disk_with_efi(&self) -> String {
        for (disk_idx, info) in &self.disk_data {
            for (_, part) in &info.partitions {
                if part.ptype == "ESP" {
                    return disk_idx.to_string();
                }
            }
        }
        String::new()
    }

    fn on_disk_select(&mut self) {
        self.invalidate_scheme();
        self.derive_partition_keys();
        self.update_install_button_state();
    }

    fn selected_disk_idx(&self) -> String {
        extract_disk_idx(&self.ui.disk_choice.choice().unwrap_or_default())
    }

    fn derive_partition_keys(&mut self) {
        let disk_idx = self.selected_disk_idx();
        if disk_idx.is_empty() {
            self.ui.partition_preview.set_label("");
            return;
        }
        // 分区样式：优先取预览方案
        let mut style: Option<String> = None;
        for pp in &self.pending {
            if pp.disk_idx.to_string() == disk_idx || pp.disk_physical.to_string() == disk_idx {
                style = Some(pp.style.clone());
                break;
            }
        }
        if style.is_none() {
            if let Ok(di) = disk_idx.parse::<u32>() {
                if let Some((_, info)) = self.disk_data.iter().find(|(i, _)| *i == di) {
                    style = Some(info.style.to_lowercase());
                }
            }
        }
        let style = style.unwrap_or_else(|| "gpt".to_string());
        let (boot_key, system_key) = if style == "mbr" {
            (format!("{disk_idx}:0"), format!("{disk_idx}:0"))
        } else {
            (format!("{disk_idx}:0"), format!("{disk_idx}:2"))
        };
        self.ui.partition_preview.set_label(&format!(
            "引导分区: {boot_key}    系统分区: {system_key}"
        ));
    }

    fn boot_windows_keys(&self) -> (String, String) {
        let disk_idx = self.selected_disk_idx();
        let mut style: Option<String> = None;
        for pp in &self.pending {
            if pp.disk_idx.to_string() == disk_idx {
                style = Some(pp.style.clone());
                break;
            }
        }
        if style.is_none() {
            if let Ok(di) = disk_idx.parse::<u32>() {
                if let Some((_, info)) = self.disk_data.iter().find(|(i, _)| *i == di) {
                    style = Some(info.style.to_lowercase());
                }
            }
        }
        let style = style.unwrap_or_else(|| "gpt".to_string());
        if style == "mbr" {
            (format!("{disk_idx}:0"), format!("{disk_idx}:0"))
        } else {
            (format!("{disk_idx}:0"), format!("{disk_idx}:2"))
        }
    }

    // ==================== 一键分区 ====================

    fn on_one_click(&mut self, disk_idx: u32, phys: u32) {
        log::info!("打开一键分区对话框: 磁盘{disk_idx} (物理磁盘 {phys})");
        let info = self
            .disk_data
            .iter()
            .find(|(i, _)| *i == disk_idx)
            .map(|(_, d)| d.clone());
        let Some(info) = info else {
            return;
        };
        let total_mb = info.size_bytes / (1024 * 1024);
        let dlg_data = one_click::DialogData {
            disk_idx,
            model: info.model.clone(),
            size_text: info.size.clone(),
            total_mb,
        };
        self.set_window_title(&format!("一键分区({}/{})", self.pending.len() + 1, self.pending.len() + 1));
        let result = one_click::show(&mut self.ui.win, dlg_data);
        self.set_status("就绪");
        let Some(result) = result else {
            log::info!("用户取消了一键分区");
            return;
        };
        log::info!(
            "已暂存分区方案（预览）：磁盘 {phys}, {}, ESP={}, 数据分区数={}，待开始安装时执行；当前共 {} 块磁盘待分区",
            result.style.to_uppercase(),
            if result.esp_enabled { "是" } else { "否" },
            result.data_parts.len(),
            self.pending.len() + 1
        );
        let new_entry = PendingPartition {
            disk_physical: phys as i64,
            disk_idx,
            style: result.style,
            esp_enabled: result.esp_enabled,
            esp_size_mb: result.esp_size_mb,
            data_parts: result.data_parts,
        };
        if let Some(slot) = self
            .pending
            .iter_mut()
            .find(|p| p.disk_physical == new_entry.disk_physical)
        {
            *slot = new_entry;
        } else {
            self.pending.push(new_entry);
        }
        self.invalidate_scheme();
        self.rebuild_tree();
        // 自动选中该硬盘
        self.select_disk_in_combo(disk_idx);
        self.update_install_button_state();
    }

    fn select_disk_in_combo(&mut self, disk_idx: u32) {
        if let Some(pos) = self.disk_combo_index.iter().position(|i| *i == disk_idx) {
            self.ui.disk_choice.set_value(pos as i32);
            self.derive_partition_keys();
        }
    }

    // ==================== 安装源加载 ====================

    fn on_browse_source(&mut self) {
        let mut dlg = dialog::FileDialog::new(dialog::FileDialogType::BrowseFile);
        dlg.set_title("选择安装源文件");
        // FLTK 用 '\n' 分隔多个过滤项，'\t' 分隔名称与通配符
        dlg.set_filter(
            "所有支持的镜像\t*.{iso,wim,esd,gho}\n\
             ISO 文件\t*.iso\n\
             WIM 文件\t*.wim\n\
             ESD 文件\t*.esd\n\
             GHO 文件\t*.gho",
        );
        dlg.show();
        let p = dlg.filename();
        if p.as_os_str().is_empty() {
            return;
        }
        let path = p.to_string_lossy().into_owned();
        log::info!("选择安装源: {path}");
        self.ui.source_path_in.set_value(&path);
        self.load_wim_info(&path);
    }

    fn load_wim_info(&mut self, file_path: &str) {
        let file_path = file_path.replace('/', "\\");
        log::info!("加载镜像信息: {file_path}");
        if file_path.to_lowercase().ends_with(".gho") {
            log::info!("检测到 GHO 文件，切换到 GHO 还原模式");
            self.set_gho_mode(true);
            self.current_wim_path = Some(file_path.clone());
            self.set_status(&format!(
                "GHO 镜像已加载: {}",
                Path::new(&file_path)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            self.update_install_button_state();
            return;
        }
        self.set_gho_mode(false);

        let lower = file_path.to_lowercase();
        let mut wim_path = file_path.clone();
        let mut mount_path: Option<String> = None;

        if lower.ends_with(".iso") {
            log::info!("正在挂载 ISO...");
            self.set_status("正在挂载 ISO...");
            let (ok, mp, error) = iso::mount_iso(&file_path);
            if !ok {
                log::error!("ISO 挂载失败: {error}");
                dialog::alert_default(&format!("ISO 挂载失败:\n{error}"));
                self.set_status("ISO 挂载失败");
                return;
            }
            let wim_files = iso::find_wim_in_iso(&mp);
            if wim_files.is_empty() {
                log::error!("ISO 中未找到 install.wim/install.esd: {mp}");
                dialog::alert_default("在 ISO 中未找到 install.wim 或 install.esd 文件");
                iso::unmount_iso(&mp);
                self.set_status("未找到安装镜像");
                return;
            }
            wim_path = wim_files[0].clone();
            mount_path = Some(mp);
            log::info!("找到镜像文件: {wim_path}");
        } else if lower.ends_with(".wim") || lower.ends_with(".esd") {
            log::info!("WIM/ESD 文件直接使用");
        } else {
            log::warn!("不支持的文件类型: {file_path}");
            dialog::alert_default("不支持的文件类型，请选择 ISO、WIM、ESD 或 GHO 文件");
            self.set_status("不支持的文件类型");
            return;
        }

        self.set_status("正在读取镜像信息...");
        let images = crate::app::install::dism::get_wim_info(&wim_path);
        if images.is_empty() {
            log::error!("读取镜像信息失败: {wim_path}");
            dialog::alert_default("无法读取镜像信息，请确认文件完整性");
            if let Some(mp) = mount_path {
                iso::unmount_iso(&mp);
            }
            self.set_status("读取镜像信息失败");
            return;
        }
        self.current_mount_path = mount_path;
        self.ui.image_choice.clear();
        let mut names: Vec<String> = Vec::new();
        for img in &images {
            let label = if !img.name.is_empty() {
                img.name.clone()
            } else if !img.description.is_empty() {
                img.description.clone()
            } else {
                format!("镜像 {}", img.index)
            };
            names.push(format!("{}: {label}", img.index));
        }
        for n in &names {
            self.ui.image_choice.add_choice(n);
        }
        if !names.is_empty() {
            self.ui.image_choice.set_value(0);
        }
        self.current_wim_path = Some(wim_path);
        self.set_status(&format!("镜像加载完成: {} 个镜像", images.len()));

        let lower_name = Path::new(&file_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ext = lower_name.rsplit('.').next().unwrap_or("");
        let is_install_image =
            ext == "iso" || lower_name == "install.wim" || lower_name == "install.esd";
        self.update_image_dependent_options(is_install_image);
        self.update_install_button_state();
        log::info!("加载完成，共 {} 个镜像", images.len());
    }

    fn update_image_dependent_options(&mut self, is_install_image: bool) {
        if is_install_image {
            self.ui.copy_tools_cb.activate();
            self.ui.deploy_cb.activate();
            self.ui.defender_cb.activate();
            self.ui.firewall_cb.activate();
            self.ui.copy_tools_cb.set_checked(true);
            self.ui.deploy_cb.set_checked(true);
        } else {
            self.ui.copy_tools_cb.deactivate();
            self.ui.deploy_cb.deactivate();
            self.ui.defender_cb.deactivate();
            self.ui.firewall_cb.deactivate();
            self.ui.copy_tools_cb.set_checked(false);
            self.ui.deploy_cb.set_checked(false);
            self.ui.defender_cb.set_checked(false);
            self.ui.firewall_cb.set_checked(false);
        }
    }

    // ==================== 安装按钮状态 ====================

    fn update_install_button_state(&mut self) {
        let can_install = if self.is_gho_mode {
            let has_gho = self.current_wim_path.is_some();
            if self.ui.gho_mode_disk_rb.value() {
                has_gho && !self.ui.gho_disk_choice.choice().unwrap_or_default().is_empty()
            } else {
                has_gho && !self.ui.gho_part_choice.choice().unwrap_or_default().is_empty()
            }
        } else {
            let has_image = self.current_wim_path.is_some()
                && !self.ui.image_choice.choice().unwrap_or_default().is_empty();
            if !self.pending.is_empty() {
                has_image
            } else {
                has_image
                    && !self.selected_disk_idx().is_empty()
                    && !self.ui.partition_preview.label().is_empty()
            }
        };
        if can_install {
            self.ui.install_btn.activate();
        } else {
            self.ui.install_btn.deactivate();
        }
    }

    // ==================== GHO 模式 ====================

    fn set_gho_mode(&mut self, is_gho: bool) {
        log::info!("切换到 {} 模式", if is_gho { "GHO" } else { "WIM" });
        self.is_gho_mode = is_gho;
        // 分组高度（与构造时 fixed 值一致），0 表示折叠隐藏。
        // 数值与 `new()` 中的卡片高度保持一致，否则切换模式后卡片会被压扁。
        const TARGET_H: i32 = 98;
        const GHO_H: i32 = 132;
        const WIM_OPT_H: i32 = 119;
        const GHO_OPT_H: i32 = 26;
        // 「选项」外框：WIM = 内边距20 + 标题18 + 间距5 + 119 = 162；
        // GHO 只剩一行选项：20 + 18 + 5 + 26 = 69，留 3px 余量取 72。
        const OPTIONS_H: i32 = 162;
        const GHO_OPTIONS_H: i32 = 72;
        if is_gho {
            self.ui.image_choice.hide();
            self.ui.target_group.hide();
            self.ui.wim_options_group.hide();
            self.ui.gho_group.show();
            self.ui.gho_options_group.show();
            self.ui.right_col.fixed(&self.ui.target_group, 0);
            self.ui.right_col.fixed(&self.ui.gho_group, GHO_H);
            self.ui.right_col.fixed(&self.ui.options_group, GHO_OPTIONS_H);
            self.ui.options_group.fixed(&self.ui.wim_options_group, 0);
            self.ui.options_group.fixed(&self.ui.gho_options_group, GHO_OPT_H);
            self.refresh_gho_targets();
        } else {
            self.ui.gho_group.hide();
            self.ui.gho_options_group.hide();
            self.ui.target_group.show();
            self.ui.image_choice.show();
            self.ui.wim_options_group.show();
            self.ui.right_col.fixed(&self.ui.target_group, TARGET_H);
            self.ui.right_col.fixed(&self.ui.gho_group, 0);
            self.ui.right_col.fixed(&self.ui.options_group, OPTIONS_H);
            self.ui.options_group.fixed(&self.ui.wim_options_group, WIM_OPT_H);
            self.ui.options_group.fixed(&self.ui.gho_options_group, 0);
        }
        self.ui.root.layout();
        self.update_install_button_state();
    }

    fn on_gho_mode_changed(&mut self) {
        if self.ui.gho_mode_disk_rb.value() {
            self.ui.gho_part_group.hide();
            self.ui.gho_disk_group.show();
        } else {
            self.ui.gho_disk_group.hide();
            self.ui.gho_part_group.show();
        }
        // 【陷阱】内层组 show/hide 后 FLTK Flex 不会自动重排（对比
        // set_gho_mode 末尾显式 root.layout()），不补一次目标行会空白。
        self.ui.gho_group.layout();
        self.refresh_gho_targets();
    }

    fn on_gho_target_selected(&mut self) {
        self.scheme_gho_disk = None;
        self.scheme_gho_partition = None;
        self.invalidate_scheme();
        self.update_install_button_state();
    }

    fn refresh_gho_targets(&mut self) {
        if self.ui.gho_mode_disk_rb.value() {
            let mut disks: Vec<String> = Vec::new();
            for (disk_idx, info) in &self.disk_data {
                disks.push(format!("磁盘{disk_idx}: {} {}", info.model, info.size));
            }
            // 必须在 clear() 之前读取当前选中文本：clear() 会把选中项清空。
            let cur = self.ui.gho_disk_choice.choice().unwrap_or_default();
            self.ui.gho_disk_choice.clear();
            if disks.is_empty() {
                // Fl_Choice 不能有空字符串条目；无磁盘时留空菜单（choice() == None）。
            } else {
                for d in &disks {
                    self.ui.gho_disk_choice.add_choice(d);
                }
                let mut restored = false;
                if let Some(ref target) = self.scheme_gho_disk {
                    for (i, d) in disks.iter().enumerate() {
                        let logic = extract_disk_idx(d);
                        let phys = self
                            .disk_data
                            .iter()
                            .find(|(idx, _)| idx.to_string() == logic)
                            .map(|(_, info)| info.physical_disk)
                            .unwrap_or(0);
                        if phys.to_string() == *target {
                            self.ui.gho_disk_choice.set_value(i as i32);
                            restored = true;
                            break;
                        }
                    }
                }
                if !restored {
                    if !cur.is_empty() {
                        // 【失效条目】仅选中已存在的条目，绝不新增：旧选中项指向的
                        // 分区可能已被清理/重建，若按 set_choice_by_text 的兜底把它
                        // add 回来，选中后解析出失效目标（分区号 0）会直接传给 Ghost。
                        select_choice_exact(&mut self.ui.gho_disk_choice, &cur);
                    } else {
                        self.ui.gho_disk_choice.set_value(0);
                    }
                }
            }
        } else {
            // 【预览优先】有一键分区暂存（预览）的盘列出预览的数据分区，前缀
            // "盘号:计划分区号"（GPT: ESP=1/MSR=2 占位，数据分区从 3 起、未建
            // ESP 时从 2 起；MBR 从 1 起——与 diskpart 创建顺序一致）。没有
            // 预览的盘仍列实际分区。
            let mut parts: Vec<String> = Vec::new();
            for (disk_idx, info) in &self.disk_data {
                let preview = self
                    .pending
                    .iter()
                    .find(|p| p.disk_physical == info.physical_disk as i64)
                    .map(|pp| gho_preview_entries(*disk_idx, pp))
                    .unwrap_or_default();
                if !preview.is_empty() {
                    parts.extend(preview);
                    continue;
                }
                for (part_key, part) in &info.partitions {
                    let letter_display = if part.letter.is_empty() {
                        "-".to_string()
                    } else {
                        part.letter.to_uppercase()
                    };
                    parts.push(format!(
                        "{part_key} [{letter_display}] {} {}",
                        part.size, part.format
                    ));
                }
            }
            let cur = self.ui.gho_part_choice.choice().unwrap_or_default();
            self.ui.gho_part_choice.clear();
            if parts.is_empty() {
                // 同上：无分区时留空菜单。
            } else {
                for p in &parts {
                    self.ui.gho_part_choice.add_choice(p);
                }
                // 方案回载：gho_partition_index 存的就是前缀里的分区号
                let mut restored = false;
                if let Some(ref target) = self.scheme_gho_partition {
                    for (i, p) in parts.iter().enumerate() {
                        if let Some((_, num)) = parse_leading_disk_part(p) {
                            if num.to_string() == *target {
                                self.ui.gho_part_choice.set_value(i as i32);
                                restored = true;
                                break;
                            }
                        }
                    }
                }
                if !restored {
                    if !cur.is_empty() {
                        // 【失效条目】仅选中已存在的条目，绝不新增：旧选中项指向的
                        // 分区可能已被清理/重建，若按 set_choice_by_text 的兜底把它
                        // add 回来，选中后解析出失效目标（分区号 0）会直接传给 Ghost。
                        select_choice_exact(&mut self.ui.gho_part_choice, &cur);
                    } else {
                        self.ui.gho_part_choice.set_value(0);
                    }
                }
            }
        }
        self.update_install_button_state();
    }

    // ==================== 日志面板 ====================

    /// 日志面板展开 / 收起。
    ///
    /// 低分辨率适配：
    /// - 展开时优先占用窗口下方的剩余屏幕空间（最多 140px）；
    /// - 下方空间不够时，再从主体（磁盘树）的富余高度里借用；
    /// - 两者都不够时才临时压缩主体，保证窗口本身永远不超出屏幕。
    fn on_toggle_log_visible(&mut self) {
        const LOG_H: i32 = 140;
        const HEADER_H: i32 = 74;
        const FOOTER_H: i32 = 62;
        // 右侧卡片列的最小高度（含主体上下 margin 14×2）：
        // WIM = 100+98+162+间距20+margin28，GHO = 100+132+72+间距20+margin28。
        let min_main = if self.is_gho_mode { 352 } else { 408 };

        if self.ui.log_panel.is_visible() {
            self.ui.log_group.hide();
            self.ui.log_panel.set_visible(false);
            self.ui.log_toggle_btn.set_label("显示日志");
            self.ui.root.fixed(&self.ui.log_group, 0);
            // 只缩回展开时增高的部分；其余高度本来就从主体富余里借用，保持不动。
            if self.log_grown > 0 {
                let h = (self.ui.win.h() - self.log_grown).max(200);
                let (x, y, w) = (self.ui.win.x(), self.ui.win.y(), self.ui.win.w());
                self.ui.win.resize(x, y, w, h);
                self.log_grown = 0;
            }
        } else {
            let (_, sh) = app::screen_size();
            let room =
                (sh as i32 - self.ui.win.y() - self.ui.win.h() - 24).clamp(0, LOG_H);
            // 此刻日志是隐藏状态（哪怕控件高度还停留在上次展开值），
            // 主体实际占用整个窗口剩余高度。
            let current_main = self.ui.win.h() - HEADER_H - FOOTER_H;
            let slack = (current_main - min_main).max(0);
            let need = (LOG_H - slack).max(0);
            let grow = need.min(room);
            // 最低 80px：日志标题 + 内边距 + 约 2 行文本。
            let log_h = (LOG_H - (need - grow)).max(80).min(LOG_H);

            self.ui.log_group.show();
            self.ui.log_panel.set_visible(true);
            self.ui.log_toggle_btn.set_label("隐藏日志");
            self.ui.root.fixed(&self.ui.log_group, log_h);
            if grow > 0 {
                let h = self.ui.win.h() + grow;
                let (x, y, w) = (self.ui.win.x(), self.ui.win.y(), self.ui.win.w());
                self.ui.win.resize(x, y, w, h);
                self.log_grown = grow;
            }
            self.ui.log_panel.scroll_to_bottom();
        }
        self.ui.root.layout();
    }

    // ==================== 选项回调 ====================

    fn on_toggle_restore(&mut self) {
        let enabled = self.ui.restore_cb.is_checked();
        self.cfg.restore_enabled = enabled;
        self.cfg.save(None);
        if enabled {
            let image_path = self.ui.source_path_in.value();
            if image_path.is_empty() {
                log::warn!("开启自动还原失败: 未选择镜像文件");
                dialog::alert_default("请先选择镜像文件再开启自动还原");
                self.ui.restore_cb.set_checked(false);
                self.cfg.restore_enabled = false;
                self.cfg.save(None);
                self.cancel_countdown();
                self.set_restore_state(false);
                return;
            }
            log::info!("自动还原已开启, 倒计时 {}秒", self.cfg.countdown_sec);
            self.start_countdown(self.cfg.countdown_sec);
            self.set_status("自动还原服务已开启");
            self.set_restore_state(true);
        } else {
            log::info!("自动还原已关闭");
            self.cancel_countdown();
            self.set_status("自动还原服务已关闭");
            self.set_restore_state(false);
        }
    }

    fn on_toggle_auto_restart(&mut self) {
        let enabled = self.ui.restart_cb.is_checked() || self.ui.gho_restart_cb.is_checked();
        log::info!("安装完成后重启: {}", if enabled { "开启" } else { "关闭" });
        self.cfg.auto_restart = enabled;
        self.cfg.save(None);
    }

    fn on_toggle_copy_tools(&mut self) {
        let enabled = self.ui.copy_tools_cb.is_checked();
        log::info!("复制调试工具: {}", if enabled { "开启" } else { "关闭" });
        self.cfg.copy_tools = enabled;
        self.cfg.save(None);
    }

    fn on_toggle_deploy_unattend(&mut self) {
        let enabled = self.ui.deploy_cb.is_checked();
        log::info!("自动部署与系统优化: {}", if enabled { "开启" } else { "关闭" });
        self.cfg.deploy_unattend = enabled;
        self.cfg.save(None);
    }

    fn on_toggle_disable_defender(&mut self) {
        let enabled = self.ui.defender_cb.is_checked();
        log::info!("禁用Windows Defender: {}", if enabled { "开启" } else { "关闭" });
        self.cfg.disable_defender = enabled;
        self.cfg.save(None);
    }

    fn on_toggle_disable_firewall(&mut self) {
        let enabled = self.ui.firewall_cb.is_checked();
        log::info!("关闭Windows防火墙: {}", if enabled { "开启" } else { "关闭" });
        self.cfg.disable_firewall = enabled;
        self.cfg.save(None);
    }

    fn on_browse_tools_source(&mut self) {
        let mut dlg = dialog::FileDialog::new(dialog::FileDialogType::BrowseDir);
        dlg.set_title("选择调试工具源目录");
        dlg.show();
        let p = dlg.filename();
        if p.as_os_str().is_empty() {
            return;
        }
        let path = p.to_string_lossy().into_owned();
        log::info!("选择调试工具源目录: {path}");
        self.ui.tools_path_in.set_value(&path);
        self.cfg.tools_source_path = path;
        self.cfg.save(None);
    }

    fn on_tools_source_path_changed(&mut self, path: &str) {
        self.cfg.tools_source_path = path.trim().to_string();
        self.cfg.save(None);
    }

    // ==================== 安装主流程（§8.3）====================

    fn on_install_system(&mut self, skip_confirm: bool) {
        if self.is_installing {
            self.on_stop_install();
            return;
        }
        if self.current_wim_path.is_none() {
            log::warn!("安装中止: 未选择安装源");
            dialog::alert_default("请选择安装源");
            return;
        }
        if self.is_gho_mode {
            // 自动还原链路要把 skip_confirm 传下去（GHO 确认同样跳过）
            self.on_gho_install(skip_confirm);
            return;
        }
        if self.ui.image_choice.choice().unwrap_or_default().is_empty() {
            log::warn!("安装中止: 未选择镜像索引");
            dialog::alert_default("请选择要安装的镜像");
            return;
        }
        let has_pending = !self.pending.is_empty();
        let mut boot_letter = String::new();
        let mut windows_letter = String::new();
        if !has_pending {
            if self.selected_disk_idx().is_empty() {
                log::warn!("安装中止: 未选择安装硬盘");
                dialog::alert_default("请选择安装硬盘");
                return;
            }
            let (boot_key, win_key) = self.boot_windows_keys();
            boot_letter = self.get_letter_by_part_key(&boot_key);
            windows_letter = self.get_letter_by_part_key(&win_key);
            if windows_letter.is_empty() {
                log::warn!("安装中止: 系统分区 {win_key} 无盘符");
                dialog::alert_default(&format!("系统分区 {win_key} 没有盘符，无法安装系统"));
                return;
            }
            if boot_letter.is_empty() {
                log::info!("引导分区 {boot_key} 无盘符，尝试自动分配...");
                boot_letter = self.try_assign_esp_drive_letter(&boot_key);
                if boot_letter.is_empty() {
                    log::warn!("安装中止: 引导分区 {boot_key} 无盘符且自动分配失败");
                    dialog::alert_default(&format!(
                        "引导分区 {boot_key} 没有盘符，且无法自动分配，无法写入引导"
                    ));
                    return;
                }
            }
        }
        let image_text = self.ui.image_choice.choice().unwrap_or_default();
        let image_index = image_text
            .split(':')
            .next()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(1);
        self.last_install_admin_username = {
            let v = self.ui.admin_user_in.value().trim().to_string();
            if v.is_empty() {
                "admin".to_string()
            } else {
                v
            }
        };
        self.last_install_image_index = image_index;
        let format_windows = self.ui.format_cb.is_checked();
        let auto_repair_boot = self.ui.repair_boot_cb.is_checked();
        let disable_defender = self.ui.defender_cb.is_checked();

        // 确认（简化：保留关键文案）。自动还原链路（run_auto_restore 传
        // skip_confirm=true）不弹确认框；手动点「开始安装」仍弹出。
        if skip_confirm {
            log::info!("自动还原: 跳过安装确认对话框");
        } else {
            let confirm_msg = format!(
                "镜像: {}\n镜像索引: {image_index}\n格式化目标分区: {}\n自动修复引导: {}\n关闭Windows防火墙: {}\n\n警告：此操作将覆盖目标分区的所有数据！\n确定要继续吗？",
                self.current_wim_path.clone().unwrap_or_default(),
                if format_windows { "是" } else { "否" },
                if auto_repair_boot { "是" } else { "否" },
                if self.ui.firewall_cb.is_checked() { "是" } else { "否" },
            );
            let choice = dialog::choice2_default(&confirm_msg, "确定", "取消", "");
            if choice != Some(0) {
                log::info!("用户取消安装确认");
                return;
            }
        }

        log::info!(
            "开始安装: 镜像={}, 索引={image_index}, 含一键分区={}, 待分区硬盘数={}",
            self.current_wim_path.clone().unwrap_or_default(),
            if has_pending { "是" } else { "否" },
            self.pending.len()
        );
        self.is_installing = true;
        self.install_cancelled.store(false, Ordering::SeqCst);
        self.set_install_panel_enabled(false);
        self.set_disk_buttons_enabled(false);
        self.ui.install_btn.set_label("停止安装");
        self.ui.install_btn.activate();
        self.set_status(if has_pending {
            "正在执行分区..."
        } else {
            "正在安装系统..."
        });

        let pending_list = self.pending.clone();
        let pending_count = self.pending.len();
        let current_phys_set: std::collections::HashSet<u32> =
            self.disk_data.iter().map(|(_, info)| info.physical_disk).collect();
        let wim_path = self.current_wim_path.clone().unwrap_or_default();
        let cancel = self.install_cancelled.clone();
        let tx = super::tx();
        std::thread::spawn(move || {
            // 【过滤】pending_list 是线程启动前的快照，物理盘可能在快照之后被
            // 拔出/重新枚举，导致后续 one_click_partition 调 select disk N 失败。
            // 这里按当前 disk_data 过滤，跳过分不到的盘并记 warning。
            let filtered_pending: Vec<PendingPartition> = pending_list
                .into_iter()
                .filter(|p| current_phys_set.contains(&(p.disk_physical as u32)))
                .collect();
            if filtered_pending.len() < pending_count {
                log::warn!(
                    "一键分区跳过不存在（已拔出/未枚举）的盘：\
                     方案计划{}块，当前枚举{}块，跳过{}",
                    pending_count,
                    current_phys_set.len(),
                    pending_count - filtered_pending.len()
                );
            }
            let mut boot_letter = boot_letter;
            let mut windows_letter = windows_letter;
            if !filtered_pending.is_empty() {
                let total = filtered_pending.len();
                for (idx, pending) in filtered_pending.iter().enumerate() {
                    let n = idx + 1;
                    log::info!("阶段1.{n}/{total}: 执行一键分区, 磁盘{}", pending.disk_physical);
                    tx.send(Message::Progress(
                        format!("正在执行一键分区({n}/{total})..."),
                        0,
                    ));
                    app::awake();
                    let is_first = idx == 0;
                    let specs: Vec<DataPartSpec> = pending
                        .data_parts
                        .iter()
                        .map(|d| DataPartSpec::new(d.size_mb, &d.fs))
                        .collect();
                    let mut assigned = diskpart::AssignedLetters::default();
                    let (ok, output) = diskpart::one_click_partition(
                        pending.disk_physical as u32,
                        &pending.style,
                        pending.esp_enabled,
                        pending.esp_size_mb,
                        Some(&specs),
                        is_first,
                        &mut assigned,
                    );
                    if !ok {
                        let err = format!("磁盘{} 分区失败:\n{output}", pending.disk_physical);
                        log::error!("{err}");
                        tx.send(Message::InstallComplete(false, err, None));
                        app::awake();
                        return;
                    }
                    if is_first {
                        if let Some(c) = assigned.esp {
                            boot_letter = format!("{c}:");
                        }
                        if let Some(c) = assigned.data.first() {
                            windows_letter = format!("{c}:");
                            if assigned.esp.is_none() {
                                boot_letter = windows_letter.clone();
                            }
                        }
                    }
                }
                tx.send(Message::Progress("正在读取分区信息...".to_string(), 5));
                app::awake();
                let new_info = crate::app::disk_info::enumerate();
                if boot_letter.is_empty() || windows_letter.is_empty() {
                    let phys = filtered_pending[0].disk_physical;
                    let (b, w) = find_letters_for_disk(&new_info, phys);
                    if boot_letter.is_empty() {
                        boot_letter = b;
                    }
                    if windows_letter.is_empty() {
                        windows_letter = w;
                    }
                }
                if boot_letter.is_empty() || windows_letter.is_empty() {
                    tx.send(Message::InstallComplete(
                        false,
                        "分区后未找到有效的引导/系统分区，请手动选择后重试".to_string(),
                        None,
                    ));
                    app::awake();
                    return;
                }
                tx.send(Message::PostPartitionRefresh(new_info));
                app::awake();
            }

            tx.send(Message::Progress("正在安装系统...".to_string(), 10));
            app::awake();
            let progress = move |status: &str, p: i32| {
                tx.send(Message::Progress(status.to_string(), p));
                app::awake();
            };
            let cancel_cb = || cancel.load(Ordering::SeqCst);
            let (ok, output) = crate::app::install::install_system(
                &wim_path,
                image_index,
                &boot_letter,
                &windows_letter,
                auto_repair_boot,
                format_windows,
                disable_defender,
                Some(&progress),
                Some(&cancel_cb),
            );
            if ok {
                log::info!("系统安装成功");
            } else {
                log::warn!("系统安装返回失败: {output}");
            }
            tx.send(Message::InstallComplete(
                ok,
                output,
                // 记录系统分区盘符（← `_last_install_windows_partition`，
                // gui.py 2085/2087：在执行 install_system 时记录，无论成败）
                Some(windows_letter),
            ));
            app::awake();
        });
    }

    fn on_stop_install(&mut self) {
        let choice = dialog::choice2_default(
            "确定要停止安装吗？\n停止后已安装的内容需要手动清理。",
            "确定",
            "取消",
            "",
        );
        if choice != Some(0) {
            return;
        }
        log::warn!("用户请求停止安装");
        self.install_cancelled.store(true, Ordering::SeqCst);
        self.ui.install_btn.deactivate();
        self.ui.install_btn.set_label("正在停止...");
        self.set_status("正在停止安装...");
    }

    // ==================== GHO 安装（§8.4）====================

    fn on_gho_install(&mut self, skip_confirm: bool) {
        let gho_path = self.current_wim_path.clone().unwrap_or_default();
        let ghost_exe = self.ui.ghost_path_in.value();
        let restore_mode = if self.ui.gho_mode_part_rb.value() {
            "partition"
        } else {
            "disk"
        };
        // 【预览联动】分区还原目标为预览条目时，记录匹配的分区计划：
        // 目标分区尚不存在，安装前需先执行一键分区（同 WIM 流程）。
        let mut preview_pending: Option<PendingPartition> = None;
        let (logical_disk, ghost_disk_index, ghost_partition_index) = if restore_mode == "disk" {
            let target_text = self.ui.gho_disk_choice.choice().unwrap_or_default();
            if target_text.is_empty() {
                dialog::alert_default("请选择目标硬盘");
                return;
            }
            let Some(m) = parse_leading_disk(&target_text) else {
                dialog::alert_default("无法解析目标硬盘");
                return;
            };
            let phys = self.resolve_ghost_disk(m);
            (m, phys, 0)
        } else {
            let target_text = self.ui.gho_part_choice.choice().unwrap_or_default();
            if target_text.is_empty() {
                dialog::alert_default("请选择目标分区");
                return;
            }
            if target_text.contains("(预览)") {
                // 一键分区预览条目：盘尚未实际分区，前缀"盘号:计划分区号"里的
                // 计划号直接作为 Ghost 目标分区号，不能按实际分区表换算。
                let Some((ld, planned)) = parse_leading_disk_part(&target_text) else {
                    dialog::alert_default("无法解析目标分区");
                    return;
                };
                let phys = self.resolve_ghost_disk(ld);
                // 找到该盘的分区计划，安装线程里先执行一键分区
                preview_pending = self
                    .pending
                    .iter()
                    .find(|p| p.disk_physical as u32 == phys)
                    .cloned();
                (ld, phys, planned)
            } else {
                let Some((ld, lp)) = parse_leading_disk_part(&target_text) else {
                    dialog::alert_default("无法解析目标分区");
                    return;
                };
                match self.resolve_ghost_part(ld, lp) {
                    Some((phys, part_num)) => (ld, phys, part_num),
                    None => {
                        // 目标分区已失效（被清理/重建）：Ghost 的分区号从 1 起，
                        // 退回 0 会变成 dst=N:0 → ABORT 10099。提示用户重选。
                        dialog::alert_default(
                            "目标分区已变更，请在下拉框中重新选择后重试",
                        );
                        return;
                    }
                }
            }
        };
        let _ = logical_disk;
        // 【重启统一】不再给 Ghost 命令追加 -rb：还原完成后与 WIM 走同一套
        // show_install_success 重启控制（勾选"还原后自动重启"→30 秒倒计时
        // 对话框，可取消；未勾选→询问是否立即重启）。
        let auto_restart = self.ui.gho_restart_cb.is_checked();
        let mode_text = if restore_mode == "disk" {
            "硬盘还原"
        } else {
            "分区还原"
        };
        let confirm_msg = format!(
            "GHO 镜像还原\n\n镜像: {gho_path}\n模式: {mode_text}\nGhost: {ghost_exe}\n还原后自动重启: {}\n\n警告：此操作将覆盖目标分区的所有数据！\n确定要继续吗？",
            if auto_restart { "是" } else { "否" }
        );
        // 自动还原链路（on_install_system(skip_confirm=true) 分发过来）不弹确认框
        if skip_confirm {
            log::info!("自动还原: 跳过 GHO 还原确认对话框");
        } else {
            let choice = dialog::choice2_default(&confirm_msg, "确定", "取消", "");
            if choice != Some(0) {
                log::info!("用户取消 GHO 还原确认");
                return;
            }
        }
        log::info!(
            "开始 GHO 还原: 镜像={gho_path}, 模式={restore_mode}, ghost_disk_index={ghost_disk_index}"
        );
        self.is_installing = true;
        self.install_cancelled.store(false, Ordering::SeqCst);
        self.set_install_panel_enabled(false);
        self.set_disk_buttons_enabled(false);
        self.ui.install_btn.set_label("停止安装");
        self.ui.install_btn.activate();
        self.set_status("正在执行 GHO 还原...");

        let tx = super::tx();
        let cancel = self.install_cancelled.clone();
        // 复选框状态先取出来：闭包要跨线程，不能搬走整个 App（含 FLTK 的
        // `*mut ()` 控件指针，非 Send）。
        let gho_repair_boot = self.ui.gho_repair_boot_cb.is_checked();
        std::thread::spawn(move || {
            let resolved_ghost = expand_vars(&ghost_exe).trim_matches('"').to_string();
            // 【预览联动】目标是预览条目：先执行一键分区（同 WIM 阶段1），
            // diskpart 创建顺序与预览计划号一致，Ghost 目标号即实际分区号。
            if let Some(pending) = &preview_pending {
                log::info!(
                    "目标为预览分区: 先执行一键分区, 磁盘{}",
                    pending.disk_physical
                );
                tx.send(Message::Progress("正在执行一键分区...".to_string(), 0));
                app::awake();
                let specs: Vec<DataPartSpec> = pending
                    .data_parts
                    .iter()
                    .map(|d| DataPartSpec::new(d.size_mb, &d.fs))
                    .collect();
                let mut assigned = diskpart::AssignedLetters::default();
                let (ok, out) = diskpart::one_click_partition(
                    pending.disk_physical as u32,
                    &pending.style,
                    pending.esp_enabled,
                    pending.esp_size_mb,
                    Some(&specs),
                    true,
                    &mut assigned,
                );
                if !ok {
                    let err = format!("磁盘{} 分区失败:\n{out}", pending.disk_physical);
                    log::error!("{err}");
                    tx.send(Message::InstallComplete(false, err, None));
                    app::awake();
                    return;
                }
                tx.send(Message::Progress("正在读取分区信息...".to_string(), 5));
                app::awake();
                let new_info = crate::app::disk_info::enumerate();
                tx.send(Message::PostPartitionRefresh(new_info));
                app::awake();
            }
            if cancel.load(Ordering::SeqCst) {
                tx.send(Message::InstallComplete(
                    false,
                    "安装已被取消".to_string(),
                    None,
                ));
                app::awake();
                return;
            }
            let src = relative_gho_src(&gho_path);
            let ghost_dst = ghost_disk_index + 1;
            let clone = if restore_mode == "disk" {
                format!(
                    "-clone,mode=load,src=\"{}\",dst={ghost_dst}",
                    src.src_name
                )
            } else {
                format!(
                    "-clone,mode=pload,src=\"{}:1\",dst={ghost_dst}:{ghost_partition_index}",
                    src.src_name
                )
            };
            // 【重启统一】命令只有 -sure -fx，重启交给完成逻辑控制
            let cmd = format!("\"{resolved_ghost}\" {clone} -sure -fx");
            log::info!("执行 Ghost 命令: {cmd} (工作目录: {})", src.workdir);
            tx.send(Message::Progress("正在执行 GHO 还原...".to_string(), 10));
            app::awake();
            let result = run_ghost(&cmd, &src.workdir);
            if let Some(ref cleanup) = src.cleanup {
                super::ghost::safe_try_remove(&cleanup.to_string_lossy());
            }
            match result {
                Ok((0, output)) => {
                    // 成功时 Ghost 的控制台输出如有内容仅记调试日志
                    if !output.trim().is_empty() {
                        log::debug!("Ghost 输出: {}", output.trim_end());
                    }
                    log::info!("GHO 还原成功");
                    // 【修复引导】分区还原后重建 BCD（← gui.py:2347-2390）。
                    // GHO 本身保留原 MBR/引导扇区，只需 bcdboot。
                    // 由「分区还原后修复引导」复选框控制（默认不勾选）。
                    if restore_mode == "partition" && gho_repair_boot {
                        tx.send(Message::Progress("正在修复系统引导...".to_string(), 50));
                        app::awake();
                        let new_info = crate::app::disk_info::enumerate();
                        let mut esp_letter = String::new();
                        let mut windows_letter = String::new();
                        for (_idx, info) in &new_info {
                            if info.physical_disk != ghost_disk_index {
                                continue;
                            }
                            for (_pk, part) in &info.partitions {
                                let ptype = part.ptype.to_uppercase();
                                let fmt = part.format.to_uppercase();
                                if esp_letter.is_empty() && ptype == "ESP" && !part.letter.is_empty() {
                                    esp_letter = part.letter.to_uppercase();
                                }
                                if windows_letter.is_empty()
                                    && fmt == "NTFS"
                                    && !part.letter.is_empty()
                                    && ptype != "ESP"
                                {
                                    windows_letter = part.letter.to_uppercase();
                                }
                            }
                            break;
                        }
                        let (boot_ok, boot_out) =
                            boot::repair_boot_from_letters(&esp_letter, &windows_letter);
                        if !boot_ok {
                            log::warn!("GHO 引导修复失败: {boot_out}");
                        }
                    }
                    tx.send(Message::InstallComplete(true, "GHO 还原成功".to_string(), None));
                    app::awake();
                }
                Ok((code, output)) => {
                    let mut err = format!("GHO 还原失败 (返回码 {code})");
                    let shown: String = output.trim().chars().take(500).collect();
                    if !shown.is_empty() {
                        err.push_str(&format!("\n\n--- Ghost 输出 ---\n{shown}"));
                    }
                    if let Some(tail) = read_ghost_debug_tail(&src.workdir, &resolved_ghost) {
                        err.push_str(&format!("\n\n--- Ghost debug.log (末尾) ---\n{tail}"));
                    }
                    log::error!("{err}");
                    tx.send(Message::InstallComplete(false, err, None));
                    app::awake();
                }
                Err(e) => {
                    log::error!("GHO 还原异常: {e}");
                    tx.send(Message::InstallComplete(false, e, None));
                    app::awake();
                }
            }
        });
    }

    fn resolve_ghost_disk(&self, logical: u32) -> u32 {
        self.disk_data
            .iter()
            .find(|(idx, _)| *idx == logical)
            .map(|(_, info)| info.physical_disk)
            .unwrap_or(logical)
    }

    fn resolve_ghost_part(&self, logical_disk: u32, logical_part: u32) -> Option<(u32, u32)> {
        let key = format!("{logical_disk}:{logical_part}");
        if let Some((_, info)) = self.disk_data.iter().find(|(idx, _)| *idx == logical_disk) {
            for (pk, part) in &info.partitions {
                if *pk == key {
                    return Some((info.physical_disk, part.number));
                }
            }
            // 分区表里没有这个 key：目标分区已被清理/重建，旧条目失效。
            // logical_part 是下拉框里解析出的枚举下标，不是真实分区号；
            // 退回 0 会变成 Ghost 的 dst=N:0（分区号从 1 起，无效）→
            // ABORT 10099 Invalid target partition specified。这里返回 None，
            // 由调用方提示用户重新选择。
            if logical_part >= 1 {
                return Some((info.physical_disk, logical_part));
            }
            return None;
        }
        if logical_part >= 1 {
            return Some((logical_disk, logical_part));
        }
        None
    }

    // ==================== 安装完成 ====================

    fn on_install_complete(&mut self, ok: bool, output: String) {
        let was_cancelled = self.install_cancelled.load(Ordering::SeqCst);
        self.is_installing = false;
        self.install_cancelled.store(false, Ordering::SeqCst);

        if ok && !self.pending.is_empty() {
            self.clear_disk_preview();
            self.refresh_disk_info(false);
        }
        let pending_optimize = ok
            && !was_cancelled
            && (self.ui.deploy_cb.is_checked() || self.ui.copy_tools_cb.is_checked())
            && self.last_install_windows_letter.is_some();
        if !pending_optimize {
            self.restore_install_panel();
        }

        if was_cancelled {
            self.set_status("安装已取消");
            self.ui.progress.set_value(0.0);
            log::warn!("安装已被用户取消");
            dialog::message_default("安装已被取消");
            self.finish_install();
            return;
        }
        if ok {
            self.ui.progress.set_value(100.0);
            self.set_status("系统安装完成，正在优化...");
            log::info!("系统安装完成");
            self.ui.install_btn.set_label("正在优化...");
            self.ui.install_btn.deactivate();
            self.do_optimize();
        } else {
            self.set_status("系统安装失败");
            let shown: String = output.chars().take(500).collect();
            dialog::alert_default(&format!("系统安装失败:\n{shown}"));
            log::error!("系统安装失败: {output}");
            self.finish_install();
        }
    }

    fn do_optimize(&mut self) {
        let deploy_enabled = self.ui.deploy_cb.is_checked();
        let copy_tools_enabled = self.ui.copy_tools_cb.is_checked();
        let windows_partition = self.last_install_windows_letter.clone();
        let admin_username = self.last_install_admin_username.clone();
        let image_index = self.last_install_image_index;
        let wim_path = self.current_wim_path.clone().unwrap_or_default();
        let tools_path = self.cfg.tools_source_path.clone();
        let disable_firewall = self.ui.firewall_cb.is_checked();
        let tx = super::tx();
        std::thread::spawn(move || {
            let mut warning: Option<String> = None;
            if (deploy_enabled || copy_tools_enabled) && windows_partition.is_some() {
                log::info!(
                    "开始系统优化: 部署应答文件={}, 复制调试工具={}, \
                     关闭防火墙={}, 目标={}, 账户名={admin_username}",
                    if deploy_enabled { "是" } else { "否" },
                    if copy_tools_enabled { "是" } else { "否" },
                    if disable_firewall { "是" } else { "否" },
                    windows_partition.clone().unwrap_or_default()
                );
                let progress = move |status: &str, p: i32| {
                    tx.send(Message::Progress(status.to_string(), p));
                    app::awake();
                };
                let (ok_opt, output_opt) = crate::app::optimize::optimize_system(
                    &windows_partition.clone().unwrap_or_default(),
                    deploy_enabled,
                    &admin_username,
                    &wim_path,
                    image_index,
                    copy_tools_enabled,
                    Some(&progress),
                    &tools_path,
                    disable_firewall,
                );
                if ok_opt {
                    log::info!("系统优化完成");
                } else {
                    log::warn!("系统优化失败: {output_opt}");
                    warning = Some(output_opt);
                }
            }
            tx.send(Message::InstallSuccess(warning));
            app::awake();
        });
    }

    fn show_install_success(&mut self, warning: Option<String>) {
        use std::os::windows::process::CommandExt;
        let mut msg = "系统安装完成！".to_string();
        if let Some(w) = &warning {
            let shown: String = w.chars().take(200).collect();
            msg.push_str(&format!("\n\n优化警告: {shown}"));
        }
        self.finish_install();
        // 【重启统一】GHO 与 WIM 用同一套重启控制：按当前模式读取各自的
        // "自动重启"复选框（GHO=还原后自动重启；WIM=安装完成后重启），
        // 勾选→30 秒倒计时对话框（可取消）；未勾选→询问是否立即重启。
        let auto_restart = if self.is_gho_mode {
            self.ui.gho_restart_cb.is_checked()
        } else {
            self.ui.restart_cb.is_checked()
        };
        if auto_restart {
            log::info!("显示重启倒计时对话框（安装完成，{} 秒）", self.cfg.restart_countdown_sec);
            restart::show(&mut self.ui.win, self.cfg.restart_countdown_sec);
        } else {
            let restart_msg = format!("{msg}\n\n是否立即重启电脑？");
            let choice = dialog::choice2_default(&restart_msg, "是", "否", "");
            if choice == Some(0) {
                log::info!("用户选择立即重启");
                let _ = std::process::Command::new("shutdown")
                    .args(["/r", "/t", "0"])
                    .creation_flags(crate::utils::CREATE_NO_WINDOW)
                    .spawn();
            } else {
                log::info!("用户选择不重启");
            }
        }
    }

    fn finish_install(&mut self) {
        if let Some(mp) = self.current_mount_path.clone() {
            log::info!("卸载 ISO 挂载: {mp}");
            iso::unmount_iso(&mp);
            self.current_mount_path = None;
        }
        self.refresh_disk_info(false);
        self.ui.progress.set_value(0.0);
    }

    fn restore_install_panel(&mut self) {
        self.set_install_panel_enabled(true);
        self.ui.install_btn.set_label("开始安装");
        self.set_disk_buttons_enabled(true);
        self.update_install_button_state();
    }

    fn set_install_panel_enabled(&mut self, _enabled: bool) {
        // fltk 简化：安装进行中禁用一键分区与安装按钮（见各自调用点）；
        // 其余控件保持可用（Python 会禁用，这里不逐个复刻以保证可操作性）。
    }

    // ==================== 盘符查询 ====================

    fn get_letter_by_part_key(&self, part_key: &str) -> String {
        if part_key.is_empty() {
            return String::new();
        }
        for (_, info) in &self.disk_data {
            for (pk, part) in &info.partitions {
                if pk == part_key {
                    if part.letter.is_empty() {
                        return String::new();
                    }
                    return format!("{}:", part.letter.to_uppercase());
                }
            }
        }
        String::new()
    }

    fn resolve_partition_to_diskpart(&self, part_key: &str) -> Option<(u32, u32)> {
        if part_key.is_empty() {
            return None;
        }
        for (_, info) in &self.disk_data {
            for (pk, part) in &info.partitions {
                if pk == part_key {
                    return Some((info.physical_disk, part.number));
                }
            }
        }
        None
    }

    fn try_assign_esp_drive_letter(&mut self, part_key: &str) -> String {
        let Some((phys, part_num)) = self.resolve_partition_to_diskpart(part_key) else {
            log::warn!("无法解析分区 {part_key} 到 diskpart 参数");
            return String::new();
        };
        if part_num == 0 {
            return String::new();
        }
        let Some(letter) = diskpart::find_available_drive_letter() else {
            log::warn!("未找到可用盘符");
            return String::new();
        };
        log::info!(
            "为分区 {part_key} 自动分配盘符 {letter}: (物理磁盘={phys}, 分区号={part_num})"
        );
        let (ok, output) = diskpart::assign_drive_letter(phys, part_num, Some(letter));
        if !ok {
            log::warn!("自动分配盘符失败: {output}");
            return String::new();
        }
        format!("{letter}:")
    }

    // ==================== 自动还原倒计时 ====================

    fn start_countdown(&mut self, seconds: i64) {
        log::info!("启动自动还原倒计时: {seconds}秒");
        self.cancel_countdown();
        self.countdown_running = true;
        self.countdown_remaining = seconds;
        self.update_countdown();
    }

    fn update_countdown(&mut self) {
        if self.countdown_remaining <= 0 {
            self.countdown_running = false;
            self.set_countdown_text("正在自动还原...");
            self.set_status("倒计时结束，开始自动还原...");
            self.run_auto_restore();
            return;
        }
        let s = self.countdown_remaining;
        self.set_countdown_text(&format!("⏱{s}s后自动还原"));
        self.countdown_remaining -= 1;
        self.countdown_id = Some(app::add_timeout3(1.0, |_| {
            App::with(|a| a.update_countdown())
        }));
    }

    fn cancel_countdown(&mut self) {
        if let Some(h) = self.countdown_id.take() {
            app::remove_timeout3(h);
        }
        self.countdown_running = false;
    }

    fn run_auto_restore(&mut self) {
        log::info!("自动还原: 开始执行");
        self.cancel_countdown();
        self.set_countdown_text("正在执行自动还原...");
        let plan_path = self.cfg.plan_path.clone();
        if !plan_path.is_empty()
            && Path::new(&plan_path).exists()
            && self.pending.is_empty()
        {
            log::info!("自动还原: 静默加载方案 {plan_path}");
            self.load_scheme(&plan_path, true);
        }
        if self.current_wim_path.is_none() {
            let image_path = self.ui.source_path_in.value();
            if !image_path.is_empty() && Path::new(&image_path).exists() {
                log::info!("自动还原: 加载镜像 {image_path}");
                self.load_wim_info(&image_path);
            } else {
                log::warn!("自动还原失败: 镜像文件不存在 '{image_path}'");
                self.set_status("自动还原失败：镜像文件不存在");
                return;
            }
        }
        let _ = self.current_wim_path.as_ref();
        self.on_install_system(true);
    }

    // ==================== 方案保存 / 加载 / 追踪 ====================

    fn invalidate_scheme(&mut self) {
        if !self.scheme_active || self.applying_scheme {
            return;
        }
        self.scheme_active = false;
        self.current_scheme = "无".to_string();
        self.ui.current_scheme_label.set_label("无");
        log::info!("UI 配置已修改，当前方案失效（配置与方案文件不再一致）");
    }

    fn gather_scheme_data(&self) -> SchemeData {
        let partitions: Vec<PartitionPlan> = self
            .pending
            .iter()
            .map(|pp| PartitionPlan {
                disk_physical: pp.disk_physical,
                style: pp.style.clone(),
                esp_enabled: pp.esp_enabled,
                esp_size_mb: pp.esp_size_mb,
                data_parts: pp.data_parts.clone(),
            })
            .collect();
        let (boot_partition, windows_partition) = self.boot_windows_keys();
        let mut install = InstallSettings {
            image_path: self.ui.source_path_in.value(),
            image_index_text: self.ui.image_choice.choice().unwrap_or_default(),
            format: self.ui.format_cb.is_checked(),
            auto_repair_boot: self.ui.repair_boot_cb.is_checked(),
            copy_tools: self.ui.copy_tools_cb.is_checked(),
            deploy_unattend: self.ui.deploy_cb.is_checked(),
            disable_defender: false, // 不再通过 unattend 处理，改为 DISM 后离线注册表修改
            disable_firewall: self.ui.firewall_cb.is_checked(),
            admin_username: self.ui.admin_user_in.value(),
            restart_after_install: self.ui.restart_cb.is_checked(),
            target_disk: self.selected_disk_idx(),
            boot_partition,
            windows_partition,
            ..Default::default()
        };
        if self.is_gho_mode {
            install.image_type = "gho".to_string();
            install.gho_mode = if self.ui.gho_mode_part_rb.value() {
                "partition".to_string()
            } else {
                "disk".to_string()
            };
            install.ghost_path = self.ui.ghost_path_in.value();
            install.gho_repair_boot = self.ui.gho_repair_boot_cb.is_checked();
            if self.ui.gho_mode_part_rb.value() {
                let t = self.ui.gho_part_choice.choice().unwrap_or_default();
                if let Some((ld, lp)) = parse_leading_disk_part(&t) {
                    match self.resolve_ghost_part(ld, lp) {
                        Some((phys, pn)) => {
                            install.gho_disk_index = phys.to_string();
                            install.gho_partition_index = pn.to_string();
                        }
                        None => {
                            // 目标分区已失效：不写入方案，避免下次自动还原
                            // 从失效条目解析出分区号 0 传给 Ghost。
                            install.gho_disk_index = String::new();
                            install.gho_partition_index = String::new();
                        }
                    }
                } else {
                    install.gho_disk_index = String::new();
                    install.gho_partition_index = String::new();
                }
            } else {
                let t = self.ui.gho_disk_choice.choice().unwrap_or_default();
                if let Some(ld) = parse_leading_disk(&t) {
                    install.gho_disk_index = self.resolve_ghost_disk(ld).to_string();
                } else {
                    install.gho_disk_index = "0".to_string();
                }
                install.gho_partition_index = "0".to_string();
            }
        } else {
            install.image_type = "wim".to_string();
        }
        SchemeData {
            partitions,
            install,
        }
    }

    fn save_scheme(&mut self) {
        let data = self.gather_scheme_data();
        let default_name = if self.current_scheme.is_empty() || self.current_scheme == "无" {
            String::new()
        } else {
            self.current_scheme
                .chars()
                .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
                .collect()
        };
        let mut dlg = dialog::FileDialog::new(dialog::FileDialogType::BrowseSaveFile);
        dlg.set_title("保存当前方案");
        // FLTK 用 '\n' 分隔多个过滤项（末尾会自动追加 "All Files (*.*)"）
        dlg.set_filter("BAT 脚本\t*.bat");
        if !default_name.is_empty() {
            dlg.set_preset_file(&format!("{default_name}.bat"));
        }
        dlg.show();
        let path = dlg.filename();
        if path.as_os_str().is_empty() {
            return;
        }
        let path = path.to_string_lossy().into_owned();
        let bat_content = build_combined_bat(&data);
        if let Err(e) = std::fs::write(&path, bat_content) {
            log::error!("保存方案失败: {e}");
            dialog::alert_default(&format!("保存方案失败:\n{e}"));
            return;
        }
        self.cfg.plan_path = path.clone();
        self.cfg.save(None);
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.current_scheme = stem.clone();
        self.ui.current_scheme_label.set_label(&stem);
        self.scheme_active = true;
        self.set_status(&format!(
            "已保存方案: {}",
            Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        log::info!("方案已保存: {path}");
        dialog::message_default(&format!("方案已保存:\n{path}"));
    }

    fn load_scheme(&mut self, from_path: &str, silent: bool) {
        let path = if from_path.is_empty() {
            let mut dlg = dialog::FileDialog::new(dialog::FileDialogType::BrowseFile);
            dlg.set_title("加载方案");
            // FLTK 用 '\n' 分隔多个过滤项（末尾会自动追加 "All Files (*.*)"）
            dlg.set_filter("BAT 脚本\t*.bat");
            dlg.show();
            let p = dlg.filename();
            if p.as_os_str().is_empty() {
                return;
            }
            p.to_string_lossy().into_owned()
        } else {
            from_path.to_string()
        };
        let bat_text = match std::fs::read_to_string(&path) {
            Ok(t) => t.trim_start_matches('\u{feff}').to_string(),
            Err(e) => {
                log::error!("加载方案失败: {e}");
                if !silent {
                    dialog::alert_default(&format!("读取方案失败:\n{e}"));
                }
                return;
            }
        };
        let Some(data) = parse_plan_metadata(&bat_text) else {
            log::error!("加载方案失败: 未找到方案元数据块");
            if !silent {
                dialog::alert_default("读取方案失败:\n未找到方案元数据块");
            }
            return;
        };
        self.apply_scheme(&data);
        self.cfg.plan_path = path.clone();
        self.cfg.save(None);
        let stem = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.current_scheme = stem.clone();
        self.ui.current_scheme_label.set_label(&stem);
        self.scheme_active = true;
        self.set_status(&format!(
            "已加载方案: {}",
            Path::new(&path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        log::info!("方案已加载: {path}");
        if !silent {
            dialog::message_default(&format!("方案已加载:\n{path}"));
        }
    }

    fn apply_scheme(&mut self, data: &SchemeData) {
        self.applying_scheme = true;
        log::info!(
            "应用方案: 硬盘数={}, 镜像='{}'",
            data.partitions.len(),
            data.install.image_path
        );
        // 【过滤】方案里保存的分区计划可能引用了当前已经不存在的物理盘
        // （比如之前保存时插着 2 块盘，现在只剩 1 块）。只保留当前枚举到
        // 的盘对应的计划，避免一键分区时 `select disk N` 找不到盘而失败。
        let current_phys_set: std::collections::HashSet<u32> = self
            .disk_data
            .iter()
            .map(|(_, info)| info.physical_disk)
            .collect();
        if !data.partitions.is_empty() {
            self.pending.clear();
            for p in &data.partitions {
                if !current_phys_set.contains(&(p.disk_physical as u32)) {
                    log::warn!(
                        "方案跳过不存在的物理盘 {}，当前枚举物理盘: {:?}",
                        p.disk_physical,
                        current_phys_set
                            .iter()
                            .map(|d| d.to_string())
                            .collect::<Vec<_>>()
                    );
                    continue;
                }
                self.pending.push(PendingPartition {
                    disk_physical: p.disk_physical,
                    disk_idx: 0,
                    style: p.style.clone(),
                    esp_enabled: p.esp_enabled,
                    esp_size_mb: p.esp_size_mb,
                    data_parts: p.data_parts.clone(),
                });
            }
            self.rebuild_tree();
        } else {
            self.clear_disk_preview();
        }
        let inst = &data.install;
        if !inst.image_path.is_empty() {
            self.ui.source_path_in.set_value(&inst.image_path);
            self.applying_scheme = true;
            self.load_wim_info(&inst.image_path);
        }
        if inst.image_type.eq_ignore_ascii_case("gho") {
            self.set_gho_mode(true);
            if inst.gho_mode == "partition" || inst.gho_mode == "pload" {
                self.ui.gho_mode_part_rb.set(true);
            } else {
                self.ui.gho_mode_disk_rb.set(true);
            }
            self.ui.ghost_path_in.set_value(&inst.ghost_path);
            self.scheme_gho_disk = Some(inst.gho_disk_index.clone());
            self.scheme_gho_partition = Some(inst.gho_partition_index.clone());
            self.refresh_gho_targets();
        } else {
            self.set_gho_mode(false);
            self.scheme_gho_disk = None;
            self.scheme_gho_partition = None;
        }
        if !inst.image_index_text.is_empty() {
            crate::ui::set_choice_by_text(&mut self.ui.image_choice, &inst.image_index_text);
        }
        self.ui.format_cb.set_checked(inst.format);
        self.ui.repair_boot_cb.set_checked(inst.auto_repair_boot);
        self.ui.copy_tools_cb.set_checked(inst.copy_tools);
        self.ui.deploy_cb.set_checked(inst.deploy_unattend);
        self.ui.defender_cb.set_checked(inst.disable_defender);
        self.ui.firewall_cb.set_checked(inst.disable_firewall);
        self.ui.admin_user_in.set_value(&inst.admin_username);
        self.ui
            .restart_cb
            .set_checked(inst.restart_after_install);
        // GHO 分区还原后修复引导：默认勾选（与 UI 初始态、InstallSettings 缺省一致）。
        // 旧方案文件没有此字段时按默认 true 处理。
        self.ui.gho_repair_boot_cb.set_checked(inst.gho_repair_boot);
        // 同步到 config.ini
        self.cfg.copy_tools = inst.copy_tools;
        self.cfg.deploy_unattend = inst.deploy_unattend;
        self.cfg.disable_defender = inst.disable_defender;
        self.cfg.disable_firewall = inst.disable_firewall;
        self.cfg.auto_restart = inst.restart_after_install;
        self.cfg.save(None);
        if !self.is_gho_mode && !inst.target_disk.is_empty() {
            self.select_disk_in_combo(inst.target_disk.parse().unwrap_or(0));
        }
        self.applying_scheme = false;
        self.update_install_button_state();
    }

    // ==================== 消息处理（主循环）====================

    /// 把后台线程经日志 sink 推入面板队列的日志行刷到 TextDisplay。
    pub fn drain_log(&mut self) {
        self.ui.log_panel.drain();
    }

    pub fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::DiskInfoReady { disks, silent } => {
                self.display_disk_info(disks, silent);
            }
            Message::Progress(status, progress) => {
                self.set_status(&status);
                self.ui.progress.set_value(progress as f64);
            }
            Message::InstallComplete(ok, output, windows_letter) => {
                // 记录本次安装的系统分区盘符（← `_last_install_windows_partition`，
                // Python 在 install_worker 两个分支中都会赋值，gui.py 2085/2087）
                if let Some(l) = windows_letter {
                    self.last_install_windows_letter = Some(l);
                }
                self.on_install_complete(ok, output);
            }
            Message::InstallSuccess(warning) => {
                self.restore_install_panel();
                self.show_install_success(warning);
            }
            Message::PostPartitionRefresh(disks) => {
                self.clear_disk_preview();
                self.display_disk_info(disks, true);
            }
        }
    }
}

// ==================== 自由函数 ====================

/// MB → 显示文本（≥1024MB 用 GB 1 位小数，否则 MB），← `_format_size_mb`
fn fmt_size_mb(size_mb: i64) -> String {
    if size_mb >= 1024 {
        format!("{:.1}GB", size_mb as f64 / 1024.0)
    } else {
        format!("{size_mb}MB")
    }
}

/// 从 "磁盘0: ..." 提取磁盘序号（← `_extract_disk_idx`）
fn extract_disk_idx(text: &str) -> String {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("磁盘") else {
        return String::new();
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits
}

/// 解析 "磁盘N" 前缀（← `re.match(r'^磁盘(\d+)')`）
fn parse_leading_disk(text: &str) -> Option<u32> {
    extract_disk_idx(text).parse().ok()
}

/// 解析 "N:M" 前缀（← `re.match(r'^(\d+):(\d+)')`）
fn parse_leading_disk_part(text: &str) -> Option<(u32, u32)> {
    let t = text.trim();
    let (d, rest) = t.split_once(':')?;
    let disk: u32 = d.parse().ok()?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let part: u32 = digits.parse().ok()?;
    Some((disk, part))
}

/// 从磁盘信息中确定引导/系统分区盘符（← `_find_letters_for_disk`）
fn find_letters_for_disk(disks: &[(u32, DiskInfo)], phys: i64) -> (String, String) {
    let mut boot = String::new();
    let mut system = String::new();
    for (_, info) in disks {
        if info.physical_disk as i64 != phys {
            continue;
        }
        let mut keys: Vec<&String> = info.partitions.iter().map(|(k, _)| k).collect();
        keys.sort();
        for k in &keys {
            let part = info.partitions.iter().find(|(pk, _)| pk == *k).map(|(_, p)| p);
            if let Some(p) = part {
                if boot.is_empty() && p.ptype == "ESP" && !p.letter.is_empty() {
                    boot = format!("{}:", p.letter.to_uppercase());
                }
                if system.is_empty()
                    && p.format.eq_ignore_ascii_case("NTFS")
                    && !p.letter.is_empty()
                    && p.ptype != "ESP"
                {
                    system = format!("{}:", p.letter.to_uppercase());
                }
            }
        }
        if boot.is_empty() {
            for k in &keys {
                let part = info.partitions.iter().find(|(pk, _)| pk == *k).map(|(_, p)| p);
                if let Some(p) = part {
                    if p.format.eq_ignore_ascii_case("FAT32") && !p.letter.is_empty() {
                        boot = format!("{}:", p.letter.to_uppercase());
                        break;
                    }
                }
            }
        }
        if boot.is_empty() && !system.is_empty() {
            boot = system.clone();
        }
        break;
    }
    (boot, system)
}

/// Ghost 命令执行（工作目录 + 捕获输出）。返回 (returncode) 或错误文本。
/// 把 `"exe 路径" 其余参数` 拆成 (exe, rest)。
/// `run_ghost` 需要把 exe 之外的所有文本**原样**交给 CreateProcess
/// （raw_arg），拆分时 rest 保留首部空格，拼接后与原串逐字节一致。
/// 预览数据分区的计划 Ghost 分区号基数（与 diskpart 一键分区的创建顺序一致）：
/// GPT 建了 ESP 则 ESP=1/MSR=2 占位，数据分区从 3 起；未建 ESP 则 MSR=1，从 2 起；
/// MBR 无占位分区，从 1 起。
fn gho_preview_base(pp: &PendingPartition) -> i64 {
    if pp.style.eq_ignore_ascii_case("gpt") {
        if pp.esp_enabled {
            3
        } else {
            2
        }
    } else {
        1
    }
}

/// 某盘一键分区预览的 GHO「目标分区」候选（仅数据分区）。
/// 前缀 `盘号:计划分区号` 供 on_gho_install 直接取计划号——盘还没实际分区，
/// 不能查实际分区表；其余文本与树里的预览行保持一致。
fn gho_preview_entries(disk_idx: u32, pp: &PendingPartition) -> Vec<String> {
    let base = gho_preview_base(pp);
    pp.data_parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let size_text = match part.size_mb {
                Some(v) => fmt_size_mb(v),
                None => "剩余空间".to_string(),
            };
            format!(
                "{disk_idx}:{} (预览) 分区{}    {size_text}    {}",
                base + i as i64,
                i + 1,
                part.fs
            )
        })
        .collect()
}

fn split_ghost_cmd(cmd: &str) -> (String, String) {
    let cmd = cmd.trim_start();
    if let Some(r) = cmd.strip_prefix('"') {
        return match r.find('"') {
            Some(i) => (r[..i].to_string(), r[i + 1..].to_string()),
            None => (r.to_string(), String::new()),
        };
    }
    match cmd.find(' ') {
        Some(i) => (cmd[..i].to_string(), cmd[i..].to_string()),
        None => (cmd.to_string(), String::new()),
    }
}

/// 运行 Ghost 并返回 `(返回码, 控制台输出)`。
/// stdout / stderr 各写入**独立**临时文件（同一文件挂两个句柄会各自持有
/// 写指针、互相覆盖），退出后按 ACP 解码合并，再删除临时文件。
fn run_ghost(cmd: &str, workdir: &str) -> Result<(i32, String), String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // 【修复】与 Python `Popen(整串)` 对齐：Ghost 自带命令行解析器，整串必须
    // 原样交给 CreateProcess。之前经 `cmd /c` + std 默认 MSVCRT 转义，
    // `src="x.GHO"` 的引号被改写成 `\"`，Ghost 端解析成 src=`\x.GHO\` →
    // 找不到镜像、瞬间退出码 1。这里拆出 exe 路径交给 Command::new（std 会
    // 按规则加引号），其余文本用 raw_arg 原样拼接，最终命令行与 Python 一致。
    let (exe, rest) = split_ghost_cmd(cmd);
    if exe.is_empty() {
        return Err("Ghost 命令为空".to_string());
    }
    let pid = std::process::id();
    let temp = std::env::temp_dir();
    let out_path = temp.join(format!("sks_ghost_out_{pid}.log"));
    let err_path = temp.join(format!("sks_ghost_err_{pid}.log"));
    let out_file =
        std::fs::File::create(&out_path).map_err(|e| format!("创建临时输出文件失败: {e}"))?;
    let err_file =
        std::fs::File::create(&err_path).map_err(|e| format!("创建临时错误文件失败: {e}"))?;
    let mut command = Command::new(&exe);
    // raw_arg 前会自动补一个分隔空格，rest 的前导空格去掉以免双空格
    let rest = rest.trim_start();
    if !rest.is_empty() {
        command.raw_arg(rest);
    }
    let status = command
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out_file))
        .stderr(Stdio::from(err_file))
        .creation_flags(crate::utils::CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    let code = status.code().unwrap_or(-1);
    // 退出后回读（Ghost 输出按 ACP 解码；读取失败不致命，输出置空）
    let read_acp = |p: &std::path::Path| {
        std::fs::read(p)
            .map(|b| crate::utils::decode_acp(&b))
            .unwrap_or_default()
    };
    let mut output = read_acp(&out_path);
    let err_out = read_acp(&err_path);
    if !err_out.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&err_out);
    }
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_file(&err_path);
    Ok((code, output))
}

/// 读取 Ghost debug.log 末尾 60 行（← `_read_ghost_debug_tail`）。
/// 候选顺序：工作目录 → %TEMP% → Ghost 所在目录（Python 版同款顺序）。
/// 按字节读取后 ACP 解码（debug.log 可能含本地编码文本，UTF-8 校验会失败）。
fn read_ghost_debug_tail(workdir: &str, resolved_ghost: &str) -> Option<String> {
    let mut candidates = Vec::new();
    if !workdir.is_empty() {
        candidates.push(Path::new(workdir).join("debug.log"));
    }
    if let Ok(t) = std::env::var("TEMP") {
        if !t.is_empty() {
            candidates.push(Path::new(&t).join("debug.log"));
        }
    }
    if let Some(parent) = Path::new(resolved_ghost).parent() {
        candidates.push(parent.join("debug.log"));
    }
    for c in &candidates {
        if !c.is_file() {
            continue;
        }
        if let Ok(bytes) = std::fs::read(c) {
            let text = crate::utils::decode_acp(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(60);
            return Some(lines[start..].join("\n"));
        }
    }
    None
}

/// 展开 `%VAR%` 环境变量（← Python `os.path.expandvars`，简化实现）
fn expand_vars(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut name = String::new();
            let mut closed = false;
            for n in chars.by_ref() {
                if n == '%' {
                    closed = true;
                    break;
                }
                name.push(n);
            }
            if closed {
                if let Ok(v) = std::env::var(&name) {
                    out.push_str(&v);
                } else {
                    out.push('%');
                    out.push_str(&name);
                    out.push('%');
                }
            } else {
                out.push('%');
                out.push_str(&name);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// run_ghost 拆分：带引号的 exe 路径 + 原样参数文本。
    #[test]
    fn split_ghost_cmd_quoted_exe() {
        let (exe, rest) = split_ghost_cmd(
            r#""X:\Program Files (x86)\ghost\ghost64.exe" -clone,mode=load,src="test.GHO",dst=1 -sure -fx -rb"#,
        );
        assert_eq!(exe, r"X:\Program Files (x86)\ghost\ghost64.exe");
        assert_eq!(
            rest,
            r#" -clone,mode=load,src="test.GHO",dst=1 -sure -fx -rb"#
        );
    }

    /// run_ghost 拆分：无引号 exe、无参数的边界情形。
    #[test]
    fn split_ghost_cmd_edge_cases() {
        let (exe, rest) = split_ghost_cmd(r"C:\tools\g64.exe -sure");
        assert_eq!(exe, r"C:\tools\g64.exe");
        assert_eq!(rest, " -sure");

        let (exe, rest) = split_ghost_cmd(r#""D:\my ghost\g.exe""#);
        assert_eq!(exe, r"D:\my ghost\g.exe");
        assert_eq!(rest, "");

        let (exe, rest) = split_ghost_cmd("g.exe");
        assert_eq!(exe, "g.exe");
        assert_eq!(rest, "");

        let (exe, rest) = split_ghost_cmd("   g.exe -x");
        assert_eq!(exe, "g.exe");
        assert_eq!(rest, " -x");
    }

    /// debug.log 末尾读取：优先取工作目录下的 debug.log，返回末 60 行。
    #[test]
    fn ghost_debug_tail_reads_workdir_log() {
        let base = std::env::temp_dir().join(format!("sks_tail_test_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let log = base.join("debug.log");
        // 70 行 → 末 60 行应从第 10 行起
        let content: String = (0..70).map(|i| format!("logline{i:02}\n")).collect();
        std::fs::write(&log, content).unwrap();

        let tail = read_ghost_debug_tail(
            &base.to_string_lossy(),
            r"Z:\nonexistent\ghost64.exe",
        )
        .expect("应读到工作目录的 debug.log");
        assert!(tail.contains("logline69"));
        assert!(tail.contains("logline10"));
        assert!(!tail.contains("logline09"), "应只含末 60 行");
        assert!(!tail.contains("logline00"));

        let _ = std::fs::remove_file(&log);
        let _ = std::fs::remove_dir(&base);
    }

    /// GHO 预览候选的计划分区号：GPT+ESP 从 3 起，GPT 无 ESP 从 2 起，MBR 从 1 起。
    #[test]
    fn gho_preview_entries_planned_numbers() {
        let gpt = PendingPartition {
            disk_physical: 0,
            disk_idx: 0,
            style: "gpt".to_string(),
            esp_enabled: true,
            esp_size_mb: 260,
            data_parts: vec![
                crate::app::scheme::bat::DataPart {
                    size_mb: Some(102400),
                    fs: "ntfs".to_string(),
                },
                crate::app::scheme::bat::DataPart {
                    size_mb: None,
                    fs: "ntfs".to_string(),
                },
            ],
        };
        let v = gho_preview_entries(0, &gpt);
        assert_eq!(v.len(), 2);
        assert!(v[0].starts_with("0:3 (预览) 分区1"));
        assert!(v[0].ends_with("ntfs"));
        assert!(v[1].starts_with("0:4 (预览) 分区2"));
        assert!(v[1].contains("剩余空间"));

        let mbr = PendingPartition {
            style: "mbr".to_string(),
            data_parts: vec![crate::app::scheme::bat::DataPart::default()],
            ..gpt.clone()
        };
        let v = gho_preview_entries(1, &mbr);
        assert_eq!(v.len(), 1);
        assert!(v[0].starts_with("1:1 (预览) 分区1"));

        let gpt_no_esp = PendingPartition {
            esp_enabled: false,
            ..gpt.clone()
        };
        let v = gho_preview_entries(2, &gpt_no_esp);
        assert!(v[0].starts_with("2:2 (预览) 分区1"));
    }

    /// 预览前缀能被 parse_leading_disk_part 解析（on_gho_install 直接取计划号）。
    #[test]
    fn gho_preview_prefix_parses() {
        let (ld, planned) = parse_leading_disk_part("0:3 (预览) 分区1    100.00GB    ntfs")
            .expect("should parse");
        assert_eq!((ld, planned), (0, 3));
    }

}
