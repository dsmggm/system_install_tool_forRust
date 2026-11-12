//! 主窗口布局度量：**正常** / **紧凑** 两档（低分辨率适配）。
//!
//! 背景：主窗口的每一块都是 `Fl_Flex` 的定高（`fixed`）分区，定高之和就是
//! 内容的最小高度。屏幕明明是 800×600 时，正常档需要约 566×814 的客户区，
//! 而可用客户区只有 760×510 —— 于是右侧「安装选项」卡片与「开始安装」按钮
//! 被裁掉、右侧复选框被切掉右半截（窗口本身没法再变大，因为屏幕就这么大）。
//!
//! 这里把散落在 `app.rs` 里的魔数集中成两套度量：
//! - [`Layout::normal`]：与改造前完全一致的尺寸（大屏视觉零变化）；
//! - [`Layout::compact`]：收窄内/外边距、行高 26→24、标题间距 5→4，
//!   在 800×600 下内容高度 506、最小宽度 714，都能塞进 760×510 的客户区。
//!
//! [`Layout::detect`] 按 `screen_size()` 选档并顺带算出窗口尺寸/位置，
//! 窗口几何不再由 `app.rs` 手推。

use fltk::app as fapp;

/// 左右两列之间分隔线的宽度
pub const DIVIDER_W: i32 = 1;

/// 页脚「当前方案」路径标签字号（13px，与按钮字号一致）。
///
/// 3 行 ×（字号 + 行距 3）≈ 48px，由 [`Layout::foot_row_h`] 兜住：
/// 正常档 52 / 紧凑档 50。放大字号必须同步加高页脚行，
/// `scheme_label_fits_footer_row` 测试会拦截不匹配的组合。
pub const SCHEME_LABEL_SIZE: i32 = 13;
/// 「当前方案」路径最多折成几行（超出部分由 tooltip 兜底）。
pub const SCHEME_MAX_LINES: usize = 3;

/// 窗口最大客户区
const WIN_MAX_W: i32 = 1240;
const WIN_MAX_H: i32 = 860;

/// 一整套布局度量（含窗口几何）。
///
/// 字段全部为「客户区像素」，与 FLTK 顶层窗口的 `w()/h()` 口径一致
/// （FLTK 的 `w()/h()` 不含边框与标题栏）。
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// 是否使用紧凑档
    pub compact: bool,

    // ---------- 窗口几何 ----------
    pub win_x: i32,
    pub win_y: i32,
    pub win_w: i32,
    pub win_h: i32,
    /// 窗口最小客户区（`size_range` 用）
    pub min_w: i32,
    pub min_h: i32,

    // ---------- 根容器 ----------
    pub root_margin: i32,
    pub root_spacing: i32,

    // ---------- 页头 ----------
    pub header_h: i32,
    pub head_row_h: i32,
    pub head_pad_x: i32,
    pub head_pad_y: i32,

    // ---------- 主体 ----------
    pub main_pad_x: i32,
    pub main_pad_y: i32,
    pub main_spacing: i32,
    /// 左列（磁盘树）宽度区间
    pub left_min: i32,
    pub left_max: i32,
    /// 右列（设置卡片）最小宽度：由最宽一行固定控件决定
    pub right_min: i32,
    /// 右列各卡片之间的间距
    pub right_spacing: i32,

    // ---------- 卡片 ----------
    pub section_pad_x: i32,
    pub section_pad_y: i32,
    pub section_spacing: i32,
    pub title_h: i32,
    pub left_title_h: i32,
    pub row_h: i32,
    pub preview_h: i32,
    /// 卡片高度余量：定高比内容略高，吸收圆角框内缩与取整误差
    pub card_slack: i32,
    /// GHO 两张卡片的余量（历史值比普通卡片多 3px，保持大屏观感不变）
    pub gho_slack: i32,

    // ---------- 卡片定高 ----------
    pub source_h: i32,
    pub target_h: i32,
    pub gho_h: i32,
    /// 「安装选项」外框（WIM 模式：5 行）
    pub options_h: i32,
    /// 「安装选项」外框（GHO 模式：1 行）
    pub gho_options_h: i32,
    /// 「安装选项」内层 WIM 组（5 行）
    pub wim_opt_h: i32,
    /// 「安装选项」内层 GHO 组（1 行）
    pub gho_opt_h: i32,

    // ---------- 页脚 ----------
    pub footer_h: i32,
    pub foot_row_h: i32,
    pub foot_pad_x: i32,
    pub foot_pad_y: i32,
    pub prog_h: i32,
    pub foot_spacing: i32,

    // ---------- 日志 ----------
    pub log_h: i32,
    pub log_min_h: i32,
    pub log_title_h: i32,
    pub log_pad_x: i32,
    pub log_pad_y: i32,
    pub log_pad_bottom: i32,
    pub log_spacing: i32,
}

impl Layout {
    /// 按当前屏幕选档并计算窗口几何。
    pub fn detect() -> Self {
        let (sw_f, sh_f) = fapp::screen_size();
        let sw = sw_f as i32;
        let sh = sh_f as i32;
        // 测试钩子：`SKS_UI_VIEWPORT=800x600` 可伪造屏幕分辨率，
        // 用于低分辨率布局的自动化视觉验证（正常运行时无影响）。
        let (sw, sh) = match std::env::var("SKS_UI_VIEWPORT") {
            Ok(v) => {
                let mut it = v.split('x');
                let a = it.next().and_then(|s| s.parse().ok()).unwrap_or(sw);
                let b = it.next().and_then(|s| s.parse().ok()).unwrap_or(sh);
                (a, b)
            }
            Err(_) => (sw, sh),
        };

        let normal = Self::normal();
        let compact = Self::compact();

        // 可用客户区：水平留 40px 给边框/滚动条，垂直留 90px 给标题栏 + 任务栏。
        // 800×600 → 760×510；1024×768 → 984×678；1920×1080 → 1880×990。
        let avail_w = (sw - 40).max(240);
        let avail_h = (sh - 90).max(240);

        // 可用区域连**正常档的最小尺寸**都放不下时切紧凑档。
        let mut lay = if avail_w < normal.min_w || avail_h < normal.min_h {
            compact
        } else {
            normal
        };

        // 紧凑档在 800×600 下本来够用；若屏幕比设计下限还小（如 640×480），
        // 只能把窗口最小尺寸压到可用区域，让窗口不超出屏幕（内容允许被裁）。
        lay.min_w = lay.min_w.min(avail_w);
        lay.min_h = lay.min_h.min(avail_h);

        // 窗口尺寸：紧凑档直接吃掉可用区域（小屏寸土寸金），
        // 正常档保留原来的留白比例（宽 -140 / 高 -240）。
        let want_w = if lay.compact { avail_w } else { sw - 140 };
        let want_h = if lay.compact { avail_h } else { sh - 240 };
        lay.win_w = want_w.min(WIN_MAX_W).min(avail_w).max(lay.min_w);
        lay.win_h = want_h.min(WIN_MAX_H).min(avail_h).max(lay.min_h);

        // 居中：避免窗口顶到屏幕下沿或被任务栏遮住
        lay.win_x = ((sw - lay.win_w) / 2).max(0);
        lay.win_y = ((sh - lay.win_h) / 2).max(0);
        lay
    }

    /// 正常档：改造前的所有尺寸，大屏视觉零变化。
    fn normal() -> Self {
        let mut l = Self::base(false);
        l.root_margin = 8;
        l.root_spacing = 8;
        l.header_h = 52;
        l.head_row_h = 26;
        l.head_pad_x = 16;
        l.head_pad_y = 10;
        l.main_pad_x = 14;
        l.main_pad_y = 12;
        l.main_spacing = 24;
        l.left_min = 320;
        l.left_max = 520;
        l.right_min = 401;
        l.right_spacing = 6;
        l.section_pad_x = 10;
        l.section_pad_y = 10;
        l.section_spacing = 5;
        l.title_h = 18;
        l.left_title_h = 20;
        l.row_h = 26;
        l.preview_h = 24;
        l.card_slack = 6;
        l.gho_slack = 9;
        // 30 → 44 → 52：「当前方案」完整路径最多三行；字号 10 → 13 后
        // 3 行 ×（13 + 行距 3）≈ 48px，页脚行同步加高避免文字被裁。
        l.foot_row_h = 52;
        l.foot_pad_x = 16;
        l.foot_pad_y = 10;
        l.prog_h = 10;
        l.foot_spacing = 8;
        l.log_h = 168;
        l.log_min_h = 80;
        l.log_title_h = 18;
        l.log_pad_x = 12;
        l.log_pad_y = 8;
        l.log_pad_bottom = 10;
        l.log_spacing = 4;
        l.recalc();
        l
    }

    /// 紧凑档：为 800×600 设计。
    ///
    /// 内容高度 506px、最小宽度 714px —— 800×600 的可用客户区是 760×510。
    fn compact() -> Self {
        let mut l = Self::base(true);
        l.root_margin = 6;
        l.root_spacing = 6;
        l.header_h = 34;
        l.head_row_h = 22;
        l.head_pad_x = 12;
        l.head_pad_y = 6;
        l.main_pad_x = 8;
        l.main_pad_y = 6;
        l.main_spacing = 12;
        l.left_min = 260;
        l.left_max = 360;
        l.right_min = 401;
        l.right_spacing = 4;
        l.section_pad_x = 8;
        l.section_pad_y = 8;
        l.section_spacing = 4;
        l.title_h = 18;
        l.left_title_h = 18;
        l.row_h = 24;
        l.preview_h = 22;
        l.card_slack = 2;
        l.gho_slack = 3;
        // 26 → 40 → 50：同上，字号 13 的三行路径需要 ≈48px。
        // 页脚行加高 10px，同时收一点上下留白 / 进度条 / 间距（-8px），
        // 净增 2px：min_content_h 504 → 506，仍留 4px 余量给 800×600 的 510。
        l.foot_row_h = 50;
        l.foot_pad_x = 12;
        l.foot_pad_y = 4;
        l.prog_h = 8;
        l.foot_spacing = 4;
        l.log_h = 132;
        l.log_min_h = 60;
        l.log_title_h = 18;
        l.log_pad_x = 10;
        l.log_pad_y = 6;
        l.log_pad_bottom = 8;
        l.log_spacing = 4;
        l.recalc();
        l
    }

    fn base(compact: bool) -> Self {
        Self {
            compact,
            win_x: 0,
            win_y: 0,
            win_w: 0,
            win_h: 0,
            min_w: 0,
            min_h: 0,
            root_margin: 0,
            root_spacing: 0,
            header_h: 0,
            head_row_h: 0,
            head_pad_x: 0,
            head_pad_y: 0,
            main_pad_x: 0,
            main_pad_y: 0,
            main_spacing: 0,
            left_min: 0,
            left_max: 0,
            right_min: 0,
            right_spacing: 0,
            section_pad_x: 0,
            section_pad_y: 0,
            section_spacing: 0,
            title_h: 0,
            left_title_h: 0,
            row_h: 0,
            preview_h: 0,
            card_slack: 0,
            gho_slack: 0,
            source_h: 0,
            target_h: 0,
            gho_h: 0,
            options_h: 0,
            gho_options_h: 0,
            wim_opt_h: 0,
            gho_opt_h: 0,
            footer_h: 0,
            foot_row_h: 0,
            foot_pad_x: 0,
            foot_pad_y: 0,
            prog_h: 0,
            foot_spacing: 0,
            log_h: 0,
            log_min_h: 0,
            log_title_h: 0,
            log_pad_x: 0,
            log_pad_y: 0,
            log_pad_bottom: 0,
            log_spacing: 0,
        }
    }

    /// 由基础度量推导游生尺寸（卡片定高、页脚高、窗口最小尺寸）。
    ///
    /// 所有数字都是「内边距 ×2 + 标题 + 间距 + 行高 ×N」，与 `app.rs` 里
    /// 创建这些组时的 `fixed()` 调用一一对应；此处集中推导，避免改一处漏一处
    /// （历史上卡片被压扁/裁切就是这么来的）。
    fn recalc(&mut self) {
        let pad_v = self.section_pad_y * 2;
        let sp = self.section_spacing;
        let row = self.row_h;

        // 安装源：标题 + 2 行（路径 / 镜像）
        self.source_h = pad_v + self.title_h + sp + row * 2 + sp + self.card_slack;
        // 目标位置：标题 + 硬盘行 + 分区预览行
        self.target_h = pad_v + self.title_h + sp + row + sp + self.preview_h + self.card_slack;
        // GHO 选项：标题 + 3 行（还原模式 / 目标硬盘 / 目标分区 / Ghost 路径 互斥）
        self.gho_h = pad_v + self.title_h + sp + row * 3 + sp * 2 + self.gho_slack;
        // WIM 选项内层：5 行（格式化/引导/重启、Defender/防火墙、复制工具、
        // 自动部署+用户名、进系统后执行）
        self.wim_opt_h = row * 5 + sp * 4;
        // GHO 选项内层：1 行
        self.gho_opt_h = row;
        self.options_h = pad_v + self.title_h + sp + self.wim_opt_h + self.card_slack;
        self.gho_options_h = pad_v + self.title_h + sp + self.gho_opt_h + self.gho_slack;

        // 页脚：上下内边距 + 进度条 + 间距 + 按钮行
        self.footer_h = self.foot_pad_y * 2 + self.prog_h + self.foot_spacing + self.foot_row_h;

        // 右列宽度预留：右列最小宽 + 主体左右内边距 + 两处列间距 + 分隔线 + 根边距
        let reserve = self.left_reserve();

        // 窗口最小尺寸：刚好放下「页头 + 主体 + 页脚」（取两种模式的较大值，
        // 避免切换 GHO 后窗口最小高度忽然不够）
        let min_content = self.min_content_h(false).max(self.min_content_h(true));
        self.min_h = min_content;
        self.min_w = reserve + self.left_min;
    }

    /// 右列之外必须预留的横向像素（含根边距、主体内边距、列间距、分隔线）。
    pub fn left_reserve(&self) -> i32 {
        self.right_min
            + self.main_pad_x * 2
            + self.main_spacing * 2
            + DIVIDER_W
            + self.root_margin * 2
    }

    /// 主体（左列 + 右列）的最小高度，含主体自身上下内边距。
    pub fn min_main(&self, gho_mode: bool) -> i32 {
        let right = if gho_mode {
            self.source_h + self.gho_h + self.gho_options_h + self.right_spacing * 2
        } else {
            self.source_h + self.target_h + self.options_h + self.right_spacing * 2
        };
        right + self.main_pad_y * 2
    }

    /// 整窗内容的最小高度：根边距 + 页头 + 主体 + 页脚 + 两处根间距。
    pub fn min_content_h(&self, gho_mode: bool) -> i32 {
        self.root_margin * 2
            + self.header_h
            + self.footer_h
            + self.root_spacing * 2
            + self.min_main(gho_mode)
    }

    /// 按窗口宽度算左列宽度：先保证右列拿到 `right_min`，余量都给磁盘树。
    pub fn left_width_for(&self, win_w: i32) -> i32 {
        (win_w - self.left_reserve()).clamp(self.left_min, self.left_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 紧凑档必须能塞进 800×600 屏幕（可用客户区 760×510）—— 这是本模块存在的理由。
    #[test]
    fn compact_fits_800x600() {
        let l = Layout::compact();
        assert!(
            l.min_w <= 760,
            "紧凑档最小宽度 {} 超过 800×600 的可用宽度 760",
            l.min_w
        );
        assert!(
            l.min_h <= 510,
            "紧凑档最小高度 {} 超过 800×600 的可用高度 510",
            l.min_h
        );
        // WIM / GHO 两种模式都要放得下（GHO 卡片更高，但「安装选项」更矮）
        assert!(l.min_content_h(false) <= 510, "WIM 内容高度超限");
        assert!(l.min_content_h(true) <= 510, "GHO 内容高度超限");
    }

    /// 「当前方案」路径字号放大后，两个档位的页脚行都必须放得下三行文字。
    #[test]
    fn scheme_label_fits_footer_row() {
        // FLTK 行高 ≈ 字号 + 3px 行距
        let needed = (SCHEME_LABEL_SIZE + 3) * SCHEME_MAX_LINES as i32;
        for l in [Layout::normal(), Layout::compact()] {
            assert!(
                l.foot_row_h >= needed,
                "foot_row_h {} 放不下 {} 行 {}px 路径文字（需 {}px）",
                l.foot_row_h,
                SCHEME_MAX_LINES,
                SCHEME_LABEL_SIZE,
                needed
            );
        }
    }

    /// 正常档的定高：除「安装选项」因新增「进系统后执行」一行（4→5 行）而
    /// 高出一行外，其余与此前完全一致（大屏观感零变化）。
    #[test]
    fn normal_keeps_legacy_heights() {
        let l = Layout::normal();
        // options_h：168 → 199（+row_h 26 + section_spacing 5）
        assert_eq!((l.source_h, l.target_h, l.options_h), (106, 104, 199));
        assert_eq!(
            (l.gho_h, l.gho_options_h, l.wim_opt_h, l.gho_opt_h),
            (140, 78, 150, 26)
        );
        // footer_h 68 → 82 → 90：页脚行 30 → 44 → 52，
        // 用于「当前方案」完整路径折行（13px 字 ×3 行）
        assert_eq!((l.header_h, l.footer_h, l.log_h), (52, 90, 168));
        assert_eq!((l.left_min, l.left_max, l.right_min), (320, 520, 401));
    }

    /// 窗口最小尺寸 = 预留横向像素 + 左列最小宽度；且恰好能放下内容。
    #[test]
    fn min_size_is_consistent() {
        for l in [Layout::normal(), Layout::compact()] {
            assert_eq!(l.min_w, l.left_reserve() + l.left_min);
            assert_eq!(l.min_h, l.min_content_h(false).max(l.min_content_h(true)));
            // 右列最小宽度必须留在预留量里，否则复选框会被裁切
            assert!(l.left_reserve() >= l.right_min);
        }
    }

    /// 左列宽度随窗口宽度收缩，但不低于下限，且右列始终拿到 right_min。
    #[test]
    fn left_width_shrinks_before_right_column() {
        let l = Layout::compact();
        // 窗口够宽：左列吃满上限
        assert_eq!(l.left_width_for(1200), l.left_max);
        // 窗口最小：左列取下限，右列刚好 right_min
        assert_eq!(l.left_width_for(l.min_w), l.left_min);
        // 右列宽度 = 窗口宽 -（预留量 - right_min）- 左列宽
        let right = l.min_w - (l.left_reserve() - l.right_min) - l.left_width_for(l.min_w);
        assert_eq!(right, l.right_min);
    }
}
