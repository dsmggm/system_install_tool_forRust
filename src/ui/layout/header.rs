//! Toolbar layout builder (restore toggle, countdown label, log toggle)

use fltk::button::{Button, CheckButton};
use fltk::enums::Align;
use fltk::frame::Frame;
use fltk::group::Flex;
use fltk::prelude::*;

use super::super::theme;

/// Widget containers for the header (toolbar) section
pub struct HeaderUi {
    pub header: Flex,
    pub restore_cb: CheckButton,
    pub countdown_label: Frame,
    pub log_toggle_btn: Button,
}

pub fn create_header(config: &crate::config::settings::AppConfig) -> HeaderUi {
    // ===== 页头：自动还原开关（左）/ 倒计时 / 日志开关 =====
    // 高度 48 = 上下内边距 18 + 行 26 + 下 4
    let mut header = Flex::default().column();
    header.set_frame(fltk::enums::FrameType::FlatBox);
    header.set_color(theme::rgb(theme::HEADER_BG));
    header.set_margins(16, 8, 16, 10);
    header.set_spacing(0);

    let mut head_row = Flex::default().row();
    head_row.set_color(theme::rgb(theme::HEADER_BG));
    // 自动还原开关：勾选后显示运行态文案（绿），取消后显示停止态文案（红）。
    let mut restore_cb = CheckButton::default().with_label("");
    restore_cb.set_checked(config.restore_enabled);
    theme::style_check(&mut restore_cb);
    restore_cb.set_label_color(theme::rgb(theme::DANGER));
    head_row.fixed(&restore_cb, 180);
    let _head_spacer = Frame::default();
    // 倒计时标签：右对齐，与日志按钮同行
    let mut countdown_label = theme::label("");
    countdown_label.set_align(Align::Right | Align::Inside);
    countdown_label.set_label_color(theme::rgb(theme::DANGER));
    head_row.fixed(&countdown_label, 210);
    let log_toggle_btn = theme::ghost_button("显示日志");
    head_row.fixed(&log_toggle_btn, 92);
    head_row.end();

    header.fixed(&head_row, 26);
    header.end();

    HeaderUi {
        header,
        restore_cb,
        countdown_label,
        log_toggle_btn,
    }
}