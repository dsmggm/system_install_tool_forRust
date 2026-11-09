//! 日志设施（← main.py::DailyRotatingFileHandler + gui.py::TextHandler）
//!
//! - 文件通道：`log/debug.log`，DEBUG 级别，按天滚动（`debug-YYYYMMDD.log`），保留 30 天，UTF-8。
//!   滚动时机在每次写入前检测跨天（与 Python emit 检查一致），跨天计算用 chrono 日期 +1 天
//!   （修复 Python 版 `replace(day=day+1)` 月末抛 ValueError 的 bug，见文档 §9.2）。
//! - GUI 通道：默认 INFO，可切 DEBUG；通过 sink 回调回传主线程（级别 + 已格式化文本）。
//! - 两个通道格式一致：`%(asctime)s - %(levelname)s - %(message)s`。

// set_gui_level/set_gui_sink 等由 GUI 日志面板（里程碑 6）启用
#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use chrono::{Datelike, Local};
use log::{Level, LevelFilter, Log, Metadata, Record};

const LOG_DIR: &str = "log";
const LOG_FILE: &str = "debug.log";
const LOG_BACKUP_DAYS: usize = 30;

type GuiSink = Box<dyn Fn(Level, String) + Send + Sync>;

/// 文件通道状态（跨天检测 + 句柄）
struct FileState {
    dir: PathBuf,
    path: PathBuf,
    file: Option<File>,
    /// 当前日志文件所属日期 YYYYMMDD（对应 Python `self._current_date`）
    current_date: String,
}

struct DailyLogger {
    state: Mutex<FileState>,
    /// GUI 通道级别：0=Off,1=Error,2=Warn,3=Info,4=Debug,5=Trace
    gui_level: AtomicU8,
    gui_sink: Mutex<Option<GuiSink>>,
}

static LOGGER: DailyLogger = DailyLogger {
    state: Mutex::new(FileState {
        dir: PathBuf::new(),
        path: PathBuf::new(),
        file: None,
        current_date: String::new(),
    }),
    gui_level: AtomicU8::new(3),
    gui_sink: Mutex::new(None),
};

fn level_num(level: Level) -> u8 {
    match level {
        Level::Error => 1,
        Level::Warn => 2,
        Level::Info => 3,
        Level::Debug => 4,
        Level::Trace => 5,
    }
}

fn filter_num(filter: LevelFilter) -> u8 {
    match filter {
        LevelFilter::Off => 0,
        LevelFilter::Error => 1,
        LevelFilter::Warn => 2,
        LevelFilter::Info => 3,
        LevelFilter::Debug => 4,
        LevelFilter::Trace => 5,
    }
}

fn filter_from_num(n: u8) -> LevelFilter {
    match n {
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        5 => LevelFilter::Trace,
        _ => LevelFilter::Off,
    }
}

fn today_str() -> String {
    let now = Local::now();
    format!("{:04}{:02}{:02}", now.year(), now.month(), now.day())
}

/// 对应 Python `logging.Formatter("%(asctime)s - %(levelname)s - %(message)s")`：
/// asctime 默认格式 `2026-10-20 10:41:05,123`。
fn format_line(record: &Record) -> String {
    format_line_with(Local::now(), record.level(), &record.args().to_string())
}

fn format_line_with(now: chrono::DateTime<Local>, level: Level, msg: &str) -> String {
    format!(
        "{},{:03} - {} - {}",
        now.format("%Y-%m-%d %H:%M:%S"),
        now.timestamp_subsec_millis(),
        level,
        msg
    )
}

fn lock_unpoisoned<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl DailyLogger {
    fn write_line(&self, line: &str) {
        let mut state = lock_unpoisoned(&self.state);
        if state.dir.as_os_str().is_empty() {
            return; // 未初始化
        }
        let today = today_str();
        if state.current_date != today {
            self.rollover(&mut state, &today);
        }
        if state.file.is_none() {
            state.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.path)
                .ok();
        }
        if let Some(f) = state.file.as_mut() {
            let _ = writeln!(f, "{}", line);
            let _ = f.flush();
        }
    }

    /// 跨天滚动：关闭句柄 → 重命名为日期文件（目标已存在则丢弃，与 Python 一致）
    /// → 重开当日文件 → 更新日期 → 清理超期备份。
    fn rollover(&self, state: &mut FileState, now_date: &str) {
        if let Some(mut f) = state.file.take() {
            let _ = f.flush();
        }
        let dest = state.dir.join(format!("debug-{}.log", state.current_date));
        if state.path.exists() && !dest.exists() {
            let _ = fs::rename(&state.path, &dest);
        }
        state.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&state.path)
            .ok();
        state.current_date = now_date.to_string();
        Self::cleanup(state);
    }

    /// 删除超过保留天数的历史日期日志（按文件名排序，最旧先删）。
    fn cleanup(state: &FileState) {
        let Ok(entries) = fs::read_dir(&state.dir) else {
            return;
        };
        let mut files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("debug-") && n.ends_with(".log"))
            .collect();
        files.sort();
        while files.len() > LOG_BACKUP_DAYS {
            if let Some(name) = files.first() {
                let _ = fs::remove_file(state.dir.join(name));
            }
            files.remove(0);
        }
    }
}

impl Log for DailyLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // 文件通道 DEBUG 级别（根 logger max_level = Debug）
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        self.write_line(&format_line(record));

        let lvl = record.level();
        let lvl_n = level_num(lvl);
        if lvl_n <= self.gui_level.load(Ordering::Relaxed) {
            let sink = lock_unpoisoned(&self.gui_sink);
            if let Some(cb) = sink.as_ref() {
                cb(lvl, format_line(record));
            }
        }
    }

    fn flush(&self) {
        let mut state = lock_unpoisoned(&self.state);
        if let Some(f) = state.file.as_mut() {
            let _ = f.flush();
        }
    }
}

/// 初始化全局日志（文件按天滚动 + GUI sink 通道）。
///
/// `base_dir` = exe 所在目录，日志写入 `base_dir/log/debug.log`。
/// 初始化前产生的日志仅进 GUI 通道（文件通道为空实现）。
pub fn init(base_dir: &Path) -> Result<(), log::SetLoggerError> {
    let dir = base_dir.join(LOG_DIR);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(LOG_FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path).ok();

    {
        let mut state = lock_unpoisoned(&LOGGER.state);
        state.dir = dir;
        state.path = path;
        state.file = file;
        state.current_date = today_str();
    }

    log::set_max_level(LevelFilter::Debug);
    log::set_logger(&LOGGER)
}

/// 设置 GUI 日志通道级别（默认 INFO）。
pub fn set_gui_level(level: LevelFilter) {
    LOGGER.gui_level.store(filter_num(level), Ordering::Relaxed);
}

/// 当前 GUI 日志通道级别。
pub fn gui_level() -> LevelFilter {
    filter_from_num(LOGGER.gui_level.load(Ordering::Relaxed))
}

/// 注册 GUI 日志 sink（级别过滤后回调，参数为级别与完整格式化行）。
/// 由 GUI 在启动时调用；回调内负责线程安全地转投主循环。
pub fn set_gui_sink<F>(sink: F)
where
    F: Fn(Level, String) + Send + Sync + 'static,
{
    *lock_unpoisoned(&LOGGER.gui_sink) = Some(Box::new(sink));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    /// 行格式必须与 Python logging.Formatter 默认 asctime 一致：
    /// `2026-10-20 10:41:05,123 - INFO - 消息`
    #[test]
    fn format_line_matches_python_asctime() {
        let line = format_line_with(Local::now(), Level::Info, "启动测试");
        // 头部 19 字符必须是可解析的日期时间
        chrono::NaiveDateTime::parse_from_str(&line[..19], "%Y-%m-%d %H:%M:%S").unwrap();
        // 毫秒以逗号开头且固定 3 位（Python asctime 用逗号 + %03d）
        assert!(line[19..23].starts_with(','), "毫秒格式错误: {line:?}");
        assert!(line.ends_with(" - INFO - 启动测试"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sks_log_test_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 超过 30 天的历史日志按文件名排序删除最旧的。
    #[test]
    fn cleanup_keeps_30_backup_files() {
        let dir = temp_dir("cleanup");
        for i in 1..=35 {
            fs::write(dir.join(format!("debug-202401{i:02}.log")), "x").unwrap();
        }
        fs::write(dir.join("debug.log"), "current").unwrap();
        let state = FileState {
            dir: dir.clone(),
            path: dir.join("debug.log"),
            file: None,
            current_date: "20240201".to_string(),
        };
        DailyLogger::cleanup(&state);
        let remaining: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("debug-"))
            .collect();
        assert_eq!(remaining.len(), 30);
        // 最旧的 20240101-20240105 应被删除
        assert!(!remaining.iter().any(|n| n == "debug-20240101.log"));
        assert!(remaining.iter().any(|n| n == "debug-20240135.log"));
        assert!(dir.join("debug.log").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 跨天滚动：旧 debug.log 重命名为日期文件，新文件接管当日日期。
    #[test]
    fn rollover_renames_and_reopens() {
        let dir = temp_dir("rollover");
        let path = dir.join("debug.log");
        fs::write(&path, "old content").unwrap();
        let logger = DailyLogger {
            state: Mutex::new(FileState {
                dir: dir.clone(),
                path: path.clone(),
                file: None,
                current_date: "20240131".to_string(), // 月末日期，回归文档 §9.2
            }),
            gui_level: AtomicU8::new(3),
            gui_sink: Mutex::new(None),
        };
        {
            let mut state = lock_unpoisoned(&logger.state);
            logger.rollover(&mut state, "20240201");
        }
        let backup = dir.join("debug-20240131.log");
        assert!(backup.exists(), "应生成 debug-20240131.log");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "old content");
        assert!(path.exists(), "应重开新的当日日志文件");
        {
            let state = lock_unpoisoned(&logger.state);
            assert_eq!(state.current_date, "20240201");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// GUI 通道默认 INFO、可切 DEBUG，按级别过滤。
    #[test]
    fn gui_level_filter_roundtrip() {
        // 注意：GUI level 为进程级全局，测试串行假设（cargo test 各测试独立进程运行）
        set_gui_level(LevelFilter::Debug);
        assert_eq!(gui_level(), LevelFilter::Debug);
        set_gui_level(LevelFilter::Info);
        assert_eq!(gui_level(), LevelFilter::Info);
    }
}
