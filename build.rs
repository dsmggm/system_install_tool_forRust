//! 构建脚本：把图标与版本信息嵌入发布 exe（← `SKS_SysInstallTool.spec` 的
//! `icon=['icon.ico']` + PyInstaller 版本资源，文档 §11.3）。
//!
//! 仅在 Windows 目标生效；其他平台（如 CI 上做 lint）静默跳过。

fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "硕科斯系统安装工具");
        res.set("FileDescription", "硕科斯系统安装工具");
        res.set("OriginalFilename", "sks_installer.exe");
        res.set("CompanyName", "硕科斯");
        res.set("LegalCopyright", "");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "2.0.0".into());
        res.set("FileVersion", &version);
        res.set("ProductVersion", &version);
        if let Err(e) = res.compile() {
            // 缺少 windres/rc.exe 时让构建失败，方便诊断。
            panic!("嵌入 Windows 资源失败: {e}");
        }
    }

    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
