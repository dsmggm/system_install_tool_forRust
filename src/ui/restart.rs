//! 安装完成后倒计时重启对话框（← `gui.py::RestartDialog`，文档 §7）
//!
//! 显示 30 秒倒计时；`立刻重启` 立即执行 `shutdown /r /t 0`；`取消` 关闭
//! 不重启；倒计时结束自动重启。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fltk::app;
use fltk::enums::{Align, Font, FrameType};
use fltk::frame::Frame;
use fltk::group::{Flex, Group};
use fltk::prelude::*;
use fltk::window::DoubleWindow;

use std::os::windows::process::CommandExt;

use super::{app_icon, theme};

/// 默认重启倒计时（config.ini `restart_countdown` 缺失/无效时的兜底值）
const DEFAULT_COUNTDOWN: i64 = 30;

/// 弹出对话框并阻塞至关闭；`countdown_sec` 为倒计时秒数（<1 时回落默认值）。
pub fn show(parent: &mut DoubleWindow, countdown_sec: i64) {
    // 防御：load() 已校验过，这里再兜底一次
    let total = if countdown_sec >= 1 { countdown_sec } else { DEFAULT_COUNTDOWN };
    // 同 one_click::show：回调期残留的 current group 会把本窗口吞成子窗口，
    // 创建顶层窗口前必须清掉（见 one_click.rs 的【陷阱】注释）。
    Group::set_current(None::<&Group>);
    let (dx, dy, dw, dh) = super::dialog_geometry(parent, 380, 200, 340, 180);
    let mut win = DoubleWindow::new(dx, dy, dw, dh, "系统安装完成");
    win.size_range(dw.min(340), dh.min(180), 0, 0);
    win.set_color(theme::rgb(theme::BG_TOP()));
    win.set_icon(Some(app_icon()));
    win.begin();

    let mut root = Flex::default().column().size_of_parent();
    root.set_margin(20);
    root.set_spacing(12);
    root.set_color(theme::rgb(theme::BG()));

    // 大卡片：成功标题（粗体）+ 倒计时副文案（次要色），随窗口剩余空间拉伸
    let mut card = Flex::default().column();
    card.set_frame(FrameType::RFlatBox);
    card.set_color(theme::rgb(theme::GLASS_CARD()));
    card.set_margins(20, 14, 20, 14);
    card.set_spacing(6);
    let mut title_lbl = Frame::default().with_label("系统安装完成");
    title_lbl.set_align(Align::Center | Align::Inside);
    title_lbl.set_label_font(Font::HelveticaBold);
    title_lbl.set_label_size(15);
    title_lbl.set_label_color(theme::rgb(theme::TEXT()));
    card.fixed(&title_lbl, 24);
    let mut sub_lbl = Frame::default();
    sub_lbl.set_align(Align::Center | Align::Inside);
    sub_lbl.set_label_size(12);
    sub_lbl.set_label_color(theme::rgb(theme::MUTED()));
    sub_lbl.set_label(&format!("将在 {total} 秒后自动重启"));
    card.end();
    // 不固定高度：卡片吸收窗口剩余空间，按钮始终贴底。
    let mut btn_row = Flex::default().row();
    btn_row.set_color(theme::rgb(theme::BG()));
    // 两个按钮平分行宽（不设固定宽度），整行在对话框内左右对称。
    let mut restart_btn = theme::primary_button("立刻重启");
    let mut cancel_btn = theme::secondary_button("取消");
    btn_row.end();
    root.fixed(&btn_row, 34);

    root.end();
    win.end();
    // 顶层 Flex 铺满窗口客户区（size_of_parent 已按窗口尺寸设置一次），
    // resizable 让窗口缩放时同步调整——与 one_click.rs / 主窗口一致。
    win.resizable(&root);
    // 渐变背景 + 标题栏融合（与主窗口一致）
    theme::apply_window_backdrop(&mut win);

    let remaining = Rc::new(Cell::new(total));
    let handle_cell: Rc<Cell<Option<app::TimeoutHandle>>> = Rc::new(Cell::new(None));
    let did_restart = Rc::new(Cell::new(false));

    // “立刻重启”例程（供定时器与按钮复用）。
    let do_restart: Rc<RefCell<Box<dyn FnMut()>>> = {
        let mut sub = sub_lbl.clone();
        let did_restart = did_restart.clone();
        let handle_cell = handle_cell.clone();
        let mut win_c = win.clone();
        Rc::new(RefCell::new(Box::new(move || {
            if did_restart.get() {
                return;
            }
            did_restart.set(true);
            if let Some(h) = handle_cell.take() {
                app::remove_timeout3(h);
            }
            sub.set_label("正在重启，请稍候...");
            if let Err(e) = std::process::Command::new("shutdown")
                .args(["/r", "/t", "0"])
                .creation_flags(crate::utils::CREATE_NO_WINDOW)
                .spawn()
            {
                log::error!("重启命令执行失败: {e}");
            }
            win_c.hide();
        })))
    };

    // 倒计时 tick
    {
        let mut sub = sub_lbl.clone();
        let remaining = remaining.clone();
        let do_restart2 = do_restart.clone();
        let h = app::add_timeout3(1.0, move |handle| {
            let n = remaining.get() - 1;
            remaining.set(n);
            if n <= 0 {
                (do_restart2.borrow_mut())();
                return;
            }
            sub.set_label(&format!("将在 {n} 秒后自动重启"));
            app::repeat_timeout3(1.0, handle);
        });
        handle_cell.set(Some(h));
    }

    // 按钮回调
    {
        let do_restart = do_restart.clone();
        restart_btn.set_callback(move |_| (do_restart.borrow_mut())());
    }
    {
        let handle_cell = handle_cell.clone();
        let mut win_c = win.clone();
        cancel_btn.set_callback(move |_| {
            if let Some(h) = handle_cell.take() {
                app::remove_timeout3(h);
            }
            win_c.hide();
        });
    }

    win.show();
    // 标题栏与背景融为一体（DWM）
    theme::apply_dwm_titlebar(&win);
    while win.shown() {
        if !app::wait() {
            break;
        }
    }
    // 收尾：确保定时器被移除
    if let Some(h) = handle_cell.take() {
        app::remove_timeout3(h);
    }
}
