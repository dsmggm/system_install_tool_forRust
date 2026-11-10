//! 一键分区对话框（← `gui.py::OneClickPartitionDialog`，文档 §7.3）
//!
//! 分区表 Radio（GPT/MBR）、EFI Check + 大小 Entry + 单位 Combo（MB/GB）、
//! 分区数量 Spinner（1-8），动态分区行（大小 + 单位 + 文件系统 + 提示），
//! 底部剩余空间 Label，`保存`/`取消`。
//!
//! 联动：
//! - MBR 强制取消并禁用 ESP；
//! - 实时 `_update_remain`：已用 = ESP + MSR(16MB, 仅 GPT) + 手动合计；
//!   剩余均分给自动行，余数给最后一个自动行；全手动有剩余并入最后分区并回写；
//! - 溢出显示 `空间不足! 已超出 NMB`；
//! - `_ok` 校验后返回 `PartitionDialogResult`（最后一个自动行 `size_mb = None`）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fltk::button::CheckButton;
use fltk::enums::{Align, Color, Event, Font, FrameType};
use fltk::frame::Frame;
use fltk::group::{Flex, Group};
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::misc::Spinner;
use fltk::prelude::*;
use fltk::window::DoubleWindow;

use crate::app::scheme::bat::DataPart;

use super::state::PartitionDialogResult;
use super::{app_icon, msg, theme};

/// 对话框输入数据（← `disk_info` 字典）
pub struct DialogData {
    pub disk_idx: u32,
    pub model: String,
    pub size_text: String,
    pub total_mb: i64,
}

/// 每个动态分区行的控件（← `self.part_rows[i]`）
#[derive(Clone)]
struct PartRow {
    size_in: Input,
    unit_choice: Choice,
    fs_choice: Choice,
    hint: Frame,
}

const FS_ITEMS: [&str; 4] = ["ntfs", "fat32", "exfat", "refs"];
const UNIT_ITEMS: [&str; 2] = ["MB", "GB"];

/// 前向声明的无参刷新闭包槽（打破 update/rebuild 相互递归）。
type UpdateFn = Rc<RefCell<Box<dyn FnMut()>>>;
/// 前向声明的按分区数重建闭包槽。
type RebuildFn = Rc<RefCell<Box<dyn FnMut(i32)>>>;

const RED: (u8, u8, u8) = (0xff, 0x00, 0x00);

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba_tuple((r, g, b, 255))
}

/// 分段条选项的视觉状态：
/// 选中 → 主色实底 + 白字；未选中 → 浅底 + 文字色。悬停时再亮一档。
fn apply_segment_visual(btn: &mut fltk::button::ToggleButton, hover: bool) {
    let active = btn.value();
    if active {
        // Fl_Button 选中态绘制用的是 selection_color()
        btn.set_color(theme::rgb(theme::INPUT_BG()));
        btn.set_selection_color(theme::rgb(if hover {
            theme::PRIMARY_HOVER()
        } else {
            theme::PRIMARY()
        }));
        btn.set_label_color(Color::White);
    } else {
        btn.set_color(theme::rgb(if hover {
            theme::HOVER_SOFT()
        } else {
            theme::INPUT_BG()
        }));
        btn.set_label_color(theme::rgb(theme::TEXT()));
    }
}

/// 分段按钮统一样式 + 悬停/选中联动（替代通用 add_hover_effect：
/// 通用悬停会把"选中的主色底"在 Leave 时刷回未选中底色）。
fn style_segment_button(btn: &mut fltk::button::ToggleButton) {
    btn.set_frame(FrameType::RFlatBox);
    btn.set_down_frame(FrameType::RFlatBox);
    btn.set_label_font(Font::HelveticaBold);
    btn.set_label_size(12);
    btn.set_align(Align::Center | Align::Inside);
    apply_segment_visual(btn, false);
    btn.handle(|b, ev| match ev {
        Event::Enter => {
            apply_segment_visual(b, true);
            b.redraw();
            true
        }
        Event::Leave => {
            apply_segment_visual(b, false);
            b.redraw();
            true
        }
        _ => false,
    });
}

/// MB → 显示文本（≥1024MB 用 GB 2 位小数，否则 MB）。
fn fmt_size(mb: i64) -> String {
    if mb >= 1024 {
        format!("{:.2}GB", mb as f64 / 1024.0)
    } else {
        format!("{mb}MB")
    }
}

/// 解析大小输入（值 + 单位）为 MB；无效或 ≤0 返回 None。
fn parse_size(text: &str, unit: &str) -> Option<i64> {
    let v: f64 = text.trim().parse().ok()?;
    if v <= 0.0 {
        return None;
    }
    Some(if unit == "GB" {
        (v * 1024.0) as i64
    } else {
        v as i64
    })
}

/// 一行计算结果。
struct RowCalc {
    size_mb: Option<i64>,
    invalid: bool,
    is_auto: bool,
}

/// 计算所有行的 size_mb / invalid / is_auto，并返回 (manual_mb, remain)。
fn calc_rows(rows: &[PartRow], total_mb: i64, used: i64) -> (Vec<RowCalc>, i64, i64) {
    let mut manual_mb = 0i64;
    let mut calcs: Vec<RowCalc> = Vec::with_capacity(rows.len());
    for row in rows {
        let s = row.size_in.value();
        let unit = row.unit_choice.choice().unwrap_or_default();
        let (size_mb, invalid) = if s.trim().is_empty() {
            (None, false)
        } else {
            match parse_size(&s, &unit) {
                Some(mb) => {
                    manual_mb += mb;
                    (Some(mb), false)
                }
                None => (None, true),
            }
        };
        let is_auto = size_mb.is_none() && !invalid;
        calcs.push(RowCalc {
            size_mb,
            invalid,
            is_auto,
        });
    }
    let remain = total_mb - used - manual_mb;
    (calcs, manual_mb, remain)
}

/// 弹出对话框并阻塞至关闭；取消返回 `None`。
pub fn show(parent: &mut DoubleWindow, data: DialogData) -> Option<PartitionDialogResult> {
    let total_mb = data.total_mb;

    // 【陷阱】本函数在按钮回调里执行，此时若存在残留的 `Fl_Group::current()`
    // （如 `rebuild_tree` 的 tree.begin()/end() 后 current 停在树的父组上），
    // `DoubleWindow::new` 会把对话框当作**子窗口**挂进那个组：不再以顶层窗口
    // 存在（EnumWindows 找不到、无标题栏、坐标/尺寸被父组布局改写），用户看
    // 到的就是"点了没弹框"。创建顶层窗口前必须清掉 current group。
    Group::set_current(None::<&Group>);
    let (dx, dy, dw, dh) = super::dialog_geometry(parent, 560, 612, 520, 548);
    let mut win = DoubleWindow::new(dx, dy, dw, dh, "一键分区");
    win.size_range(dw.min(520), dh.min(548), 0, 0);
    win.set_color(theme::rgb(theme::BG));
    win.set_icon(Some(app_icon()));
    win.draw(|w| theme::draw_bg_gradient(w));
    win.begin();

    // 顶层 Flex 必须铺满窗口客户区，否则整棵控件树都是 0x0（什么都不显示）。
    // `size_of_parent()` 立即按窗口尺寸设置一次；`win.resizable(&root)`（见下方）
    // 让窗口缩放时同步调整——与主窗口 `App` 的做法保持一致。
    let mut root = Flex::default().column().size_of_parent();
    root.set_margin(16);
    root.set_spacing(10);
    root.set_color(theme::rgb(theme::BG));

    // ---- 页头：磁盘信息条（标题 + 型号/容量）----
    let mut header = Flex::default().row();
    header.set_color(theme::rgb(theme::BG));
    header.set_spacing(10);
    header.set_margin(0);
    let mut header_icon = Frame::default().with_label("HD");
    header_icon.set_frame(FrameType::RFlatBox);
    header_icon.set_color(theme::rgb(theme::PRIMARY));
    header_icon.set_label_color(Color::White);
    header_icon.set_label_font(Font::HelveticaBold);
    header_icon.set_label_size(13);
    header_icon.set_align(Align::Center | Align::Inside);
    header.fixed(&header_icon, 34);
    let mut info = Flex::default().column();
    info.set_color(theme::rgb(theme::BG));
    info.set_spacing(0);
    info.set_margin(0);
    let disk_title = theme::title(&format!("磁盘 {} 分区", data.disk_idx));
    info.fixed(&disk_title, 18);
    let mut disk_sub = Frame::default().with_label(&format!(
        "{}  ·  总容量 {} ({}MB)",
        data.model, data.size_text, total_mb
    ));
    disk_sub.set_align(Align::Left | Align::Inside);
    disk_sub.set_label_color(theme::rgb(theme::MUTED));
    disk_sub.set_label_size(11);
    info.end();
    header.end();
    root.fixed(&header, 42);

    // ---- 卡片①：引导设置（分区表格式 + ESP）----
    let mut boot_card = Flex::default().column();
    theme::style_card(&mut boot_card);
    boot_card.set_spacing(8);
    let boot_head = theme::title("引导设置");
    boot_card.fixed(&boot_head, 20);

    let mut style_row = Flex::default().row();
    style_row.set_color(theme::rgb(theme::BG()));
    style_row.set_spacing(8);
    let style_lbl = theme::form_label("分区表格式");
    style_row.fixed(&style_lbl, 80);
    // GPT/MBR 使用真正的分段控件外观：两个 ToggleButton 拼成一体。
    // 用 RadioRoundButton 逻辑但视觉上改为扁平连体段。
    let mut style_seg = Flex::default().row();
    theme::style_segmented(&mut style_seg);
    style_seg.set_spacing(0); // 两段之间不留缝，视觉连体
    let mut style_gpt = fltk::button::ToggleButton::default().with_label("GPT (UEFI)");
    style_gpt.set_type(fltk::button::ButtonType::Radio);
    style_gpt.set(true); // 默认选中 GPT
    style_segment_button(&mut style_gpt);
    style_seg.fixed(&style_gpt, 110);
    let mut style_mbr = fltk::button::ToggleButton::default().with_label("MBR (BIOS)");
    style_mbr.set_type(fltk::button::ButtonType::Radio);
    style_segment_button(&mut style_mbr);
    style_seg.fixed(&style_mbr, 110);
    style_seg.end();
    // 【分段条宽度修复】此前 style_seg 不固定宽度，会在 style_row 里被拉伸
    // 到整行剩余宽度，两个 110px 按钮之外露出大片灰底框。固定其宽度为
    // 两段之和 + 上下左右各 3px 内边距 = 226px，底框与按钮严丝合缝。
    style_row.fixed(&style_seg, 226);
    style_row.end();
    boot_card.fixed(&style_row, 30);

    let mut esp_row = Flex::default().row();
    esp_row.set_color(theme::rgb(theme::BG()));
    esp_row.set_spacing(8);
    let mut esp_cb = CheckButton::default().with_label("EFI分区 (ESP)");
    esp_cb.set_checked(true); // 默认勾选（← Python `_set_gpt_default_esp`）
    theme::style_check(&mut esp_cb);
    esp_row.fixed(&esp_cb, 110);
    let mut esp_size_in = Input::default();
    esp_size_in.set_value("300");
    theme::style_input(&mut esp_size_in);
    esp_row.fixed(&esp_size_in, 70);
    let mut esp_unit = Choice::default();
    for u in UNIT_ITEMS {
        esp_unit.add_choice(u);
    }
    esp_unit.set_value(0);
    theme::style_choice(&mut esp_unit);
    esp_row.fixed(&esp_unit, 60);
    let mut esp_info = Frame::default().with_label("FAT32 (系统分区)");
    esp_info.set_align(Align::Left | Align::Inside);
    esp_info.set_label_color(theme::rgb(theme::MUTED()));
    esp_info.set_label_size(11);
    esp_row.end();
    boot_card.fixed(&esp_row, 26);

    boot_card.end();
    root.fixed(&boot_card, 118);

    // ---- 卡片②：数据分区（标题 + 数量 + 分区行列表）----
    let mut data_card = Flex::default().column();
    theme::style_card(&mut data_card);
    data_card.set_spacing(8);
    let data_head = theme::title("数据分区");
    data_card.fixed(&data_head, 20);

    let mut count_row = Flex::default().row();
    count_row.set_color(theme::rgb(theme::BG()));
    count_row.set_spacing(8);
    let count_lbl = theme::form_label("分区数量");
    count_row.fixed(&count_lbl, 80);
    let mut count_spin = Spinner::default();
    count_spin.set_range(1.0, 8.0);
    count_spin.set_step(1.0);
    count_spin.set_value(1.0); // 默认 1 个分区（← Python `part_count_var = tk.IntVar(value=1)`）
    count_row.fixed(&count_spin, 60);
    count_row.end();
    data_card.fixed(&count_row, 26);

    // 分区行容器必须用 Fl_Flex：普通 Fl_Group 的 `resizable_` 指向自身，
    // 不会给子控件分配尺寸，`parts_v` 会一直是 0x0（整块区域空白）。
    // 浅底列表容器 + 白色行 = 清晰的"分区行列表"观感。
    let mut parts_group = Flex::default().column();
    parts_group.set_frame(FrameType::RFlatBox);
    parts_group.set_color(theme::rgb(theme::INPUT_BG()));
    parts_group.set_margin(4);
    parts_group.set_spacing(0);
    parts_group.begin();
    let mut parts_v = Flex::default().column();
    parts_v.set_margin(0);
    parts_v.set_spacing(2);
    parts_v.end();
    parts_group.end();
    // 8 行 × 24 + 7 × 2 间距 = 206，加上下内边距 8 = 214。
    // parts_group 不固定高度：作为 data_card 内唯一弹性子项吸收剩余空间，
    // 8 行放得下且多出的空间留白在列表底部（避免行被拉高或裁切）。
    let mut remain = Frame::default();
    remain.set_align(Align::Left | Align::Inside);
    remain.set_label_color(theme::rgb(theme::MUTED));
    remain.set_label_size(12);
    data_card.fixed(&remain, 22);

    data_card.end();
    // data_card 同样不固定：root 内唯一弹性子项，随窗口缩放拉伸列表区。

    // ---- 底部按钮行：主操作贴右，与卡片之间留出呼吸空间 ----
    let mut btn_row = Flex::default().row();
    btn_row.set_color(theme::rgb(theme::BG));
    btn_row.set_spacing(10);
    // 不固定的占位吸收剩余宽度，把按钮推到对话框右下角。
    let _spacer = Frame::default();
    let mut cancel_btn = theme::secondary_button("取消");
    btn_row.fixed(&cancel_btn, 96);
    let mut ok_btn = theme::primary_button("保存");
    btn_row.fixed(&ok_btn, 110);
    btn_row.end();
    root.fixed(&btn_row, 36);

    root.end();
    win.end();
    // 窗口缩放时由 root 铺满客户区（与主窗口 `App` 的做法一致）。
    win.resizable(&root);

    // ============ 共享状态 ============
    let rows_rc: Rc<RefCell<Vec<PartRow>>> = Rc::new(RefCell::new(Vec::new()));
    let total_cell = Rc::new(Cell::new(total_mb));
    let result_cell: Rc<RefCell<Option<PartitionDialogResult>>> = Rc::new(RefCell::new(None));

    // 前向声明的闭包槽（打破相互递归）
    let update_rc: UpdateFn = Rc::new(RefCell::new(Box::new(|| {})));
    let rebuild_rc: RebuildFn = Rc::new(RefCell::new(Box::new(|_| {})));

    let esp_c = esp_cb.clone();
    let esp_in = esp_size_in.clone();
    let esp_u = esp_unit.clone();
    let gpt = style_gpt.clone();

    // ---- update_remain ----
    {
        let rows = rows_rc.clone();
        let esp_c = esp_c.clone();
        let esp_in = esp_in.clone();
        let esp_u = esp_u.clone();
        let gpt = gpt.clone();
        let mut remain = remain.clone();
        let total = total_cell.clone();
        *update_rc.borrow_mut() = Box::new(move || {
            let is_gpt = gpt.value();
            let esp_on = esp_c.is_checked() && is_gpt;
            let esp_mb = if esp_on {
                parse_size(&esp_in.value(), &esp_u.choice().unwrap_or_default()).unwrap_or(0)
            } else {
                0
            };
            let used = esp_mb + if is_gpt { 16 } else { 0 };

            let (calcs, _manual, remain_mb) = calc_rows(&rows.borrow(), total.get(), used);
            let overflow = remain_mb < 0;

            let auto_idx: Vec<usize> = calcs
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_auto)
                .map(|(i, _)| i)
                .collect();
            let auto_count = auto_idx.len();

            let mut shares: Vec<i64> = vec![0; calcs.len()];
            if !overflow && auto_count > 0 {
                let base = remain_mb / auto_count as i64;
                let extra = remain_mb % auto_count as i64;
                for (k, &i) in auto_idx.iter().enumerate() {
                    shares[i] = base + if k == auto_count - 1 { extra } else { 0 };
                }
            }

            {
                let mut rows = rows.borrow_mut();
                for (i, row) in rows.iter_mut().enumerate() {
                    if calcs[i].invalid {
                        row.hint.set_label_color(rgb(RED.0, RED.1, RED.2));
                        row.hint.set_label("大小无效");
                    } else if calcs[i].is_auto {
                        if overflow || shares[i] <= 0 {
                            row.hint.set_label_color(rgb(RED.0, RED.1, RED.2));
                            row.hint.set_label("空间不足！");
                        } else {
                            row.hint.set_label_color(theme::rgb(theme::MUTED));
                            row.hint.set_label(&format!("自动均分 {}", fmt_size(shares[i])));
                        }
                    } else {
                        row.hint.set_label_color(theme::rgb(theme::MUTED));
                        row.hint.set_label("");
                    }
                }
            }

            // 全手动仍有剩余：并入最后分区并回写
            if !overflow && auto_count == 0 && remain_mb > 0 {
                if let Some(last) = rows.borrow_mut().last_mut() {
                    if let Some(base) = calcs.last().and_then(|c| c.size_mb) {
                        let total_last = base + remain_mb;
                        if total_last >= 1024 {
                            last.unit_choice.set_value(1); // GB
                            last.size_in.set_value(&format!("{:.2}", total_last as f64 / 1024.0));
                        } else {
                            last.unit_choice.set_value(0); // MB
                            last.size_in.set_value(&total_last.to_string());
                        }
                        last.hint.set_label_color(theme::rgb(theme::MUTED));
                        last.hint.set_label("包含剩余空间");
                    }
                }
            }

            if overflow {
                remain.set_label_color(rgb(RED.0, RED.1, RED.2));
                remain.set_label(&format!(
                    "空间不足! 已超出 {}MB ({:.2}GB)",
                    -remain_mb,
                    -remain_mb as f64 / 1024.0
                ));
            } else if auto_count > 0 {
                remain.set_label_color(theme::rgb(theme::TEXT));
                remain.set_label("剩余空间已均分到未设置的分区 (剩余 0MB)");
            } else {
                remain.set_label_color(theme::rgb(theme::TEXT));
                remain.set_label("空间已全部分配 (剩余 0MB)");
            }
        });
    }

    // ---- rebuild_part_rows ----
    {
        let rows = rows_rc.clone();
        let mut container = parts_v.clone();
        let update = update_rc.clone();
        *rebuild_rc.borrow_mut() = Box::new(move |count: i32| {
            let count = count.clamp(1, 8);
            container.clear();
            // FLTK 的分组模型不是栈，而是一个全局单指针 `Fl_Group::current_`：
            //   - `Fl_Group::begin()` → `current_ = this`
            //   - `Fl_Group::end()`   → `current_ = current_->parent()`（当前组的父组）
            //   - `Fl_Group` 构造函数会自动 `begin()`
            //   - `Fl_Widget` 构造函数在 `current_` 非空时把自己 `add()` 进 `current_`
            // 本闭包在对话框构建（`win.end()`）之后才执行，此时 `current_` 为 null，
            // 若不显式 begin()，新建的行控件就成了「孤儿」——不在任何控件树里，
            // 既不显示也不参与布局（表现为对话框「数据分区」区域整块空白），
            // 且每次改分区数量都会泄漏一批控件。
            // 因此这里显式把 `parts_v` 设为当前组，建完再恢复进入闭包前的状态。
            // 注意：`Group::current()` 在 current 为 null 时会 panic，保存当前组
            // 必须用 `Group::try_current()`。
            let saved_current = Group::try_current();
            container.begin();
            let mut rows_mut = rows.borrow_mut();
            rows_mut.clear();
            for i in 0..count {
                let mut row_flex = Flex::default().row();
                row_flex.set_color(theme::rgb(theme::CARD));
                let label = theme::form_label(&format!("分区{}", i + 1));
                row_flex.fixed(&label, 60);

                let mut size_in = Input::default();
                theme::style_input(&mut size_in);
                {
                    let u = update.clone();
                    size_in.set_callback(move |_| (u.borrow_mut())());
                }
                row_flex.fixed(&size_in, 70);

                let mut unit_choice = Choice::default();
                for u in UNIT_ITEMS {
                    unit_choice.add_choice(u);
                }
                unit_choice.set_value(1);
                theme::style_choice(&mut unit_choice);
                {
                    let u = update.clone();
                    unit_choice.set_callback(move |_| (u.borrow_mut())());
                }
                row_flex.fixed(&unit_choice, 60);

                let mut fs_choice = Choice::default();
                for f in FS_ITEMS {
                    fs_choice.add_choice(f);
                }
                fs_choice.set_value(0);
                theme::style_choice(&mut fs_choice);
                row_flex.fixed(&fs_choice, 90);

                let mut hint = Frame::default();
                hint.set_align(Align::Left | Align::Inside);
                hint.set_label_color(theme::rgb(theme::MUTED));
                row_flex.fixed(&hint, 160);

                row_flex.end();
                // 行高固定为 26px（← Python 原版每行约 26px，最多 8 行），
                // 否则 Fl_Flex 会把剩余高度平均分给每一行，输入框会被拉得过高。
                container.fixed(&row_flex, 26);
                rows_mut.push(PartRow {
                    size_in,
                    unit_choice,
                    fs_choice,
                    hint,
                });
            }
            drop(rows_mut);
            container.end();
            // 恢复进入闭包前的 `current_`（对话框存续期间应为 null）。
            Group::set_current(saved_current.as_ref());
            // 重新计算 `parts_v` 的子控件位置：新增的行控件默认位于 (0,0)，
            // 不触发一次 layout 的话所有行会叠在一起（只有首次显示前的那次
            // `root.layout()` 才会顺带摆好）。
            container.layout();
            (update.borrow_mut())();
        });
    }

    // ---- 分区数量变化 ----
    {
        let rb = rebuild_rc.clone();
        count_spin.set_callback(move |s| {
            let n = s.value().round() as i32;
            s.set_value(n.clamp(1, 8) as f64);
            (rb.borrow_mut())(n);
        });
    }

    // ---- ESP 大小 / 单位变化 ----
    {
        let u = update_rc.clone();
        esp_size_in.set_callback(move |_| (u.borrow_mut())());
    }
    {
        let u = update_rc.clone();
        esp_unit.set_callback(move |_| (u.borrow_mut())());
    }

    // ---- 分区表格式切换（MBR 取消并禁用 ESP）----
    {
        let u = update_rc.clone();
        let mut esp_cb2 = esp_cb.clone();
        let mut esp_in2 = esp_size_in.clone();
        let mut esp_u2 = esp_unit.clone();
        let mut gpt_v = style_gpt.clone();
        let mut mbr_v = style_mbr.clone();
        let apply: Box<dyn FnMut(bool)> = Box::new(move |is_gpt: bool| {
            // 同步两段的选中视觉（radio 互斥时 FLTK 不会触发另一端的回调）
            apply_segment_visual(&mut gpt_v, false);
            apply_segment_visual(&mut mbr_v, false);
            if !is_gpt {
                esp_cb2.set_checked(false);
                esp_cb2.deactivate();
                esp_in2.deactivate();
                esp_u2.deactivate();
            } else {
                esp_cb2.activate();
                if esp_cb2.is_checked() {
                    esp_in2.activate();
                    esp_u2.activate();
                } else {
                    esp_in2.deactivate();
                    esp_u2.deactivate();
                }
            }
            (u.borrow_mut())();
        });
        let apply_rc = Rc::new(RefCell::new(apply));
        {
            let a = apply_rc.clone();
            style_gpt.set_callback(move |_| (a.borrow_mut())(true));
        }
        {
            let a = apply_rc.clone();
            style_mbr.set_callback(move |_| (a.borrow_mut())(false));
        }
    }

    // ---- ESP 勾选变化：同步输入框可用性 ----
    {
        let u = update_rc.clone();
        let mut esp_in2 = esp_size_in.clone();
        let mut esp_u2 = esp_unit.clone();
        let gpt2 = gpt.clone();
        esp_cb.set_callback(move |c| {
            if c.is_checked() && gpt2.value() {
                esp_in2.activate();
                esp_u2.activate();
            } else {
                esp_in2.deactivate();
                esp_u2.deactivate();
            }
            (u.borrow_mut())();
        });
    }

    // 初始：MBR 分支不会触发，但 ESP 默认未勾选 -> 输入框禁用
    esp_size_in.deactivate();
    esp_unit.deactivate();

    // ============ 取消 ============
    {
        let mut win_c = win.clone();
        cancel_btn.set_callback(move |_| win_c.hide());
    }

    // ============ 保存 ============
    {
        let rows = rows_rc.clone();
        let esp_c = esp_c.clone();
        let esp_in = esp_in.clone();
        let esp_u = esp_u.clone();
        let gpt = gpt.clone();
        let total = total_cell.clone();
        let result = result_cell.clone();
        let mut win_c = win.clone();
        let update = update_rc.clone();
        let disk_idx = data.disk_idx;
        ok_btn.set_callback(move |_| {
            (update.borrow_mut())();
            let is_gpt = gpt.value();
            let style = if is_gpt { "gpt" } else { "mbr" };

            let esp_enabled = esp_c.is_checked() && is_gpt;
            let mut esp_mb = 0i64;
            if esp_enabled {
                match parse_size(&esp_in.value(), &esp_u.choice().unwrap_or_default()) {
                    Some(v) if v > 0 => esp_mb = v,
                    _ => {
                        msg::alert("ESP大小无效，请重新输入");
                        return;
                    }
                }
            }

            let used = esp_mb + if is_gpt { 16 } else { 0 };
            let rows_snapshot: Vec<PartRow> = rows.borrow().clone();
            let n = rows_snapshot.len();
            let (calcs, manual, remain) = calc_rows(&rows_snapshot, total.get(), used);

            // 逐行校验
            for (i, c) in calcs.iter().enumerate() {
                if c.invalid {
                    msg::alert(&format!("分区{}的大小无效，请重新输入", i + 1));
                    return;
                }
            }

            // 计算自动行份额
            let auto_idx: Vec<usize> = calcs
                .iter()
                .enumerate()
                .filter(|(_, c)| c.is_auto)
                .map(|(i, _)| i)
                .collect();
            let mut auto_sizes: Vec<i64> = vec![0; n];
            if remain >= 0 && !auto_idx.is_empty() {
                let base = remain / auto_idx.len() as i64;
                let extra = remain % auto_idx.len() as i64;
                for (k, &i) in auto_idx.iter().enumerate() {
                    auto_sizes[i] = base + if k == auto_idx.len() - 1 { extra } else { 0 };
                }
            }

            // 每行 size_mb 判定
            let mut final_sizes: Vec<Option<i64>> = vec![None; n];
            for i in 0..n {
                if calcs[i].invalid {
                    msg::alert(&format!("分区{}的大小无效，请重新输入", i + 1));
                    return;
                }
                if calcs[i].is_auto {
                    if remain < 0 || auto_sizes[i] <= 0 {
                        msg::alert(&format!(
                            "分区{}的空间不足，请调整分区大小后重试",
                            i + 1
                        ));
                        return;
                    }
                    final_sizes[i] = Some(auto_sizes[i]);
                } else {
                    match calcs[i].size_mb {
                        Some(mb) if mb > 0 => final_sizes[i] = Some(mb),
                        _ => {
                            msg::alert(&format!(
                                "分区{}的大小不能为空",
                                i + 1
                            ));
                            return;
                        }
                    }
                }
            }

            // 总空间校验
            let mut total_used = used;
            for s in final_sizes.iter().flatten() {
                total_used += *s;
            }
            if total.get() - total_used < 0 {
                msg::alert(&format!(
                    "分区总大小 ({total_used}MB) 超出磁盘容量 ({}MB)\n超出 {}MB，请调整分区大小后重试",
                    total.get(),
                    total_used - total.get()
                ));
                return;
            }

            // 组装结果：最后一个自动行 -> None
            let mut data_parts: Vec<DataPart> = Vec::with_capacity(n);
            for i in 0..n {
                let fs = rows_snapshot[i].fs_choice.choice().unwrap_or_default();
                let is_last_auto = i == n - 1 && calcs[i].is_auto;
                if is_last_auto {
                    data_parts.push(DataPart {
                        size_mb: None,
                        fs,
                    });
                } else {
                    data_parts.push(DataPart {
                        size_mb: final_sizes[i],
                        fs,
                    });
                }
            }

            log::info!(
                "分区方案确认: 磁盘{disk_idx}, {}, ESP={esp_mb}MB, 分区数={n}, 已用={total_used}MB, 剩余={}MB",
                style.to_uppercase(),
                total.get() - total_used
            );
            let _ = manual;
            *result.borrow_mut() = Some(PartitionDialogResult {
                style: style.to_string(),
                esp_enabled,
                esp_size_mb: esp_mb,
                data_parts,
            });
            win_c.hide();
        });
    }

    // 初始重建 1 行（默认分区数量 = 1）
    (rebuild_rc.borrow_mut())(1);

    win.show();
    // 显示后重新计算一次布局：此时窗口客户区尺寸才最终确定，
    // 需要把 root → parts_group → parts_v → 各行的尺寸一路传下去。
    root.layout();
    while win.shown() {
        if !fltk::app::wait() {
            break;
        }
    }

    let out = result_cell.borrow().clone();
    out
}
