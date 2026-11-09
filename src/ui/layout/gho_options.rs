//! GHO options layout builder (mode selection, target selection, ghost path)

use fltk::button::RadioRoundButton;
use fltk::group::Flex;
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::prelude::*;

use super::super::theme;

/// Widget containers for the GHO options section
pub struct GhoOptionsUi {
    pub gho_group: Flex,
    pub gho_mode_disk_rb: RadioRoundButton,
    pub gho_mode_part_rb: RadioRoundButton,
    pub gho_disk_group: Flex,
    pub gho_disk_choice: Choice,
    pub gho_part_group: Flex,
    pub gho_part_choice: Choice,
    pub ghost_path_in: Input,
}

pub fn create_gho_options() -> GhoOptionsUi {
    // —— GHO 还原选项（高度 132 = 内边距 20 + 标题 18 + 3 行 × 26 + 2 间距 10）——
    //    「目标硬盘」与「目标分区」两行互斥显示，同一时刻只有 3 行可见。
    let ghost_default = r"%ProgramFiles(x86)%\ghost\ghost64.exe";
    let mut gho_group = Flex::default().column();
    theme::style_card(&mut gho_group);
    let gho_title = theme::title("GHO 还原选项");
    gho_group.fixed(&gho_title, 18);
    let mut gho_mode_row = Flex::default().row();
    gho_mode_row.set_color(theme::rgb(theme::CARD));
    let gm_label = theme::form_label("还原模式");
    gho_mode_row.fixed(&gm_label, 62);
    let mut gho_mode_disk_rb = RadioRoundButton::default().with_label("硬盘还原");
    gho_mode_disk_rb.set(true);
    theme::style_check(&mut gho_mode_disk_rb);
    gho_mode_row.fixed(&gho_mode_disk_rb, 92);
    let mut gho_mode_part_rb = RadioRoundButton::default().with_label("分区还原");
    theme::style_check(&mut gho_mode_part_rb);
    gho_mode_row.fixed(&gho_mode_part_rb, 92);
    gho_mode_row.end();
    gho_group.fixed(&gho_mode_row, 26);

    let mut gho_disk_group = Flex::default().row();
    gho_disk_group.set_color(theme::rgb(theme::CARD));
    let gd_label = theme::form_label("目标硬盘");
    gho_disk_group.fixed(&gd_label, 62);
    let mut gho_disk_choice = Choice::default();
    theme::style_choice(&mut gho_disk_choice);
    gho_disk_group.end();
    gho_group.fixed(&gho_disk_group, 26);

    let mut gho_part_group = Flex::default().row();
    gho_part_group.set_color(theme::rgb(theme::CARD));
    let gp_label = theme::form_label("目标分区");
    gho_part_group.fixed(&gp_label, 62);
    let mut gho_part_choice = Choice::default();
    theme::style_choice(&mut gho_part_choice);
    gho_part_group.end();
    gho_group.fixed(&gho_part_group, 26);
    gho_part_group.hide();

    let mut ghost_row = Flex::default().row();
    ghost_row.set_color(theme::rgb(theme::CARD));
    let ghost_label = theme::form_label("Ghost 路径");
    ghost_row.fixed(&ghost_label, 70);
    let mut ghost_path_in = Input::default();
    ghost_path_in.set_value(ghost_default);
    theme::style_input(&mut ghost_path_in);
    ghost_row.end();
    gho_group.fixed(&ghost_row, 26);
    gho_group.end();

    GhoOptionsUi {
        gho_group,
        gho_mode_disk_rb,
        gho_mode_part_rb,
        gho_disk_group,
        gho_disk_choice,
        gho_part_group,
        gho_part_choice,
        ghost_path_in,
    }
}