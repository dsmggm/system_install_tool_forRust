//! Install source layout builder (path input, browse button, image choice)

use fltk::button::Button;
use fltk::group::Flex;
use fltk::input::Input;
use fltk::menu::Choice;
use fltk::prelude::*;

use super::super::theme;

/// Widget containers for the install source section
pub struct InstallSourceUi {
    pub source_group: Flex,
    pub source_path_in: Input,
    pub browse_source_btn: Button,
    pub image_choice: Choice,
}

pub fn create_install_source() -> InstallSourceUi {
    // —— 安装源（高度 100 = 内边距 20 + 标题 18 + 间距 10 + 2 行 × 26）——
    let mut source_group = Flex::default().column();
    theme::style_card(&mut source_group);
    let _source_title = theme::title("安装源");
    source_group.fixed(&_source_title, 18);

    let mut source_row = Flex::default().row();
    source_row.set_color(theme::rgb(theme::CARD));
    let path_label = theme::form_label("路径");
    source_row.fixed(&path_label, 44);
    let mut source_path_in = Input::default();
    source_path_in.set_readonly(true); // 路径只读：只能通过「浏览...」写入
    theme::style_input(&mut source_path_in);
    let browse_source_btn = theme::secondary_button("浏览...");
    source_row.fixed(&browse_source_btn, 84);
    source_row.end();
    source_group.fixed(&source_row, 26);

    let mut image_row = Flex::default().row();
    image_row.set_color(theme::rgb(theme::CARD));
    let img_label = theme::form_label("镜像");
    image_row.fixed(&img_label, 44);
    let mut image_choice = Choice::default();
    theme::style_choice(&mut image_choice);
    image_row.end();
    source_group.fixed(&image_row, 26);
    source_group.end();

    InstallSourceUi {
        source_group,
        source_path_in,
        browse_source_btn,
        image_choice,
    }
}