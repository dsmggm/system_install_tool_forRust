//! 主题模式：浅色 / 深色 / 自动（跟随系统）
//!
//! 通过 Windows 注册表判断系统主题：
//! - HKCU\Software\Microsoft\Windows\CurrentVersion\ContentSettings\AppsUseLightTheme
//!   0 = 深色主题, 1 = 浅色主题 (默认)

use std::sync::atomic::{AtomicU8, Ordering};

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
        // 访问注册表 HKCU\Software\Microsoft\Windows\CurrentVersion\ContentSettings\AppsUseLightTheme
        match get_registry_value(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\ContentSettings",
            "AppsUseLightTheme",
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