//! WIM options layout builder (formatting, boot repair, auto-restart, defender, firewall, tools, optimization)

use fltk::button::{Button, CheckButton};
use fltk::enums::FrameType;
use fltk::frame::Frame;
use fltk::group::Flex;
use fltk::input::Input;
use fltk::prelude::*;

use super::super::theme;
use crate::config::settings::AppConfig;

/// Widget containers for the WIM options section
pub struct WimOptionsUi {
    pub wim_options_group: Flex,
    pub format_cb: CheckButton,
    pub repair_boot_cb: CheckButton,
    pub restart_cb: CheckButton,
    pub defender_cb: CheckButton,
    pub firewall_cb: CheckButton,
    pub copy_tools_cb: CheckButton,
    pub tools_path_in: Input,
    pub browse_tools_btn: Button,
    pub deploy_cb: CheckButton,
    pub admin_user_in: Input,
}

pub fn create_wim_options(config: &AppConfig) -> WimOptionsUi {
    let mut wim_options_group = Flex::default().column();
    wim_options_group.set_frame(FrameType::NoBox);
    wim_options_group.set_color(theme::rgb(theme::CARD));
    wim_options_group.set_margins(0, 0, 0, 0);
    wim_options_group.set_spacing(5);

    // 行1：格式化 / 修复引导 / 安装完成后重启
    let mut opt_row1 = Flex::default().row();
    opt_row1.set_color(theme::rgb(theme::CARD));
    let mut format_cb = CheckButton::default().with_label("格式化目标分区");
    theme::style_check(&mut format_cb);
    opt_row1.fixed(&format_cb, 130);
    let mut repair_boot_cb = CheckButton::default().with_label("修复引导");
    repair_boot_cb.set(true);
    theme::style_check(&mut repair_boot_cb);
    opt_row1.fixed(&repair_boot_cb, 100);
    let mut restart_cb = CheckButton::default().with_label("安装完成后重启");
    restart_cb.set_checked(config.auto_restart);
    theme::style_check(&mut restart_cb);
    opt_row1.fixed(&restart_cb, 150);
    let mut opt_spacer1 = Frame::default();
    opt_spacer1.set_color(theme::rgb(theme::CARD));
    opt_row1.fixed(&opt_spacer1, 1);
    opt_row1.end();
    wim_options_group.fixed(&opt_row1, 26);

    // 行2：禁用 Defender / 关闭防火墙
    let mut opt_row2 = Flex::default().row();
    opt_row2.set_color(theme::rgb(theme::CARD));
    let mut defender_cb = CheckButton::default().with_label("禁用 Windows Defender");
    theme::style_check(&mut defender_cb);
    opt_row2.fixed(&defender_cb, 176);
    let mut firewall_cb = CheckButton::default().with_label("关闭 Windows 防火墙");
    theme::style_check(&mut firewall_cb);
    opt_row2.fixed(&firewall_cb, 168);
    let mut opt_spacer2 = Frame::default();
    opt_spacer2.set_color(theme::rgb(theme::CARD));
    opt_row2.fixed(&opt_spacer2, 1);
    opt_row2.end();
    wim_options_group.fixed(&opt_row2, 26);

    // 行3：复制调试工具 + 源目录
    let mut copy_row = Flex::default().row();
    copy_row.set_color(theme::rgb(theme::CARD));
    let mut copy_tools_cb = CheckButton::default().with_label("复制调试工具到桌面:");
    theme::style_check(&mut copy_tools_cb);
    copy_row.fixed(&copy_tools_cb, 150);
    let mut tools_path_in = Input::default();
    tools_path_in.set_value(&config.tools_source_path);
    theme::style_input(&mut tools_path_in);
    let browse_tools_btn = theme::secondary_button("浏览...");
    copy_row.fixed(&browse_tools_btn, 84);
    copy_row.end();
    wim_options_group.fixed(&copy_row, 26);

    // 行4：自动部署与系统优化 + 用户名
    let mut deploy_row = Flex::default().row();
    deploy_row.set_color(theme::rgb(theme::CARD));
    let mut deploy_cb = CheckButton::default().with_label("自动部署与系统优化");
    theme::style_check(&mut deploy_cb);
    deploy_row.fixed(&deploy_cb, 152);
    let user_label = theme::form_label("用户名");
    deploy_row.fixed(&user_label, 52);
    let mut admin_user_in = Input::default();
    admin_user_in.set_value("admin");
    theme::style_input(&mut admin_user_in);
    deploy_row.fixed(&admin_user_in, 120);
    let mut opt_spacer3 = Frame::default();
    opt_spacer3.set_color(theme::rgb(theme::CARD));
    deploy_row.fixed(&opt_spacer3, 1);
    deploy_row.end();
    wim_options_group.fixed(&deploy_row, 26);
    wim_options_group.end();

    WimOptionsUi {
        wim_options_group,
        format_cb,
        repair_boot_cb,
        restart_cb,
        defender_cb,
        firewall_cb,
        copy_tools_cb,
        tools_path_in,
        browse_tools_btn,
        deploy_cb,
        admin_user_in,
    }
}