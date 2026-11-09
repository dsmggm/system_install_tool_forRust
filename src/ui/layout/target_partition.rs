//! Target partition layout builder (disk choice, partition preview)

use fltk::frame::Frame;
use fltk::group::Flex;
use fltk::menu::Choice;
use fltk::prelude::*;

use super::super::theme;

/// Widget containers for the target partition section
pub struct TargetPartitionUi {
    pub target_group: Flex,
    pub disk_choice: Choice,
    pub partition_preview: Frame,
}

pub fn create_target_partition() -> TargetPartitionUi {
    // —— 目标分区（高度 98 = 内边距 20 + 标题 18 + 间距 10 + 26 + 24）——
    let mut target_group = Flex::default().column();
    theme::style_card(&mut target_group);
    let _target_title = theme::title("目标分区");
    target_group.fixed(&_target_title, 18);

    let mut disk_row = Flex::default().row();
    disk_row.set_color(theme::rgb(theme::CARD));
    let disk_label = theme::form_label("系统安装硬盘");
    disk_row.fixed(&disk_label, 96);
    let mut disk_choice = Choice::default();
    theme::style_choice(&mut disk_choice);
    disk_row.end();
    target_group.fixed(&disk_row, 26);

    let mut partition_preview = theme::label("");
    partition_preview.set_label_color(theme::rgb(theme::MUTED));
    target_group.fixed(&partition_preview, 24);
    target_group.end();

    TargetPartitionUi {
        target_group,
        disk_choice,
        partition_preview,
    }
}