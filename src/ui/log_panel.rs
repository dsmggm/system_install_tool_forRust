//! GUI 日志面板（← `gui.py::TextHandler` + `_create_log_area`）
//!
//! 颜色方案（仿 VS Code 深色主题，与 Python 一致）：
//!   DEBUG #569cd6 / INFO #d4d4d4 / WARNING #dcdcaa / ERROR #f44747 / CRITICAL #f44747 粗体
//!
//! 实现：`logging::set_gui_sink` 在任意线程被调用，回调把 (级别, 文本)
//! 推入本模块的全局队列并 `awake()` 唤醒主循环；主循环 `drain()` 取出后
//! 追加到 TextDisplay（样式缓冲同步追加，逐字节着色）。

use std::collections::VecDeque;
use std::sync::Mutex;

use fltk::enums::Font;
use fltk::prelude::*;
use fltk::text::{StyleTableEntry, TextBuffer, TextDisplay};
use log::Level;

use crate::utils::logging;
use super::theme;

/// 一个待写入的日志行（级别 → 样式索引）
struct QueuedLine {
    style: usize,
    text: String,
}

/// 样式表索引：0=info, 1=debug, 2=warning, 3=error
const STYLE_INFO: usize = 0;
const STYLE_DEBUG: usize = 1;
const STYLE_WARNING: usize = 2;
const STYLE_ERROR: usize = 3;

static QUEUE: Mutex<VecDeque<QueuedLine>> = Mutex::new(VecDeque::new());

fn style_of(level: Level) -> usize {
    match level {
        Level::Debug | Level::Trace => STYLE_DEBUG,
        Level::Info => STYLE_INFO,
        Level::Warn => STYLE_WARNING,
        Level::Error => STYLE_ERROR,
    }
}

/// 日志面板持有者：TextBuffer + Display + 样式缓冲。
pub struct LogPanel {
    pub display: TextDisplay,
    text_buf: TextBuffer,
    style_buf: TextBuffer,
    visible: bool,
}

impl LogPanel {
    /// 创建一个指定大小的日志面板（初始隐藏，由 App 决定 pack 时机）。
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        let mut text_buf = TextBuffer::default();
        text_buf.set_tab_distance(4);
        let style_buf = TextBuffer::default();

        let mut display = TextDisplay::new(x, y, w, h, None);
        display.set_buffer(text_buf.clone());
        display.set_text_font(Font::Courier);
        display.set_text_size(12);
        display.set_text_color(theme::rgb(theme::TEXT()));
        display.set_color(theme::rgb(theme::INPUT_BG()));

        let styles = vec![
            StyleTableEntry {
                color: theme::rgb(theme::TEXT()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::PRIMARY()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::WARN()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::DANGER()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::DANGER()),
                font: Font::CourierBold,
                size: 12,
            },
        ];
        // 注册样式表（按 style_buf 中每个字节 = 'A'+index 着色）
        display.set_highlight_data(style_buf.clone(), styles);
        display.show_cursor(false);

        Self {
            display,
            text_buf,
            style_buf,
            visible: false,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }

    /// 把队列中的日志行追加到面板（主循环调用）。
    pub fn drain(&mut self) {
        let mut lines: Vec<QueuedLine> = {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            q.drain(..).collect()
        };
        if lines.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut style = String::new();
        for line in lines.drain(..) {
            text.push_str(&line.text);
            text.push('\n');
            let ch = (b'A' + line.style as u8) as char;
            for _ in 0..line.text.len() {
                style.push(ch);
            }
            style.push(ch); // '\n' 也占一个样式字节
        }
        self.text_buf.append(&text);
        self.style_buf.append(&style);
        // 自动滚动到底部（对应 Python `_append` 的 yview_moveto(1.0)）
        if self.visible {
            let n = self.text_buf.count_lines(0, self.text_buf.length());
            self.display.scroll(n, 0);
        }
        self.display.redraw();
    }

    /// 显示日志时定位到底部（← `_toggle_log_visible` 的 `_scroll_to_bottom`）
    pub fn scroll_to_bottom(&mut self) {
        let n = self.text_buf.count_lines(0, self.text_buf.length());
        self.display.scroll(n, 0);
        self.display.redraw();
    }

    /// 主题切换后刷新日志面板的背景色与样式表颜色。
    pub fn refresh_theme(&mut self) {
        self.display.set_color(theme::rgb(theme::INPUT_BG()));
        self.display.set_text_color(theme::rgb(theme::TEXT()));
        let styles = vec![
            StyleTableEntry {
                color: theme::rgb(theme::TEXT()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::PRIMARY()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::WARN()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::DANGER()),
                font: Font::Courier,
                size: 12,
            },
            StyleTableEntry {
                color: theme::rgb(theme::DANGER()),
                font: Font::CourierBold,
                size: 12,
            },
        ];
        self.display.set_highlight_data(self.style_buf.clone(), styles);
        self.display.redraw();
    }
}

/// 注册 GUI 日志 sink：推入队列并唤醒主循环。
/// `logging::init` 之后调用一次。
pub fn install_sink() {
    logging::set_gui_sink(|level, text| {
        {
            let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(QueuedLine {
                style: style_of(level),
                text,
            });
        }
        fltk::app::awake();
    });
}
