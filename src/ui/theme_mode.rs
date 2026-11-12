//! 主题模式：浅色 / 深色 / 自动（跟随系统）
//!
//! 通过 Windows 注册表判断系统主题：
//! - HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme
//!   0 = 深色主题, 1 = 浅色主题 (读不到时按浅色兜底)

use std::sync::atomic::{AtomicU8, Ordering};

/// 系统主题所在注册表子键（相对 HKEY_CURRENT_USER）。
///
/// 注意：`AppsUseLightTheme` 属于 `Themes\Personalize`，不是
/// `ContentSettings`（后者没有这个值，读它会永远失败而回落成浅色）。
pub const THEME_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

/// 主题值名
pub const THEME_VALUE: &str = "AppsUseLightTheme";

/// 全局主题模式状态：0=Auto, 1=Light, 2=Dark
static THEME_MODE: AtomicU8 = AtomicU8::new(0);

/// 主题模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    /// 浅色主题
    Light,
    /// 深色主题
    Dark,
    /// 自动跟随系统
    Auto,
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl From<u8> for ThemeMode {
    fn from(v: u8) -> Self {
        match v {
            1 => ThemeMode::Light,
            2 => ThemeMode::Dark,
            _ => ThemeMode::Auto,
        }
    }
}

impl ThemeMode {
    /// 获取当前主题模式（从全局状态读取）
    pub fn get() -> ThemeMode {
        ThemeMode::from(THEME_MODE.load(Ordering::Relaxed))
    }

    /// 设置主题模式
    pub fn set(mode: ThemeMode) {
        let val = match mode {
            ThemeMode::Auto => 0u8,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        };
        THEME_MODE.store(val, Ordering::Relaxed);
    }

    /// 检测当前系统主题
    pub fn detect_system() -> Self {
        // 访问注册表 HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme
        match get_registry_value(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            THEME_SUBKEY,
            THEME_VALUE,
        ) {
            Ok(0) => ThemeMode::Dark,
            _ => ThemeMode::Light, // 默认浅色，或读取值非0
        }
    }

    /// 当前有效的主题（在 Auto 模式下返回系统检测结果）
    #[allow(dead_code)]
    pub fn effective(&self) -> Self {
        match self {
            ThemeMode::Auto => Self::detect_system(),
            mode => *mode,
        }
    }

    /// 转换为整数值（用于保存到配置）
    #[allow(dead_code)]
    pub fn to_u8(self) -> u8 {
        match self {
            ThemeMode::Auto => 0,
            ThemeMode::Light => 1,
            ThemeMode::Dark => 2,
        }
    }
}

fn get_registry_value(hkey: windows_sys::Win32::System::Registry::HKEY, subkey: &str, value_name: &str) -> Result<u64, i32> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ, REG_DWORD};

    let subkey_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let value_name_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();

    let mut key_handle: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
    let result = unsafe {
        RegOpenKeyExW(
            hkey,
            subkey_w.as_ptr(),
            0,
            KEY_READ,
            &mut key_handle as *mut _ as *mut _,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(result as i32);
    }

    let mut value_type = 0u32;
    let mut value_size = 0u32;
    // 先查询大小
    let result = unsafe {
        RegQueryValueExW(
            key_handle,
            value_name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut value_size,
        )
    };
    if result != ERROR_SUCCESS {
        unsafe { RegCloseKey(key_handle) };
        return Err(result as i32);
    }

    if value_type != REG_DWORD {
        unsafe { RegCloseKey(key_handle) };
        return Err(0x8007000Du32 as i32); // ERROR_INVALID_DATA
    }

    let mut value: u32 = 0;
    let result = unsafe {
        RegQueryValueExW(
            key_handle,
            value_name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            (&mut value as *mut u32) as *mut _,
            &mut value_size,
        )
    };
    unsafe { RegCloseKey(key_handle) };

    if result != ERROR_SUCCESS {
        return Err(result as i32);
    }

    Ok(value as u64)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 防回归：`AppsUseLightTheme` 只存在于 `Themes\Personalize`。
    ///
    /// 历史 bug：曾读 `CurrentVersion\ContentSettings`，该键/值在本机不存在，
    /// 读取必然失败 → 一直回落成浅色，表现为"跟随系统永远是明亮"。
    #[test]
    fn subkey_is_themes_personalize() {
        assert_eq!(
            THEME_SUBKEY,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        );
        assert!(!THEME_SUBKEY.contains("ContentSettings"));
        assert_eq!(THEME_VALUE, "AppsUseLightTheme");
    }
}
