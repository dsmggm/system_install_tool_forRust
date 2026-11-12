//! 统一视觉主题：配色、字体与常用控件样式。
//!
//! 支持浅色/深色主题，自动检测系统主题，字体颜色随主题自动切换。
//!
//! 说明：FLTK 没有样式表，所有外观都靠 `box/color/font/size` 逐控件设置，
//! 因此这里集中成少量构造函数，避免各处散落的魔法颜色。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, Ordering};

use fltk::app;
use fltk::button::Button;
use fltk::enums::{Align, Color, Font, FrameType};
use fltk::frame::Frame;
use fltk::group::{Flex, Group};
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::misc::Progress;
use fltk::prelude::*;
use fltk::tree::Tree;

use crate::ui::theme_mode::{self, ThemeMode};

/// RGB 三元组（便于复用与派生）
pub type Rgb = (u8, u8, u8);

// ==================== 主题模式 ====================

/// 当前主题是否为深色
pub fn is_dark() -> bool {
    match get_mode() {
        ThemeMode::Auto => is_system_dark(),
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
    }
}

fn is_system_dark() -> bool {
    match theme_mode::ThemeMode::detect_system() {
        theme_mode::ThemeMode::Dark => true,
        _ => false,
    }
}

/// 获取当前主题模式
pub fn get_mode() -> ThemeMode {
    ThemeMode::get()
}

/// 模式的配置文件名（config.ini `theme_mode` 的取值）
pub fn mode_name(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::Auto => "auto",
    }
}

/// 主题切换按钮的标签：只反映两档（明亮/深色），按当前**生效**主题。
///
/// 【为什么不用 emoji】FLTK 仅使用全局字体（Microsoft YaHei UI）绘制文字，
/// 不会回退到彩色 emoji 字体（Segoe UI Emoji），🌓/☀️/🌑 会显示成豆腐块。
/// 【为什么没有 Auto】按要求只在启动时按系统深浅色自动选择一次，
/// 运行中的切换只在两档之间循环，不再常驻检测系统主题变化。
pub fn theme_btn_text() -> &'static str {
    if is_dark() {
        "深色"
    } else {
        "明亮"
    }
}

/// 设置主题模式
pub fn set_mode(mode: ThemeMode) {
    ThemeMode::set(mode);
    LAST_DARK_STORE.store(is_dark(), Ordering::Relaxed);
    apply_theme();
}

/// 上一次生效主题（用于 Auto 模式下检测"系统主题变了没有"）
static LAST_DARK_STORE: AtomicBool = AtomicBool::new(false);

/// 同步系统主题到当前模式（Auto 模式使用系统主题）。
/// 返回 `true` 表示生效主题发生变化，调用方应刷新整个控件树。
pub fn sync_from_system() -> bool {
    let mut changed = false;
    if get_mode() == ThemeMode::Auto {
        let now = is_dark();
        if now != LAST_DARK_STORE.load(Ordering::Relaxed) {
            LAST_DARK_STORE.store(now, Ordering::Relaxed);
            apply_theme();
            changed = true;
        }
    }
    changed
}

/// 应用当前主题到全局样式。
/// FLTK 的三类全局色分别驱动：普通控件底色 / 弹出菜单、Choice 下拉等
/// 亮底（FL_BACKGROUND2_COLOR）/ 默认前景（FL_FOREGROUND_COLOR）。
/// 逐控件显式设置过的颜色不受影响，由 `refresh_tree` 统一重刷。
fn apply_theme() {
    app::set_background_color(current::bg().0, current::bg().1, current::bg().2);
    app::set_background2_color(current::card().0, current::card().1, current::card().2);
    app::set_foreground_color(current::text().0, current::text().1, current::text().2);
}

// ==================== 配色定义 ====================

/// 浅色主题配色
pub mod light {
    use super::Rgb;
    pub const BG: Rgb = (0xF5, 0xF7, 0xFC);
    /// 渐变停靠点①（顶部，标题栏 DWM 融合用它）：淡科技蓝
    pub const BG_TOP: Rgb = (0xD6, 0xE4, 0xFC);
    /// 渐变停靠点②（中部过渡）
    pub const BG_MID: Rgb = (0xE7, 0xED, 0xF9);
    /// 渐变停靠点③（底部，中性浅灰，色相从蓝漂移到中性）
    pub const BG_BOTTOM: Rgb = (0xEF, 0xF0, 0xF4);
    pub const CARD: Rgb = (0xFF, 0xFF, 0xFF);
    /// 毛玻璃卡片：接近纯白的亮层，与渐变底部形成明显"浮起"层次
    pub const GLASS_CARD: Rgb = (0xFD, 0xFE, 0xFF);
    pub const INPUT_BG: Rgb = (0xEC, 0xF0, 0xF8);
    pub const BORDER: Rgb = (0xE1, 0xE6, 0xF1);
    pub const TEXT: Rgb = (0x19, 0x22, 0x33);
    pub const MUTED: Rgb = (0x67, 0x6E, 0x80);
    pub const PRIMARY: Rgb = (0x25, 0x63, 0xEB);
    pub const PRIMARY_DARK: Rgb = (0x1D, 0x4E, 0xD8);
    pub const PRIMARY_HOVER: Rgb = (0x3D, 0x83, 0xF6);
    pub const HOVER_SOFT: Rgb = (0xD6, 0xE1, 0xFB);
    pub const PRESS_SOFT: Rgb = (0xB9, 0xCC, 0xF7);
    pub const PRIMARY_SOFT: Rgb = (0xE5, 0xEE, 0xFD);
    pub const SUCCESS: Rgb = (0x17, 0xA3, 0x4A);
    pub const WARN: Rgb = (0xD9, 0x77, 0x06);
    pub const DANGER: Rgb = (0xDC, 0x26, 0x26);
    pub const HEADER_BG: Rgb = (0xFF, 0xFF, 0xFF);
    pub const SELECTION: Rgb = (0xD8, 0xE4, 0xFF);
}

/// 深色主题配色
pub mod dark {
    use super::Rgb;
    pub const BG: Rgb = (0x15, 0x17, 0x1E);
    /// 渐变停靠点①（顶部）：明显蓝调
    pub const BG_TOP: Rgb = (0x25, 0x31, 0x4E);
    /// 渐变停靠点②（中部过渡）
    pub const BG_MID: Rgb = (0x1A, 0x20, 0x30);
    /// 渐变停靠点③（底部，近黑中性）
    pub const BG_BOTTOM: Rgb = (0x12, 0x13, 0x18);
    pub const CARD: Rgb = (0x20, 0x23, 0x2D);
    /// 毛玻璃卡片：明显亮于渐变底，模拟深色磨砂
    pub const GLASS_CARD: Rgb = (0x28, 0x2F, 0x43);
    pub const INPUT_BG: Rgb = (0x2B, 0x2F, 0x3A);
    pub const BORDER: Rgb = (0x38, 0x3D, 0x4A);
    pub const TEXT: Rgb = (0xE4, 0xE8, 0xF2);
    pub const MUTED: Rgb = (0x92, 0x99, 0xAD);
    pub const PRIMARY: Rgb = (0x4D, 0x8E, 0xF7);
    pub const PRIMARY_DARK: Rgb = (0x2F, 0x70, 0xE0);
    pub const PRIMARY_HOVER: Rgb = (0x71, 0xA3, 0xFA);
    pub const HOVER_SOFT: Rgb = (0x39, 0x3F, 0x51);
    pub const PRESS_SOFT: Rgb = (0x31, 0x36, 0x45);
    pub const PRIMARY_SOFT: Rgb = (0x2B, 0x33, 0x45);
    pub const SUCCESS: Rgb = (0x3E, 0xD6, 0x8C);
    pub const WARN: Rgb = (0xF4, 0xB8, 0x4A);
    pub const DANGER: Rgb = (0xF0, 0x6B, 0x6B);
    pub const HEADER_BG: Rgb = (0x1A, 0x1D, 0x25);
    pub const SELECTION: Rgb = (0x2E, 0x46, 0x74);
}

/// 当前主题的配色（返回当前主题的值）
mod current {
    use super::Rgb;
    use super::is_dark;

    pub fn bg() -> Rgb { if is_dark() { super::dark::BG } else { super::light::BG } }
    pub fn bg_top() -> Rgb { if is_dark() { super::dark::BG_TOP } else { super::light::BG_TOP } }
    pub fn bg_mid() -> Rgb { if is_dark() { super::dark::BG_MID } else { super::light::BG_MID } }
    pub fn bg_bottom() -> Rgb { if is_dark() { super::dark::BG_BOTTOM } else { super::light::BG_BOTTOM } }
    pub fn card() -> Rgb { if is_dark() { super::dark::CARD } else { super::light::CARD } }
    pub fn glass_card() -> Rgb { if is_dark() { super::dark::GLASS_CARD } else { super::light::GLASS_CARD } }
    pub fn input_bg() -> Rgb { if is_dark() { super::dark::INPUT_BG } else { super::light::INPUT_BG } }
    pub fn border() -> Rgb { if is_dark() { super::dark::BORDER } else { super::light::BORDER } }
    pub fn text() -> Rgb { if is_dark() { super::dark::TEXT } else { super::light::TEXT } }
    pub fn muted() -> Rgb { if is_dark() { super::dark::MUTED } else { super::light::MUTED } }
    pub fn primary() -> Rgb { if is_dark() { super::dark::PRIMARY } else { super::light::PRIMARY } }
    pub fn primary_dark() -> Rgb { if is_dark() { super::dark::PRIMARY_DARK } else { super::light::PRIMARY_DARK } }
    pub fn primary_hover() -> Rgb { if is_dark() { super::dark::PRIMARY_HOVER } else { super::light::PRIMARY_HOVER } }
    pub fn hover_soft() -> Rgb { if is_dark() { super::dark::HOVER_SOFT } else { super::light::HOVER_SOFT } }
    pub fn press_soft() -> Rgb { if is_dark() { super::dark::PRESS_SOFT } else { super::light::PRESS_SOFT } }
    pub fn primary_soft() -> Rgb { if is_dark() { super::dark::PRIMARY_SOFT } else { super::light::PRIMARY_SOFT } }
    pub fn success() -> Rgb { if is_dark() { super::dark::SUCCESS } else { super::light::SUCCESS } }
    pub fn warn() -> Rgb { if is_dark() { super::dark::WARN } else { super::light::WARN } }
    pub fn danger() -> Rgb { if is_dark() { super::dark::DANGER } else { super::light::DANGER } }
    pub fn header_bg() -> Rgb { if is_dark() { super::dark::HEADER_BG } else { super::light::HEADER_BG } }
    pub fn selection() -> Rgb { if is_dark() { super::dark::SELECTION } else { super::light::SELECTION } }
}

// 导出当前主题的配色函数
pub fn BG() -> Rgb { current::bg() }
pub fn BG_TOP() -> Rgb { current::bg_top() }
pub fn BG_MID() -> Rgb { current::bg_mid() }
pub fn BG_BOTTOM() -> Rgb { current::bg_bottom() }
pub fn CARD() -> Rgb { current::card() }
pub fn GLASS_CARD() -> Rgb { current::glass_card() }
pub fn INPUT_BG() -> Rgb { current::input_bg() }
pub fn BORDER() -> Rgb { current::border() }
pub fn TEXT() -> Rgb { current::text() }
pub fn MUTED() -> Rgb { current::muted() }
pub fn PRIMARY() -> Rgb { current::primary() }
pub fn PRIMARY_DARK() -> Rgb { current::primary_dark() }
pub fn PRIMARY_HOVER() -> Rgb { current::primary_hover() }
pub fn HOVER_SOFT() -> Rgb { current::hover_soft() }
pub fn PRESS_SOFT() -> Rgb { current::press_soft() }
pub fn PRIMARY_SOFT() -> Rgb { current::primary_soft() }
pub fn SUCCESS() -> Rgb { current::success() }
pub fn WARN() -> Rgb { current::warn() }
pub fn DANGER() -> Rgb { current::danger() }
pub fn HEADER_BG() -> Rgb { current::header_bg() }
pub fn SELECTION() -> Rgb { current::selection() }

// ==================== 主题切换时控件颜色重映射 ====================

/// 浅色/深色两套配色的配对表（同一角色两两对应，顺序一致）。
///
/// 【为什么要这张表】FLTK 没有样式表，控件颜色在创建时就被写死成当时的
/// 主题值。切换主题时无法逐个重建控件，只能靠"旧值 → 新值"映射：
/// 遍历控件树，把命中旧主题色的颜色替换成新主题下同一角色的颜色。
const ROLE_PAIRS: [(Rgb, Rgb); 21] = [
    (light::BG, dark::BG),
    (light::BG_TOP, dark::BG_TOP),
    (light::BG_MID, dark::BG_MID),
    (light::BG_BOTTOM, dark::BG_BOTTOM),
    (light::CARD, dark::CARD),
    (light::GLASS_CARD, dark::GLASS_CARD),
    (light::INPUT_BG, dark::INPUT_BG),
    (light::BORDER, dark::BORDER),
    (light::TEXT, dark::TEXT),
    (light::MUTED, dark::MUTED),
    (light::PRIMARY, dark::PRIMARY),
    (light::PRIMARY_DARK, dark::PRIMARY_DARK),
    (light::PRIMARY_HOVER, dark::PRIMARY_HOVER),
    (light::HOVER_SOFT, dark::HOVER_SOFT),
    (light::PRESS_SOFT, dark::PRESS_SOFT),
    (light::PRIMARY_SOFT, dark::PRIMARY_SOFT),
    (light::SUCCESS, dark::SUCCESS),
    (light::WARN, dark::WARN),
    (light::DANGER, dark::DANGER),
    (light::HEADER_BG, dark::HEADER_BG),
    (light::SELECTION, dark::SELECTION),
];

/// 把一个"创建时写进控件的旧主题色"映射到当前主题下的同角色颜色。
/// 找不到（非主题调色板颜色，例如纯白/纯黑按钮字、透明底）时返回 `None`，
/// 调用方应跳过不改。
fn remap_role(color: Color) -> Option<Color> {
    let (r, g, b) = color.to_rgb();
    // alpha 都是 255 时 to_rgb 才等价；本工程所有主题色都经 rgb() 构造。
    let target = if is_dark() {
        ROLE_PAIRS
            .iter()
            .find(|(light, _)| *light == (r, g, b))
            .map(|(_, dark)| *dark)
    } else {
        ROLE_PAIRS
            .iter()
            .find(|(_, dark)| *dark == (r, g, b))
            .map(|(light, _)| *light)
    };
    target.map(|(r, g, b)| Color::from_rgba_tuple((r, g, b, 255)))
}

/// 递归刷新控件树的所有主题相关颜色。
///
/// 对每个控件处理：
/// - `color()`（底色）与 `selection_color()`（按下底色）：命中调色板即重映射；
/// - `label_color()`（文字色）：重映射，但**跳过纯白**——主按钮的
///   "白字"是有意为之（深浅两态都用白），不能被映射成卡片底色；
/// - Input 的 `text_color()`（输入文字色）单独处理（属于 InputExt）；
/// - Tree 的 `set_item_label_fgcolor()`（树节点文字色）单独处理，
///   它由 FLTK 内部绘制，不属于普通控件 label。
pub fn refresh_tree<W: WidgetExt>(w: &mut W) {
    restyle_widget(w);
    // 递归进入组（Group/Tabs/Flex/Wizard 等）的子控件继续刷新。
    // 注意：Tree 也是 Group 子类，但其内部树节点不是普通子控件，
    // 文字色由 set_item_label_fgcolor 在 restyle_widget 里单独处理。
    if let Some(g) = Group::from_dyn_widget(w) {
        let n = g.children();
        for i in 0..n {
            if let Some(mut c) = g.child(i) {
                refresh_tree(&mut c);
            }
        }
    }
}

/// 刷新单个控件的主题相关颜色。
fn restyle_widget<W: WidgetExt>(w: &mut W) {
    // 底色 / 按下底色
    if let Some(c) = remap_role(w.color()) {
        w.set_color(c);
    }
    if let Some(c) = remap_role(w.selection_color()) {
        w.set_selection_color(c);
    }
    // 文字色（跳过纯白：主按钮白字不能映射成卡片底色）
    if w.label_color() != Color::White {
        if let Some(c) = remap_role(w.label_color()) {
            w.set_label_color(c);
        }
    }
    // 输入框文字色
    if let Some(mut input) = Input::from_dyn_widget(w) {
        if let Some(c) = remap_role(input.text_color()) {
            input.set_text_color(c);
        }
    }
    // 树节点文字色（FLTK 内部绘制，与控件 label 色无关）
    if let Some(mut t) = Tree::from_dyn_widget(w) {
        t.set_item_label_fgcolor(rgb(current::text()));
        t.set_item_label_bgcolor(rgb(current::card()));
        // 【逐条目重刷】Fl_Tree_Item 构造时会把当时的主题色快照进条目内部
        // （_labelfgcolor/_labelbgcolor），之后既不跟随控件颜色也不跟随
        // prefs —— 只改上面两行会漏掉已存在的条目，表现为深色切浅色后
        // 磁盘列表仍是旧底色。必须遍历所有条目逐个重设。
        let mut cursor = t.first();
        while let Some(mut item) = cursor {
            item.set_label_fgcolor(rgb(current::text()));
            item.set_label_bgcolor(rgb(current::card()));
            cursor = t.next(&item);
        }
    }
}

pub trait IntoRgb {
    fn into_rgb(self) -> Rgb;
}

impl IntoRgb for Rgb {
    fn into_rgb(self) -> Rgb {
        self
    }
}

impl<F: Fn() -> Rgb> IntoRgb for F {
    fn into_rgb(self) -> Rgb {
        self()
    }
}

pub fn rgb(color: impl IntoRgb) -> Color {
    let (r, g, b) = color.into_rgb();
    Color::from_rgba_tuple((r, g, b, 255))
}

// ==================== 初始化 ====================

/// 全局字体与基础尺寸（必须在创建任何控件之前调用）。
pub fn init() {
    app::set_font(ui_font());
    app::set_font_size(12);
    LAST_DARK_STORE.store(is_dark(), Ordering::Relaxed);
    apply_theme();
}

/// 界面统用字体：中文优先用系统自带雅黑（缺失时 fltk 回落 Helvetica）。
///
/// 自绘控件（如磁盘行内的「规划分区」按钮）也要用它，才能与相邻控件同字体。
pub fn ui_font() -> Font {
    Font::by_name("Microsoft YaHei UI")
}

// ==================== 样式函数 ====================

/// 把 Flex 变成圆角毛玻璃卡片。
pub fn style_card(f: &mut Flex) {
    f.set_frame(FrameType::RFlatBox);
    f.set_color(rgb(current::glass_card()));
    f.set_margins(10, 10, 10, 10);
    f.set_spacing(5);
}

/// 卡片内嵌分组（透明底、保留边距与间距）：外框已合并到大卡片时使用。
pub fn style_section(f: &mut Flex) {
    f.set_frame(FrameType::NoBox);
    f.set_color(rgb(current::glass_card()));
    f.set_margins(10, 10, 10, 10);
    f.set_spacing(5);
}

/// 透明背景的普通容器（用于卡片列等不需要自绘背景的地方）。
pub fn style_plain(f: &mut Flex) {
    f.set_frame(FrameType::NoBox);
    f.set_color(rgb(current::bg()));
    f.set_margins(0, 0, 0, 0);
}

/// 卡片标题（小号深色粗体，左侧留白）。
pub fn title(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_font(Font::HelveticaBold);
    f.set_label_size(13);
    f.set_label_color(rgb(current::text()));
    f
}

/// 表单标签（右对齐不必要，左对齐 + 次要色）。
pub fn form_label(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_color(rgb(current::muted()));
    f.set_label_size(12);
    f
}

/// 普通文本标签。
pub fn label(txt: &str) -> Frame {
    let mut f = Frame::default().with_label(txt);
    f.set_align(Align::Left | Align::Inside);
    f.set_label_color(rgb(current::text()));
    f.set_label_size(12);
    f
}

/// 主操作按钮（实心底色白字；悬停明显变亮、按下明显变深）。
pub fn primary_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(current::primary()));
    b.set_selection_color(rgb(current::primary_dark()));
    b.set_label_color(Color::White);
    b.set_label_font(Font::HelveticaBold);
    b.set_label_size(13);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, current::primary_hover());
    b
}

/// 次级按钮（浅底、主色文字；悬停变浅蓝、按下更深一档）。
pub fn secondary_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(current::input_bg()));
    b.set_selection_color(rgb(current::press_soft()));
    b.set_label_color(rgb(current::primary()));
    b.set_label_size(12);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, current::primary_soft());
    b
}

/// 文字按钮（圆角小药丸：浅底 + 主色文字，页头次要操作）。
pub fn ghost_button(txt: &str) -> Button {
    let mut b = Button::default().with_label(txt);
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(rgb(current::input_bg()));
    b.set_selection_color(rgb(current::press_soft()));
    b.set_label_color(rgb(current::primary()));
    b.set_label_size(12);
    b.set_align(Align::Center | Align::Inside);
    add_hover_effect(&mut b, current::primary_soft());
    b
}

/// 给按钮添加悬停效果（进入/离开时切换颜色）。
/// 需要在按钮创建后立即调用，`hover_color` 为鼠标进入时的底色。
/// 支持传入 RGB 元组或函数引用（如 `theme::PRIMARY_SOFT`）。
///
/// 【主题切换兼容】进入/离开时都会先经 `remap_role` 重映射到当前主题的
/// 同角色颜色——主题切换后按钮底色已变，闭包里捕获的创建期颜色直接写回
/// 会把旧主题色刷回去。
pub fn add_hover_effect<B: fltk::prelude::WidgetBase>(btn: &mut B, hover_color: impl IntoRgb) {
    let default_color = btn.color();
    let hover_rgb = hover_color.into_rgb();
    btn.handle(move |w, ev| {
        match ev {
            fltk::enums::Event::Enter => {
                w.set_color(remap_role(Color::from_rgba_tuple((hover_rgb.0, hover_rgb.1, hover_rgb.2, 255)))
                    .unwrap_or_else(|| rgb((hover_rgb.0, hover_rgb.1, hover_rgb.2))));
                redraw_whole_window(w);
                true
            }
            fltk::enums::Event::Leave => {
                w.set_color(remap_role(default_color).unwrap_or(default_color));
                redraw_whole_window(w);
                true
            }
            _ => false,
        }
    });
}

/// 悬停重绘用整窗重绘，不用 `widget.redraw()` 局部重绘。
///
/// 【为什么】主窗口挂了自定义渐变背景绘制回调（`apply_window_backdrop`）。
/// 局部重绘曾经在 FL_DAMAGE_CHILD 路径下丢失标签（"鼠标经过按钮，按钮
/// 就没了"）；`apply_window_backdrop` 现已强制全量重绘子控件，该问题已从
/// 根源修复。整窗重绘保留为双保险——成本相同，且保证悬停换色与背景渐变
/// 永远一致。
fn redraw_whole_window<B: fltk::prelude::WidgetBase>(w: &B) {
    if let Some(mut win) = w.window() {
        win.redraw();
    }
}

/// 分段选择容器（浅底圆角条，GPT/MBR 等互斥选项放进里面形成"分段"外观）。
pub fn style_segmented(f: &mut Flex) {
    f.set_frame(FrameType::RFlatBox);
    f.set_color(rgb(current::input_bg()));
    f.set_margins(3, 3, 3, 3);
    f.set_spacing(3);
}

/// 分段条内的选项按钮（NoBox，透明文字色）。
pub fn segmented_item(w: &mut impl ButtonExt) {
    w.set_frame(FrameType::NoBox);
    w.set_down_frame(FrameType::NoBox);
    w.set_color(Color::TransparentBg);
    w.set_selection_color(Color::TransparentBg);
    w.set_label_color(rgb(current::text()));
    w.set_label_size(12);
}

/// 输入框样式（浅底圆角填充）。
pub fn style_input(i: &mut Input) {
    i.set_frame(FrameType::RFlatBox);
    i.set_color(rgb(current::input_bg()));
    i.set_text_color(rgb(current::text()));
    i.set_text_size(12);
}

/// 下拉框样式。
pub fn style_choice(c: &mut Choice) {
    c.set_frame(FrameType::RFlatBox);
    c.set_color(rgb(current::input_bg()));
    c.set_text_color(rgb(current::text()));
    c.set_text_size(12);
}

/// 复选框样式（仅统一文字颜色/字号）。
pub fn style_check(c: &mut impl WidgetExt) {
    c.set_label_color(rgb(current::text()));
    c.set_label_size(12);
}

/// 进度条样式。
pub fn style_progress(p: &mut Progress) {
    p.set_frame(FrameType::RFlatBox);
    p.set_color(rgb(current::input_bg()));
    p.set_selection_color(rgb(current::primary()));
}

// ==================== 渐变背景 + 标题栏融合（毛玻璃观感） ====================

use fltk::draw;

/// 画整窗竖向渐变背景：三停靠点平滑过渡（顶部科技蓝 → 中部过渡 → 底部中性）。
/// 顶部色与 DWM 标题栏色一致，从而标题栏与内容区融为一体；
/// 顶部到底部同时做明度与色相漂移（蓝 → 中性），层次感比两色渐变明显。
pub fn draw_bg_gradient(w: i32, h: i32) {
    let stops: [(f32, Rgb); 3] = if is_dark() {
        [
            (0.0, dark::BG_TOP),
            (0.5, dark::BG_MID),
            (1.0, dark::BG_BOTTOM),
        ]
    } else {
        [
            (0.0, light::BG_TOP),
            (0.5, light::BG_MID),
            (1.0, light::BG_BOTTOM),
        ]
    };
    let bands = (h / 2).max(1);
    for i in 0..bands {
        let t = i as f32 / (bands.max(2) - 1) as f32;
        draw::draw_rect_fill(0, i * 2, w, 2, sample_stops(&stops, t));
    }
    // 末尾不足一条 band 的余量补齐
    let rest = h - bands * 2;
    if rest > 0 {
        draw::draw_rect_fill(0, bands * 2, w, rest, sample_stops(&stops, 1.0));
    }
}

/// 在渐变停靠点之间线性插值取样。
fn sample_stops(stops: &[(f32, Rgb); 3], t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    // 找到 t 所在的停靠区间 [stops[i], stops[i+1]]
    let i = if t > stops[1].0 { 1 } else { 0 };
    let (t0, c0) = stops[i];
    let (t1, c1) = stops[i + 1];
    let f = if t1 > t0 {
        ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mix = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * f) as u8 };
    Color::from_rgba_tuple((
        mix(c0.0, c1.0),
        mix(c0.1, c1.1),
        mix(c0.2, c1.2),
        255,
    ))
}

/// 绘制链路修复标记：启动日志里会打印它，用来确认当前运行的是哪一版构建
/// （排障时一眼看出是不是旧 exe）。
pub const UI_FIX_TAG: &str = "ui-redraw-fix-2026-11-11-b";

/// 给顶层窗口挂上"渐变背景 + 子控件"的自绘回调。
/// 必须在 `win.end()`（子控件挂载完毕）之后、`show()` 之前调用。
pub fn apply_window_backdrop<W>(win: &mut W)
where
    W: WidgetBase + WindowExt + 'static,
{
    win.draw(|w| {
        let (cw, ch) = (w.w(), w.h());
        draw_bg_gradient(cw, ch);
        // 【局部重绘修复】本回调在默认绘制（Fl_Window::draw）**之后**执行
        // （cfltk 的 Widget_Derived 默认 super_draw_first=true），而默认绘制
        // 结束时已经把所有子控件的 damage 清零。于是接下来这次
        // draw_children 在「局部重绘」（FL_DAMAGE_CHILD：按钮按下、树行内
        // 按钮悬停、进度条推进等）时走的是 update_child 分支 —— 没有任何
        // 控件还带 damage，什么都不画，受损矩形里只剩渐变：
        //   - 按下任意按钮 → 按钮矩形变成方形色块，整窗重绘后才恢复；
        //   - 悬停树行内按钮（tree.redraw）→ 整个树区域被渐变覆盖成空。
        // 修复：把窗口 damage 强制置为 ALL，让 draw_children 走全量分支
        // （draw_child 会先 clear_damage(FL_DAMAGE_ALL) 再绘制，递归地把
        // 每个子控件都标成全量重绘），受损矩形内的父级卡片底色、树条目、
        // 按钮全部补画完整。屏幕拷贝仍按受损矩形裁剪，不会整窗闪烁；
        // 代价与一次整窗重绘相同（悬停效果本来就在做整窗重绘）。
        w.set_damage_type(fltk::enums::Damage::All);
        unsafe {
            fltk_sys::window::Fl_Double_Window_draw_children(
                w.as_widget_ptr() as *mut fltk_sys::window::Fl_Double_Window,
            );
        }
    });
}

/// 【标题栏与背景融为一体】
/// 通过 DWM 把标题栏/边框颜色设为渐变顶部色（Windows 11 生效；
/// Windows 10 不支持改标题栏颜色，会静默忽略，同时仍应用深浅色标题栏）。
/// 在窗口 `show()` 之后调用；主题切换后再次调用即可刷新。
#[cfg(target_os = "windows")]
pub fn apply_dwm_titlebar<W: WindowExt>(win: &W) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;

    // COLORREF 为 0x00BBGGRR
    fn colorref(c: Rgb) -> u32 {
        ((c.2 as u32) << 16) | ((c.1 as u32) << 8) | (c.0 as u32)
    }

    let hwnd = win.raw_handle() as HWND;
    if hwnd.is_null() {
        return;
    }
    let caption = colorref(current::bg_top());
    // DWMWA_COLOR_NONE (0xFFFFFFFE)：完全去掉边框线（Win11 支持）
    let border: u32 = 0xFFFF_FFFE;
    let text = colorref(current::text());
    let dark: i32 = is_dark() as i32;

    unsafe {
        // DWMWA_USE_IMMERSIVE_DARK_MODE = 20（Win10 19041+ / Win11）
        DwmSetWindowAttribute(hwnd, 20, &dark as *const _ as *const core::ffi::c_void, 4);
        // DWMWA_BORDER_COLOR = 34, DWMWA_CAPTION_COLOR = 35, DWMWA_TEXT_COLOR = 36（Win11）
        DwmSetWindowAttribute(hwnd, 34, &border as *const _ as *const core::ffi::c_void, 4);
        DwmSetWindowAttribute(hwnd, 35, &caption as *const _ as *const core::ffi::c_void, 4);
        DwmSetWindowAttribute(hwnd, 36, &text as *const _ as *const core::ffi::c_void, 4);
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply_dwm_titlebar<W: WindowExt>(_win: &W) {}