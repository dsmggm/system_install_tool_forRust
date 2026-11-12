//! UI 层（fltk-rs，← gui.py）— 里程碑 6 全量实现。
//!
//! 线程模型：GUI 单线程。后台耗时操作（磁盘枚举 / diskpart / DISM / Ghost）
//! 跑 `std::thread`，结果经 `app::channel` 的 `Message` 回传；主循环用
//! `app::add_check` 周期轮询 `Receiver`（fltk 的 `recv` 为非阻塞 `try_recv`），
//! 取出消息后交给 `App::handle_message` 处理。
//!
//! 全局状态：fltk 回调必须 `'static`，无法借用 `App`。这里把 `App` 放进
//! 线程本地槽 `APP`；`App::with(f)` 先把 `App` 从槽中取出（释放借用），
//! 执行 `f` 后再放回，从而允许回调内嵌套模态对话框（嵌套事件循环）。

mod app;
mod disk_row;
mod ghost;
mod layout;
mod log_panel;
mod msg;
mod one_click;
mod restart;
mod state;
pub mod theme;
pub mod theme_mode;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::error::Error;

use fltk::app::{self as fapp, Sender};
use fltk::image::PngImage;
use fltk::menu::Choice;
use fltk::prelude::*;
use fltk::window::DoubleWindow;

/// 获取应用图标（PNG，编译时嵌入）。
/// 返回 `PngImage`（实现 `ImageExt`），可直接传给 `win.set_icon()`。
pub fn app_icon() -> PngImage {
    static ICON_BYTES: &[u8] = include_bytes!("../../assets/icon.png");
    PngImage::from_data(ICON_BYTES).expect("加载应用图标失败")
}

pub use app::App;
pub use state::Message;

thread_local! {
    /// 全局 `App` 实例槽（仅在 UI 线程访问）。
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// `App::with` 的嵌套排队队列。
    ///
    /// 对话框的模态循环（`one_click::show` 等）在 `App::with` 的回调栈内
    /// 自旋 `app::wait()`，期间其他回调——最典型的是 5s 自动刷新超时——
    /// 会再次进入 `App::with`。若直接执行会出现两个 `&mut App`，因此旧
    /// 实现选择**静默丢弃**；但 `schedule_next_refresh` 只在刷新回调内部
    /// 重新武装，被丢弃一次刷新链就**永久断裂**（对话框关闭后磁盘信息
    /// 再也不自动刷新，与 Python 原版 tkinter 模态期间 `after()` 继续触发
    /// 的行为相悖）。这里把嵌套调用排队，在最外层回调结束、`App` 归还
    /// 之后按 FIFO 依次补执行。
    static PENDING_WITH: RefCell<VecDeque<Box<dyn FnOnce(&mut App)>>> =
        RefCell::new(VecDeque::new());
}

/// 全局消息发送端（← `root.after(0, cb)` 的跨线程投递）。
///
/// fltk 的 `channel<T>` 是进程级单例，重复调用返回指向同一通道的句柄，
/// 因此后台线程可随时 `super::tx().send(...)` 把结果投回主循环。
pub fn tx() -> Sender<Message> {
    Sender::<Message>::get()
}

/// 在 UI 线程上以可变方式访问全局 `App`。
///
/// 取出 → 执行 → 放回。执行期间槽为 `None`；此时若回调重入（对话框
/// 模态循环里触发的刷新超时、消息泵等），不执行也不丢弃，而是把闭包
/// 放进 `PENDING_WITH` 排队，待最外层调用归还 `App` 后按 FIFO 补执行。
/// 这允许 `f` 内运行模态对话框，同时保证刷新链等周期任务不断裂。
impl App {
    pub(crate) fn with<F: FnOnce(&mut App) + 'static>(f: F) {
        APP.with(|slot| {
            let taken = slot.borrow_mut().take();
            if let Some(mut a) = taken {
                f(&mut a);
                *slot.borrow_mut() = Some(a);
                // 排空嵌套期间排队的闭包；补执行本身可能再次嵌套（例如某个
                // 补执行又打开了对话框），新排队项会在后续循环中被取走。
                loop {
                    let next = PENDING_WITH.with(|q| q.borrow_mut().pop_front());
                    let Some(g) = next else { break };
                    let taken = slot.borrow_mut().take();
                    if let Some(mut a) = taken {
                        g(&mut a);
                        *slot.borrow_mut() = Some(a);
                    } else {
                        // 不可达的保险分支：App 已空却仍无主——原样塞回队列，
                        // 留给下一次 with 的排空循环。
                        PENDING_WITH.with(|q| q.borrow_mut().push_front(g));
                        break;
                    }
                }
            } else {
                // 嵌套进入：App 正被外层（模态循环）持有，排队等待补执行。
                PENDING_WITH.with(|q| q.borrow_mut().push_back(Box::new(f)));
            }
        });
    }

    /// 把 `self` 注册进全局槽（清除旧值）。
    fn register_global(self) {
        APP.with(|slot| {
            *slot.borrow_mut() = Some(self);
        });
    }
}

/// 按文本选中 `Choice` 中的条目。
///
/// 对齐 tkinter `ttk.Combobox.set()`：若 `text` 已在列表中则选中它，
/// 否则追加该条目后再选中（与 Python 原版 `self.image_var.set(idx_text)` 行为一致）。
///
/// 注意 FLTK 1.5 的 `Fl_Choice` 不能有空字符串条目（会被当作菜单终结符丢弃），
/// 调用方传入空串时直接忽略。
pub fn set_choice_by_text(c: &mut Choice, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let count = c.size() as i32 - 1; // size() 含结尾的 NULL 项，真实条目数 = size() - 1
    if count > 0 {
        for i in 0..count {
            if c.text(i).as_deref() == Some(text) {
                return c.set_value(i);
            }
        }
    }
    let idx = c.add_choice(text);
    c.set_value(idx)
}

/// 仅当条目**已存在**时选中，绝不新增。
/// 用于刷新目标下拉框：旧选中项指向的分区可能已被清理/重建，
/// 若按 `set_choice_by_text` 的兜底把它 add 回来，选中后解析出失效
/// 目标（如分区号 0）会直接传给 Ghost。
pub fn select_choice_exact(c: &mut Choice, text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let count = c.size() as i32 - 1; // size() 含结尾的 NULL 项
    for i in 0..count {
        if c.text(i).as_deref() == Some(text) {
            return c.set_value(i);
        }
    }
    false
}

/// 计算弹出对话框的尺寸与位置（低分辨率适配）。
///
/// - 【位置恒为居中】以**屏幕可用区域**（整屏扣除任务栏约 40px）为中心，
///   不再跟随父窗口位置，也绝不记忆/复用上一次坐标；
/// - 设计尺寸超出屏幕可用区域时自动收缩，但不小于 `min_*`；
/// - 坐标再夹紧到屏幕内，避免标题栏/任务栏跑到屏幕外。
///
/// 返回 `(x, y, w, h)`，供 `DoubleWindow::new` 直接使用。
pub fn dialog_geometry(
    _parent: &DoubleWindow,
    desired_w: i32,
    desired_h: i32,
    min_w: i32,
    min_h: i32,
) -> (i32, i32, i32, i32) {
    let (sw, sh) = fapp::screen_size();
    let sw = sw as i32;
    let sh = sh as i32;
    // 预留窗口边框与任务栏空间：水平 24px，垂直 48px。
    let avail_w = (sw - 24).max(160);
    let avail_h = (sh - 48).max(120);
    let w = desired_w.min(avail_w).max(min_w.min(avail_w));
    let h = desired_h.min(avail_h).max(min_h.min(avail_h));

    let x = ((sw - w) / 2).clamp(0, (avail_w - w).max(0));
    let y = ((sh - 40 - h) / 2).clamp(0, (avail_h - h).max(0));
    (x, y, w, h)
}

/// 启动主窗口（阻塞至关闭）。
pub fn run(config: crate::config::settings::AppConfig) -> Result<(), Box<dyn Error>> {
    // 日志面板 sink 需在创建 App 之前注册（启动早期日志才进面板）。
    log_panel::install_sink();

    let start = App::new(config);
    start.register_global();

    // 【主题只在启动时自动选择】按用户要求不再常驻检测系统主题变化：
    // 启动时按系统深浅色定一次（App::new → theme::init / Auto → 检测），
    // 之后由用户手动在「明亮/深色」间切换，运行中不再自动跟改。

    // 消息泵：非阻塞轮询 channel，再排空日志面板队列；顺带检测树宽变化
    // （窗口缩放后硬盘行内按钮重新右对齐）。
    fapp::add_check(|_| {
        let rx = fapp::Receiver::<Message>::get();
        while let Some(msg) = rx.recv() {
            App::with(|a| a.handle_message(msg));
        }
        App::with(|a| {
            a.drain_log();
        });
    });

    let flapp = fapp::App::default();
    while flapp.wait() {}

    // 清理主题检查定时器
    Ok(())
}
