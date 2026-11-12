//! 统一提示/确认对话框（居中弹出，替代 FLTK 原生 alert/message/choice2）。
//!
//! 【为什么不用 fltk::dialog::alert_default 等】它们默认是"跟随鼠标"
//! （Fl_Message::enable_hotspot_ = 1）定位，弹出位置不居中；fltk-sys 又
//! 没有暴露自定义坐标的接口（`Fl_message_position` 未绑定）。
//! 而文件浏览框之外的提示框样式与新版主题也不一致，因此实现一套模态
//! 消息窗：以屏幕可用区为中心弹出、按钮/配色与主题统一。
//!
//! 提供：
//! - `alert(text)` 提示（确定）— 替代 `dialog::alert_default`
//! - `info(text)`  信息（确定）— 替代 `dialog::message_default`
//! - `confirm(text) -> bool` 确认（确定/取消）— 替代 `choice2("确定","取消")`
//! - `ask(text) -> bool`     询问（是/否）— 替代 `choice2("是","否")`
//!
//! 返回语义与原生一致：confirm/ask 关闭窗口或按 Esc 一律视为"否/取消"。

use std::cell::RefCell;
use std::rc::Rc;

use fltk::app;
use fltk::enums::{Align, Event, Key};
use fltk::frame::Frame;
use fltk::group::{Flex, Group};
use fltk::prelude::*;
use fltk::window::DoubleWindow;

use super::{app_icon, theme};

/// 全角字符近似宽度（12px 字体下，中文/全角标点 ≈ 13px）。
fn char_px(c: char) -> i32 {
    let u = c as u32;
    if u >= 0x2E80 || (0x1100..=0x115F).contains(&u) || (0xFF00..=0xFF64).contains(&u) {
        13
    } else {
        7
    }
}

/// 把文本按像素宽度做软换行，返回 (行列表, 最宽行的像素宽)。
///
/// FLTK 的 Frame 不自动换行，镜像路径等长文本必须拆行，否则窗口右侧被
/// 推到屏幕外。已有换行符按原样保留。
fn wrap_text(text: &str, max_px: i32) -> (Vec<String>, i32) {
    let mut lines: Vec<String> = Vec::new();
    let mut max_w = 0i32;
    for raw_line in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0i32;
        let mut last_space: Option<usize> = None;
        let mut width_at_last_space = 0i32;
        for c in raw_line.chars() {
            let cw = char_px(c);
            if cur_w + cw > max_px && !cur.is_empty() {
                // 先尝试在最近的空格处断行（路径/参数更易读），没有就硬断。
                if let Some(sp) = last_space {
                    let head: String = cur.chars().take(sp).collect();
                    let tail: String = cur.chars().skip(sp + 1).collect();
                    lines.push(head);
                    max_w = max_w.max(width_at_last_space);
                    cur = tail;
                    cur_w = char_px_iter_px(&cur);
                } else {
                    lines.push(cur.clone());
                    max_w = max_w.max(cur_w);
                    cur.clear();
                    cur_w = 0;
                }
                last_space = None;
            }
            if c == ' ' {
                last_space = Some(cur.chars().count());
                width_at_last_space = cur_w;
            }
            cur.push(c);
            cur_w += cw;
        }
        lines.push(cur);
        max_w = max_w.max(cur_w);
    }
    (lines, max_w)
}

/// 重新计算字符串像素宽（软断行后回填用）。
fn char_px_iter_px(s: &str) -> i32 {
    s.chars().map(char_px).sum()
}

/// 弹出模态消息窗并阻塞至关闭。
///
/// - `btn0`：主按钮（右），返回 0；
/// - `btn1`：次按钮（左），返回 1；`None` 表示只有主按钮；
/// - 关闭窗口（X）或按 Esc：返回 1（有次按钮时）或 0（仅确认类）。
fn show_modal(title: &str, text: &str, btn0: &str, btn1: Option<&str>) -> i32 {
    // 与 one_click::show 同源陷阱：回调栈里残留的 current group 会把顶层
    // 窗口吞成子窗口，创建前必须清空（见 one_click.rs 注释）。
    Group::set_current(None::<&Group>);

    // 尺寸：按换行后的最宽行估算，夹在 [420, 760]；高度按行数。
    let text_w_target = 620;
    let (lines, natural) = wrap_text(text, text_w_target);
    let inner_w = natural.max({
        // 全是窄 ASCII 时的最小可读宽度
        260
    });
    let w = (inner_w + 76).clamp(420, 760);
    let text_h = lines.len() as i32 * 20;
    let (sw, sh) = app::screen_size();
    let sw = sw as i32;
    let sh = sh as i32;
    let h = (106 + text_h).clamp(150, (sh - 80).max(150));

    let x = ((sw - w) / 2).max(0);
    let y = ((sh - h) / 2).max(0);
    let mut win = DoubleWindow::new(x, y, w, h, title);
    win.set_color(theme::rgb(theme::BG_TOP()));
    win.set_icon(Some(app_icon()));
    win.begin();

    let mut root = Flex::default().column().size_of_parent();
    root.set_margin(16);
    root.set_spacing(12);
    root.set_color(theme::rgb(theme::BG()));

    // 文本卡片：圆角 CARD 底 + 文字左对齐
    let mut card = Flex::default().column();
    theme::style_card(&mut card);
    let mut label = Frame::default();
    label.set_align(Align::Left | Align::Inside);
    label.set_label_size(12);
    label.set_label_color(theme::rgb(theme::TEXT()));
    label.set_label(&lines.join("\n"));
    card.end();
    // 文本卡片占满剩余高度，按钮固定贴底
    let mut btn_row = Flex::default().row();
    btn_row.set_color(theme::rgb(theme::BG()));
    let _spacer = Frame::default();
    let result_rc: Rc<RefCell<i32>> = Rc::new(RefCell::new(-1));
    if let Some(b1) = btn1 {
        let mut cancel_btn = theme::secondary_button(b1);
        btn_row.fixed(&cancel_btn, 96);
        {
            let result = result_rc.clone();
            let mut win_c = win.clone();
            cancel_btn.set_callback(move |_| {
                *result.borrow_mut() = 1;
                win_c.hide();
            });
        }
    }
    let mut ok_btn = theme::primary_button(btn0);
    btn_row.fixed(&ok_btn, 110);
    btn_row.end();
    root.fixed(&btn_row, 34);
    {
        let result = result_rc.clone();
        let mut win_c = win.clone();
        ok_btn.set_callback(move |_| {
            *result.borrow_mut() = 0;
            win_c.hide();
        });
    }

    root.end();
    win.end();
    // 渐变背景 + 标题栏融合（与主窗口一致）
    theme::apply_window_backdrop(&mut win);

    // Esc 关闭 → 不改 result（保持 -1，按"取消/否"处理）。
    {
        let mut win_h = win.clone();
        win.handle(move |_w, ev| {
            if ev == Event::KeyDown && app::event_key() == Key::Escape {
                win_h.hide();
                true
            } else {
                false
            }
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
    let out = *result_rc.borrow();
    out
}

/// 提示（替代 `alert_default`）。
pub fn alert(text: &str) {
    let _ = show_modal("提示", text, "确定", None);
}

/// 信息（替代 `message_default`）。
pub fn info(text: &str) {
    let _ = show_modal("信息", text, "确定", None);
}

/// 确认（替代 `choice2_default(msg, "确定", "取消", "")`）。
/// 返回 `true` 仅当用户点了「确定」。
pub fn confirm(text: &str) -> bool {
    show_modal("请确认", text, "确定", Some("取消")) == 0
}

/// 询问（替代 `choice2_default(msg, "是", "否", "")`）。
/// 返回 `true` 仅当用户点了「是」。
pub fn ask(text: &str) -> bool {
    show_modal("请确认", text, "是", Some("否")) == 0
}
