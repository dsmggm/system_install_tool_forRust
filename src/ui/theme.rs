//! 统一视觉主题：配色、字体与常用控件样式。
//!
//! 目标是把界面从「FLTK 默认灰色控件堆叠」调整成更现代的卡片式布局：
//! 浅灰背景 + 白色圆角卡片 + 单一主色（蓝）强调操作按钮。
//!
//! 说明：FLTK 没有样式表，所有外观都靠 `box/color/font/size` 逐控件设置，
//! 因此这里集中成少量构造函数，避免各处散落的魔法颜色。

#![allow(dead_code)]

use fltk::app;
use fltk::button::Button;
use fltk::enums::{Align, Color, Font, FrameType};
use fltk::frame::Frame;
use fltk::group::Flex;
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::misc::Progress;
use fltk::prelude::*;

/// RGB 三元组（便于复用与派生）
pub type Rgb = (u8, u8, u8);

// ---------- 配色 ----------
/// 窗口底色（浅灰蓝）
pub const BG: Rgb = (0xF3, 0xF5, 0xFA);
/// 卡片底色
pub const CARD: Rgb = (0xFF, 0xFF, 0xFF);
/// 输入框/次级按钮底色
pub const INPUT_BG: Rgb = (0xE6, 0xEC, 0xF7);
/// 卡片分隔线/边框
pub const BORDER: Rgb = (0xE3, 0xE8, 0xF1);
/// 主文字色
pub const TEXT: Rgb = (0x1F, 0x29, 0x37);
/// 次要文字色
pub const MUTED: Rgb = (0x6B, 0x72, 0x80);
/// 主色（操作蓝）
pub const PRIMARY: Rgb = (0x2F, 0x6F, 0xED);
/// 主色按下态
pub const PRIMARY_DARK: Rgb = (0x15, 0x3D, 0xB4);
/// 主色悬停态（比 PRIMARY 明显变亮，一眼能看出）
pub const PRIMARY_HOVER: Rgb = (0x5B, 0x97, 0xF7);
/// 浅底按钮悬停底色（比 INPUT_BG 清晰可辨的浅蓝）
pub const HOVER_SOFT: Rgb = (0xC9, 0xDB, 0xFA);
/// 浅底按钮按下底色（比 HOVER_SOFT 更深一档）
pub const PRESS_SOFT: Rgb = (0xAC, 0xC8, 0xF6);
/// 主色浅底（次级按钮/选中行）
pub const PRIMARY_SOFT: Rgb = (0xE8, 0xF0, 0xFE);
/// 成功色
pub const SUCCESS: Rgb = (0x15, 0x9C, 0x4A);
/// 警告色
pub const WARN: Rgb = (0xD9, 0x5B, 0x0B);
/// 错误/倒计时色
pub const DANGER: Rgb = (0xD3, 0x2F, 0x2F);
/// 页头底色
pub const HEADER_BG: Rgb = (0xFF, 0xFF, 0xFF);
/// 树选中行底色（浅蓝，保证黑字可读）
pub const SELECTION: Rgb = (0xDB, 0xE7, 0xFF);

pub fn rgb((r, g, b): Rgb) -> Color {
    Color::from_rgba_tuple((r, g, b, 255))
}

/// 全局字体与基础尺寸（必须在创建任何控件之前调用）。
pub fn init() {
    // 中文界面优先用系统自带雅黑；缺失时 fltk 会回落到 Helvetica。
    app::set_font(Font::by_name("Microsoft YaHei UI"));
    app::set_font_size(12);
    app::set_background_color(BG.0, BG.1, BG.2);
}

/// 把 Flex 变成白色圆角卡片。
pub fn style_card(f: &mut Flex) {
    f.set_frame(FrameType::RFlatBox);
    f.set_color(rgb(CARD));
    f.set_margins(10, 10, 10, 10);
    f.set_spacing(5);
}

/// 透明背景的普通容器（用于卡片列等不需要自绘背景的地方）。
pub fn style_plain(f: &mut Flex) {
    f.set_frame(FrameType::NoBox);
    f.set_color(rgb(BG));
    f.set_margins(0, 0, 0, 0);
}

/// 卡片标题（小号深色粗体，左侧留白）。
pub fn title(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_font(Font::HelveticaBold);
    f.set_label_size(12);
    f.set_label_color(rgb(TEXT));
    f
}

/// 表单标签（右对齐不必要，左对齐 + 次要色）。
pub fn form_label(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_color(rgb(MUTED));
    f.set_label_size(12);
    f
}

/// 普通文本标签。
pub fn label(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_color(rgb(TEXT));
    f.set_label_size(12);
    f
}

/// 主操作按钮（实心蓝底白字；悬停明显变亮、按下明显变深）。
pub fn primary_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(PRIMARY));
    b.set_selection_color(rgb(PRIMARY_DARK)); // 按下态底色
    b.set_label_color(Color::White);
    b.set_label_font(Font::HelveticaBold);
    b.set_label_size(13);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, PRIMARY_HOVER);
    b
}

/// 次级按钮（浅底、主色文字；悬停变浅蓝、按下更深一档）。
pub fn secondary_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(INPUT_BG));
    b.set_selection_color(rgb(PRESS_SOFT));
    b.set_label_color(rgb(PRIMARY));
    b.set_label_size(12);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, HOVER_SOFT);
    b
}

/// 文字按钮（圆角小药丸：浅底 + 主色文字，页头次要操作）。
pub fn ghost_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(INPUT_BG));
    b.set_selection_color(rgb(PRESS_SOFT));
    b.set_label_color(rgb(PRIMARY));
    b.set_label_size(12);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, HOVER_SOFT);
    b
}

/// 树行内按钮（紧凑、浅底、悬停明显变蓝）。
/// 适合在 Tree 的 LabelAndWidget 模式下做行内操作按钮。
pub fn tree_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(INPUT_BG));              // 默认浅底
    b.set_selection_color(rgb(PRESS_SOFT));  // 按下时深一档浅蓝
    b.set_label_color(rgb(PRIMARY));         // 主色文字
    b.set_label_font(Font::HelveticaBold);
    b.set_label_size(11);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, HOVER_SOFT);
    b
}

/// 给按钮添加悬停效果（进入/离开时切换颜色）。
/// 需要在按钮创建后立即调用，`hover_color` 为鼠标进入时的底色。
pub fn add_hover_effect<B: fltk::prelude::WidgetBase>(btn: &mut B, hover_color: Rgb) {
    let default_color = btn.color();
    btn.handle(move |w, ev| {
        match ev {
            fltk::enums::Event::Enter => {
                w.set_color(rgb(hover_color));
                w.redraw();
                true
            }
            fltk::enums::Event::Leave => {
                w.set_color(default_color);
                w.redraw();
                true
            }
            _ => false,
        }
    });
}

/// 分段选择容器（浅底圆角条，GPT/MBR 等互斥选项放进里面形成"分段"外观）。
pub fn style_segmented(f: &mut Flex) {
    f.set_frame(FrameType::RFlatBox);
    f.set_color(rgb(INPUT_BG));
    f.set_margins(3, 3, 3, 3);
    f.set_spacing(3);
}

/// 分段条内的选项按钮（NoBox，透明文字色）。
pub fn segmented_item(w: &mut impl ButtonExt) {
    w.set_frame(FrameType::NoBox);
    w.set_down_frame(FrameType::NoBox);
    w.set_color(Color::TransparentBg);
    w.set_selection_color(Color::TransparentBg);
    w.set_label_color(rgb(TEXT));
    w.set_label_size(12);
}

/// 输入框样式（浅底圆角填充）。
pub fn style_input(i: &mut Input) {
    i.set_frame(FrameType::RFlatBox);
    i.set_color(rgb(INPUT_BG));
    i.set_text_color(rgb(TEXT));
    i.set_text_size(12);
}

/// 下拉框样式。
pub fn style_choice(c: &mut Choice) {
    c.set_frame(FrameType::RFlatBox);
    c.set_color(rgb(INPUT_BG));
    c.set_text_color(rgb(TEXT));
    c.set_text_size(12);
}

/// 复选框样式（仅统一文字颜色/字号）。
pub fn style_check(c: &mut impl WidgetExt) {
    c.set_label_color(rgb(TEXT));
    c.set_label_size(12);
}

/// 进度条样式。
pub fn style_progress(p: &mut Progress) {
    p.set_frame(FrameType::RFlatBox);
    p.set_color(rgb(INPUT_BG));
    p.set_selection_color(rgb(PRIMARY));
}
