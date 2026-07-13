<div align="center">

# Window Sharing Hider

Windows 窗口捕获排除管理工具

[![Rust](https://img.shields.io/badge/Rust-1.97-DE6B35?style=flat-square&logo=rust&logoColor=white)](Cargo.toml)
[![Windows](https://img.shields.io/badge/Windows-10%202004%2B-0078D4?style=flat-square&logo=windows11&logoColor=white)](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
[![Slint](https://img.shields.io/badge/Slint-1.16-2379F4?style=flat-square)](https://slint.dev/)
[![License](https://img.shields.io/badge/License-MIT-2EA44F?style=flat-square)](LICENSE)

</div>

## 项目介绍

Window Sharing Hider 用于管理 Windows 桌面窗口的捕获排除状态。用户可以在窗口列表中选择目标，并通过 `WDA_EXCLUDEFROMCAPTURE` 控制该窗口是否出现在受支持的会议共享或屏幕捕获画面中。

应用会回读窗口当前的 display affinity，界面显示的是系统实际状态，而不是仅记录开关操作结果。对于不属于当前进程的窗口，应用会在目标进程内调用 `SetWindowDisplayAffinity`，并在操作完成后释放相关句柄与远程内存。

## 功能

- 枚举当前桌面的可选窗口。
- 显示窗口标题、进程名称、窗口类名和原生进程图标。
- 设置或移除 `WDA_EXCLUDEFROMCAPTURE`。
- 回读并显示窗口的实际保护状态。
- 支持 x64 和 WOW64 目标进程。
- 可选显示系统窗口与输入法窗口。
- 识别并持续跟踪微信输入法候选框。
- 窗口重建后根据已保存策略重新应用保护。
- 扫描与 affinity 操作在独立后台线程执行。
- 对窗口退出、权限不足和操作超时进行隔离处理。

## 系统要求

| 项目 | 要求 |
| --- | --- |
| 操作系统 | Windows 10 Version 2004 或更新版本 |
| Rust | 1.97 或更新版本 |
| 工具链 | `x86_64-pc-windows-msvc` |
| 构建环境 | MSVC Build Tools、Windows SDK |

`WDA_EXCLUDEFROMCAPTURE` 从 Windows 10 Version 2004 开始提供。高完整性进程或受保护进程可能拒绝访问。

## 构建

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo build --release
```

生成文件：

```text
target\release\window-sharing-hider.exe
```

直接运行开发版本：

```powershell
cargo run
```

构建固定深色主题：

```powershell
$env:SLINT_STYLE = "fluent-dark"
cargo build --release
Remove-Item Env:SLINT_STYLE
```

## 使用

1. 启动应用并等待窗口列表刷新。
2. 找到需要排除的窗口。
3. 打开窗口行右侧的保护开关。
4. 确认状态变为“已保护”。
5. 需要重新枚举窗口时，点击“刷新列表”。

“显示系统与输入法窗口”用于列出普通模式下隐藏的系统宿主和输入法窗口。“微信输入法候选框”开关用于持续发现候选框，并在候选窗口重建后重新应用保护。

## 项目结构

```text
WindowSharingHider/
├── Cargo.toml
├── Cargo.lock
├── build.rs
├── ui/
│   └── app.slint
└── src/
    ├── main.rs
    ├── app.rs
    ├── model.rs
    ├── policy.rs
    ├── worker.rs
    └── platform/
        ├── mod.rs
        └── windows/
            ├── affinity.rs
            ├── enumeration.rs
            ├── icons.rs
            ├── remote.rs
            └── resources.rs
```

### 模块职责

| 模块 | 职责 |
| --- | --- |
| `ui/app.slint` | 窗口列表、策略开关和状态界面 |
| `src/app.rs` | 应用状态、扫描结果合并和 UI 视图模型 |
| `src/model.rs` | 窗口标识、affinity、保护状态和图标模型 |
| `src/policy.rs` | 窗口分类、过滤和自动重试策略 |
| `src/worker.rs` | 扫描 Worker 与 affinity Worker |
| `enumeration.rs` | Win32 窗口枚举和进程信息读取 |
| `affinity.rs` | display affinity 设置与状态验证 |
| `remote.rs` | PE 导出解析和 x86/x64 远程调用 |
| `icons.rs` | Windows Shell 图标提取与有界缓存 |
| `resources.rs` | 进程句柄、线程句柄和远程内存的 RAII 管理 |

## 实现说明

窗口以 `HWND + PID` 组成的 `WindowKey` 标识，避免窗口句柄被系统复用后继承旧状态。扫描结果带有 generation 编号，过期结果不会覆盖新状态。affinity 请求使用 request ID 区分，迟到的操作结果不会回滚最新操作。

远程调用根据目标进程架构选择 x86 或 x64 stub，通过目标进程模块的 PE 导出表解析函数地址。进程图标在扫描线程中提取，并以可执行文件路径为键写入有界缓存；UI 线程只负责将 RGBA 像素转换为 Slint `Image`。

## 捕获限制

`SetWindowDisplayAffinity` 只对遵循 Windows display affinity 的捕获方式生效。不同版本的会议软件或录屏软件可能使用不同捕获后端，应在实际使用环境中验证。

该机制不是内容加密，也不能阻止摄像机拍摄、驱动级采集或忽略 display affinity 的捕获程序。

## 验证

```powershell
cargo fmt --all --check
cargo test
cargo build --release
```

## 许可证

[MIT License](LICENSE)
