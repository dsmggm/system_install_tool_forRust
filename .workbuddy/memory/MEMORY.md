# 项目约定（system_install_tool_forRust / sks_installer）

## 工作流铁律

- **每次修改代码后都要「发布」**：用户明确要求「每次修完之后，都进行发布」。
  发布 = `cargo build --release`，产物为 `target/release/sks_installer.exe`，
  构建完成后核对产物时间戳晚于改动源码，并告知用户产物路径与大小。
- 发布前必须 `cargo test` 通过；改 UI 布局时还要用
  `SKS_UI_VIEWPORT=1366x768 / 800x600` 启动 + `PrintWindow` 截图复核，两种档位都不能裁切。
- 不修改用户未要求的配置项语义；`config.ini` 为全局配置，方案文件（bat）只存布尔开关。

## 技术要点

- Rust + fltk 1.4（fltk-bundled），Windows 专用；release profile：opt-level=3 / lto=thin / strip。
- `assets/unattend.xml` 为模板，部署时内存渲染替换占位符（`%username%`、`%disable_firwall%`、`%run_after%` 等），
  再写入 `<Windows>\panther\unattend.xml` 与 `System32\sysprep\panther`。
- `%run_after%` 位于 `C:\Windows\Setup\Scripts\unattend-01.cmd`，由 FirstLogon.ps1 调用；
  勾选后替换为 `if exist "<path>" start "" /wait "<path>"`（`start` 的空标题参数不可省）。
- 「进系统后执行」等镜像相关选项只在加载 install.wim / install.esd / *.iso 时可勾选，
  统一由 `update_image_dependent_options` 控制启停。
- 布局尺寸集中在 `src/ui/layout.rs`，`normal_keeps_legacy_heights` 等断言固定正常档高度，
  改行数需同步更新断言；紧凑档内容高度须 ≤510px（800×600 可用）。
