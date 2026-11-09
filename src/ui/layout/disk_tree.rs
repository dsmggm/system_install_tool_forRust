//! Disk tree layout builder (left column, tree widget)

use fltk::enums::FrameType;
use fltk::group::Flex;
use fltk::prelude::*;
use fltk::tree::Tree;

use super::super::theme;

/// Widget containers for the disk tree section
pub struct DiskTreeUi {
    pub left_col: Flex,
    pub tree: Tree,
}

pub fn create_disk_tree() -> DiskTreeUi {
    // -- 左：磁盘与分区卡片 --
    let mut left = Flex::default().column();
    theme::style_card(&mut left);
    let _left_title = theme::title("磁盘与分区");
    left.fixed(&_left_title, 20);
    let mut tree = Tree::new(0, 0, 0, 0, None);
    // 【去掉选中功能】树改为纯展示列表：点击节点不再产生选中态/
    // 高亮（此前单选/多选行为不稳定，索性彻底禁用选中）。
    tree.set_select_mode(fltk::tree::TreeSelect::None);
    // 【Bug A】选中行底框改用 FlatBox，让选中行真正被填色。
    // `Fl_Tree_Item::draw_item_content()` 绘制选中底色时走
    //   `fl_draw_box(prefs.selectbox(), ..., bg)`，而 `bg` 就是
    //   `selection_color()`。EngravedFrame 只描边不填色 → 选中态不可见；
    //   FlatBox 用 `fl_rectf` 填满整行，浅蓝底 + 黑字清晰可读。
    tree.set_select_frame(FrameType::FlatBox);
    // 【Bug C】不显示 FLTK 默认的 "ROOT" 根节点行。
    tree.set_show_root(false);
    // 【Bug A】选中底色改成浅色，让选中项文字可读（深色底会触发
    // fl_contrast 选白字，而 inactive 时底色被冲淡 → 白字浅底不可读）。
    tree.set_selection_color(theme::rgb(theme::SELECTION));
    // 行内控件（每硬盘一个「一键分区」按钮）与标签同行显示：
    // FLTK 默认 DrawDefault 会用子控件**替换**节点文本，必须显式选择这个组合。
    tree.set_item_draw_mode(fltk::tree::TreeItemDrawMode::LabelAndWidget);
    // 行内按钮与节点文字之间留一点空隙
    tree.set_widget_margin_left(6);
    // 现代观感：去掉连接线、统一字号、与卡片同底色
    tree.set_connector_style(fltk::tree::TreeConnectorStyle::None);
    // 直接去掉树的收缩/展开功能：不显示展开箭头（用户此前反馈
    // 收缩/展开按钮不起效果），分区子行永远展开显示。
    tree.set_show_collapse(false);
    tree.set_item_label_size(12);
    tree.set_frame(FrameType::NoBox);
    tree.set_color(theme::rgb(theme::CARD));
    left.end();

    DiskTreeUi { left_col: left, tree }
}