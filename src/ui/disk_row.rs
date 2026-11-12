//! 磁盘列表行内「规划分区」按钮：**树内自绘 + 命中检测**。
//!
//! # 为什么不再用 FLTK 的 tree-as-container
//!
//! 之前每个硬盘行挂一个真正的 `Fl_Button`（`TreeItem::set_widget` +
//! `TreeItemDrawMode::LabelAndWidget`）。这在 FLTK 上有两个无法绕开的缺陷：
//!
//! 1. **按钮位置由标签文本宽度决定，不是右对齐**
//!    `Fl_Tree_Item::draw()` 把控件 x 算成
//!    `label_x + 实测标签宽度 + widget_margin_left`，标签越长按钮越靠右。
//!    「磁盘 0: ST1000DM010-2EP102 （HDD）  [931.51 GB]」在 260~306px 宽的
//!    紧凑左列里远超可视宽度，按钮被推到树的右边界之外 —— 用户看不见，
//!    自然也点不到。"规划分区按钮无法使用"就是这个原因。
//!
//! 2. **滚动 / 重绘时行内控件会留下残影，整行显示错乱**
//!    `Fl_Tree_Item::draw()` 每次绘制都会 `widget()->resize()` 把控件按
//!    当前滚动偏移重新定位；`resize` 会给父控件打上 `FL_DAMAGE_CHILD`。
//!    一旦树只剩 child 级 damage，`Fl_Tree_Item::draw()` 里
//!    `if (tree()->damage() & ~FL_DAMAGE_CHILD)` 不成立 → **条目不重绘**，
//!    只重绘被 resize 的子控件 → 旧位置的按钮像素不被背景覆盖。
//!    拖动横向滚动条时按钮跟着位移，于是屏幕上一路留下"幽灵按钮"，
//!    文字也被压在下面 —— 也就是"拖动横向滚动条列表显示异常"。
//!
//! # 现在的做法
//!
//! 行内按钮**不是控件**，而是树的 draw 钩子（`Tree::draw`，cfltk 的
//! `inner_drawer` 在 `Fl_Tree::draw()` 之后执行）在每行右侧自绘的一个
//! 圆角矩形：
//!
//! - 位置由 `label_x + label_w`（= 树可见内容区右边界，FLTK 内部就是这么
//!   算的）向右吸附，**与标签长度和滚动偏移无关**，永远贴着可视区右边；
//! - 命中检测用同一套几何（`row_button_rect` / `button_rect`），
//!   因此"画在哪就点在哪"；
//! - 没有任何子控件参与树的绘制 → 滚动时只有正常的一次整树重绘，
//!   不存在残影，也不需要 `Fl::delete_widget` 那套延迟删除（重建树时
//!   再没有"幽灵按钮"要手工销毁，顺带消除了回调栈内 use-after-free 风险）。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use fltk::app;
use fltk::draw;
use fltk::enums::{Align, Event};
use fltk::prelude::*;
use fltk::tree::{Tree, TreeItem};

use super::theme;

/// 按钮文案（宽度按它实测）。
const BTN_LABEL: &str = "规划分区";
/// 按钮字号：与树标签同号（`tree.set_item_label_size(12)`），此前 10 号偏小。
const BTN_FONT_SIZE: i32 = 12;
/// 文案左右内边距之和（字号变大后同步放大，避免文字贴边）。
const BTN_PAD_X: i32 = 18;
/// 按钮宽度上下限（实测宽度异常时的兜底）。
const BTN_MIN_W: i32 = 56;
const BTN_MAX_W: i32 = 100;
/// 按钮与树可见区右边界之间的距离。
const BTN_INSET_RIGHT: i32 = 6;
/// 按钮高度（仍受行高约束：实际取 `min(BTN_H, 行高 - BTN_ROW_GAP)`）。
const BTN_H: i32 = 20;
/// 与行高的最小间距。
const BTN_ROW_GAP: i32 = 2;

/// 行内按钮的事件结果。
pub enum RowEvent {
    /// 与本按钮无关，交回 FLTK / 树继续处理
    Ignored,
    /// 命中按钮但还没有完整点击（如按下、悬停），事件已被消费
    Consumed,
    /// 完成一次点击：`(disk_idx, phys)`
    Clicked(u32, u32),
}

/// 一行硬盘（「规划分区」按钮的数据源）。
#[derive(Clone, Debug)]
pub struct DiskRow {
    /// 树中该硬盘节点的路径（重建树时写入，绘制/命中的定位依据）
    pub path: String,
    /// 物理磁盘号
    pub phys: u32,
    /// 磁盘序号（=`disk_data` 的 key，回调 `on_one_click` 用）
    pub disk_idx: u32,
}

/// 行内按钮的绘制与命中状态。
///
/// 放在 `Rc` 里由「树的 draw 钩子」「树的 handle 钩子」「App」三方共享：
/// 前两者是 `'static` 闭包，只能通过共享单元读取当前行列表与悬停态。
#[derive(Default)]
pub struct DiskRowUi {
    rows: RefCell<Vec<DiskRow>>,
    /// 当前悬停的物理盘号（`None` = 无）
    hover: Cell<Option<u32>>,
    /// 按下时命中的物理盘号（松开时校验是否仍在同一按钮上）
    pressed: Cell<Option<u32>>,
    /// 安装进行中置 false：按钮变灰且不响应点击
    enabled: Cell<bool>,
}

impl DiskRowUi {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            enabled: Cell::new(true),
            ..Self::default()
        })
    }

    /// 重建树后同步行列表（路径变了，旧几何全部失效）。
    pub fn set_rows(&self, rows: Vec<DiskRow>) {
        self.rows.replace(rows);
        self.hover.set(None);
        self.pressed.set(None);
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.set(on);
        if !on {
            self.hover.set(None);
            self.pressed.set(None);
        }
    }

    /// 树自绘钩子：在每行右侧画「规划分区」按钮。
    ///
    /// 必须在 `Fl_Tree::draw()` **之后**执行（cfltk 的 `inner_drawer` 就是
    /// 这个时机），这样按钮盖在行文字之上、且每次重绘（含滚动）都跟着重画。
    pub fn draw(&self, tree: &Tree) {
        let rows = self.rows.borrow();
        if rows.is_empty() || !tree.visible() || tree.w() <= 0 || tree.h() <= 0 {
            return;
        }
        let (bw, bh) = button_dims();

        // 裁剪到树的**可见内容区**：右侧让开竖向滚动条、底部让开横向滚动条，
        // 否则按钮会盖在滚动条上（滚动条是在 Fl_Tree::draw() 里最后画的，
        // 我们的钩子在它之后，不裁剪就会反盖回去）。
        let sb = scrollbar_size(tree);
        let clip_x = tree.x();
        let clip_y = tree.y();
        let clip_w = tree.w() - if tree.is_vscroll_visible() { sb } else { 0 };
        let clip_h = tree.h() - if tree.is_hscroll_visible() { sb } else { 0 };
        if clip_w <= 0 || clip_h <= 0 {
            return;
        }

        draw::push_clip(clip_x, clip_y, clip_w, clip_h);
        for row in rows.iter() {
            let Some(item) = tree.find_item(&row.path) else {
                continue;
            };
            let Some(rect) = row_button_rect(&item, bw, bh) else {
                continue;
            };
            // 行的可见性：整行都在裁剪区外时无需画（裁剪本身也会丢弃，
            // 但这里提前跳过可以省掉每帧的文字测量）
            if rect.1 + rect.3 < clip_y || rect.1 > clip_y + clip_h {
                continue;
            }
            let hovered = self.hover.get() == Some(row.phys);
            self.draw_button(rect, hovered);
        }
        draw::pop_clip();
    }

    /// 树的事件钩子：把点击映射回 `(disk_idx, phys)`。
    ///
    /// 松手才算一次点击（与 FLTK 按钮一致）：按下时记录命中的盘，松开时
    /// 必须仍落在同一按钮上，拖动出去再松手不触发。
    pub fn handle(&self, tree: &Tree, ev: Event) -> RowEvent {
        if self.rows.borrow().is_empty() {
            return RowEvent::Ignored;
        }
        let (mx, my) = (app::event_x(), app::event_y());
        match ev {
            Event::Push => {
                if !self.enabled.get() {
                    return RowEvent::Ignored;
                }
                match self.hit_test(tree, mx, my) {
                    Some((_disk_idx, phys)) => {
                        self.pressed.set(Some(phys));
                        // 按下就吃掉事件：避免树把这次点击当成"点空白处"
                        RowEvent::Consumed
                    }
                    None => RowEvent::Ignored,
                }
            }
            Event::Released => {
                let Some(pressed) = self.pressed.take() else {
                    return RowEvent::Ignored;
                };
                match self.hit_test(tree, mx, my) {
                    Some((disk_idx, phys)) if phys == pressed && self.enabled.get() => {
                        RowEvent::Clicked(disk_idx, phys)
                    }
                    _ => RowEvent::Ignored,
                }
            }
            // Enter 与 Move 同样处理：鼠标从窗口外直接进入按钮（或刚切回本窗口）
            // 时 FLTK 只发一次 FL_ENTER、不发 FL_MOVE，漏掉它按钮就不会高亮。
            Event::Move | Event::Drag | Event::Enter => {
                let hit = if self.enabled.get() {
                    self.hit_test(tree, mx, my).map(|h| h.1)
                } else {
                    None
                };
                if hit != self.hover.get() {
                    self.hover.set(hit);
                    redraw_window(tree);
                }
                RowEvent::Ignored
            }
            Event::Leave => {
                if self.hover.get().is_some() {
                    self.hover.set(None);
                    redraw_window(tree);
                }
                RowEvent::Ignored
            }
            _ => RowEvent::Ignored,
        }
    }

    /// 命中检测：返回 `(disk_idx, phys)`。
    fn hit_test(&self, tree: &Tree, mx: i32, my: i32) -> Option<(u32, u32)> {
        let rows = self.rows.borrow();
        let (bw, bh) = button_dims();
        for row in rows.iter() {
            let Some(item) = tree.find_item(&row.path) else {
                continue;
            };
            let Some((x, y, w, h)) = row_button_rect(&item, bw, bh) else {
                continue;
            };
            if mx >= x && mx < x + w && my >= y && my < y + h {
                return Some((row.disk_idx, row.phys));
            }
        }
        None
    }

    fn draw_button(&self, rect: (i32, i32, i32, i32), hovered: bool) {
        let (x, y, w, h) = rect;
        let enabled = self.enabled.get();
        // 先用行底色清一次矩形：标签文字是 FLTK 在 draw_item_content 里画的
        // （先于本钩子），标签过长时会有字形落在按钮范围内，圆角外的四个
        // 角落盖不住。行底色 == 树底色，清完不会有色块接缝。
        draw::draw_rect_fill(x, y, w, h, theme::rgb(theme::CARD()));
        let bg = if hovered && enabled {
            theme::rgb(theme::PRIMARY_SOFT())
        } else {
            theme::rgb(theme::INPUT_BG())
        };
        draw::set_draw_color(bg);
        draw::draw_rounded_rectf(x, y, w, h, 4);
        draw::set_draw_color(theme::rgb(if hovered && enabled {
            theme::PRIMARY()
        } else {
            theme::BORDER()
        }));
        draw::draw_rounded_rect(x, y, w, h, 4);
        draw::set_font(theme::ui_font(), BTN_FONT_SIZE);
        draw::set_draw_color(theme::rgb(if enabled {
            theme::PRIMARY()
        } else {
            theme::MUTED()
        }));
        draw::draw_text2(BTN_LABEL, x, y, w, h, Align::Center);
    }
}

/// 悬停换色后重绘：**整窗重绘**，不用 `tree.redraw()` 局部重绘。
///
/// 主窗口挂有渐变背景自绘回调（`theme::apply_window_backdrop`），
/// 局部重绘（FL_DAMAGE_CHILD）走的是 `update_child` 分支，这条路径容易
/// 出现"父级底色没重画 / 只剩渐变"的残影。整窗重绘走完整的
/// 「渐变 + 全部子控件」绘制流程，与其它控件（按钮悬停）保持一致。
fn redraw_window(tree: &Tree) {
    if let Some(mut win) = tree.window() {
        win.redraw();
    }
}

/// 实测按钮尺寸：文案宽度 + 左右内边距，高度取 `BTN_H`。
fn button_dims() -> (i32, i32) {
    draw::set_font(theme::ui_font(), BTN_FONT_SIZE);
    let (tw, _th) = draw::measure(BTN_LABEL, true);
    ((tw + BTN_PAD_X).clamp(BTN_MIN_W, BTN_MAX_W), BTN_H)
}

/// 树滚动条宽度（树自己没设时回落到全局值）。
fn scrollbar_size(tree: &Tree) -> i32 {
    let s = tree.scrollbar_size();
    if s > 0 {
        s
    } else {
        app::scrollbar_size()
    }
}

/// 计算某行的按钮矩形。
///
/// 横向：**吸附在树可见内容区右边界**（`label_x + label_w`，FLTK 在
/// `Fl_Tree_Item::draw()` 里就是这么定义 `_label_xywh[2]` 的），因此按钮
/// 位置与标签长度、横向滚动偏移都无关。标签过长时文字会被按钮盖住，
/// 而不是把按钮推出可视区。
///
/// 纵向：在行内垂直居中，并保证至少留 `BTN_ROW_GAP / 2` 的上下空隙。
fn row_button_rect(item: &TreeItem, bw: i32, bh: i32) -> Option<(i32, i32, i32, i32)> {
    let label_w = item.label_w();
    // 行还没被绘制过（label_w 尚未计算）时跳过
    if label_w <= 0 {
        return None;
    }
    let right = item.label_x() + label_w;
    let row_y = item.y();
    let row_h = item.h();
    if row_h <= 0 {
        return None;
    }
    let h = bh.min(row_h - BTN_ROW_GAP).max(12);
    let y = row_y + (row_h - h) / 2;
    Some(button_rect(right, y, h, bw))
}

/// 纯几何部分：把按钮贴到 `right` 右侧内边距处（可单测）。
fn button_rect(right: i32, y: i32, h: i32, bw: i32) -> (i32, i32, i32, i32) {
    (right - bw - BTN_INSET_RIGHT, y, bw, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按钮必须吸附在可视区右边界内侧，不受行宽（= 标签长度）影响。
    #[test]
    fn button_sticks_to_right_edge() {
        for right in [200, 640, 1200] {
            let (x, y, w, h) = button_rect(right, 40, 16, 60);
            assert_eq!(x + w, right - BTN_INSET_RIGHT);
            assert_eq!(y, 40);
            assert_eq!((w, h), (60, 16));
        }
    }

    /// 按钮永远落在树的可视宽度内（贴右不动，标签再长也不会被推出去）。
    #[test]
    fn button_stays_inside_viewport() {
        // 紧凑左列最窄情形：可视内容区只有 180px
        let right = 180;
        let (x, _y, w, _h) = button_rect(right, 0, 16, 60);
        assert!(x >= 0, "按钮左边越界: {x}");
        assert!(x + w <= right, "按钮右边越界");
    }
}
