//! Footer layout builder (progress bar, scheme label, buttons)

use fltk::button::Button;
use fltk::enums::Align;
use fltk::frame::Frame;
use fltk::group::Flex;
use fltk::misc::Progress;
use fltk::prelude::*;

use super::super::theme;

/// Widget containers for the footer section
pub struct FooterUi {
    pub footer: Flex,
    pub progress: Progress,
    pub scheme_head: Frame,
    pub current_scheme_label: Frame,
    pub save_scheme_btn: Button,
    pub load_scheme_btn: Button,
    pub install_btn: Button,
}

pub fn create_footer() -> FooterUi {
    // ===== 页脚：进度条 + 当前方案 + 方案操作 + 开始安装 =====
    // 高度 62 = 上下内边距 18 + 进度条 8 + 间距 6 + 按钮行 30
    let mut footer = Flex::default().column();
    footer.set_frame(fltk::enums::FrameType::FlatBox);
    footer.set_color(theme::rgb(theme::CARD));
    footer.set_margins(16, 8, 16, 10);
    footer.set_spacing(6);

    let mut progress = Progress::default();
    theme::style_progress(&mut progress);
    progress.set_minimum(0.0);
    progress.set_maximum(100.0);
    progress.set_value(0.0);
    footer.fixed(&progress, 8);

    let mut foot_row = Flex::default().row();
    foot_row.set_color(theme::rgb(theme::CARD));
    let mut scheme_head = Frame::default().with_label("当前方案");
    scheme_head.set_align(Align::Left | Align::Inside);
    scheme_head.set_label_size(11);
    scheme_head.set_label_color(theme::rgb(theme::MUTED));
    foot_row.fixed(&scheme_head, 62);
    let mut current_scheme_label = theme::label("无");
    current_scheme_label.set_label_color(theme::rgb(theme::PRIMARY));
    let save_scheme_btn = theme::secondary_button("保存当前方案");
    foot_row.fixed(&save_scheme_btn, 116);
    let load_scheme_btn = theme::secondary_button("加载方案");
    foot_row.fixed(&load_scheme_btn, 92);
    let mut install_btn = theme::primary_button("开始安装");
    install_btn.deactivate();
    foot_row.fixed(&install_btn, 140);
    foot_row.end();
    footer.fixed(&foot_row, 30);
    footer.end();

    FooterUi {
        footer,
        progress,
        scheme_head,
        current_scheme_label,
        save_scheme_btn,
        load_scheme_btn,
        install_btn,
    }
}