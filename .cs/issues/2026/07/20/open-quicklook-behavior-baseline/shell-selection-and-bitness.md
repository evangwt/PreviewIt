# 前台文件窗口如何产生当前选择

## 读者先带走什么

QuickLook 先按前台窗口类型选择专用适配器，再通过 Shell COM、窗口消息、剪贴板或第三方 IPC 取得第一个选择。稳定基线 `4.5.0` 在 x64 Windows 上让 AnyCPU/WPF 主程序以 x64 进程运行；x86 产物不是另一套主程序，而是读取 32 位传统文件对话框选择所需的 Native DLL 和 WoW64 helper。`4.5.0` 尚无 ARM64 构建；后续 `master` 虽加入 ARM64 编译配置，但部分重型插件仍主动移除 ARM64 原生依赖。

## 主路径

窗口管理器需要路径时调用托管 NativeMethods。`4.5.0` 的托管层根据当前进程架构选择 `QuickLook.Native32.dll` 或 `QuickLook.Native64.dll`，并在独立 STA 线程上调用原生选择函数。本机 `4.1.1` 的 AnyCPU 程序也已通过 `IsWow64Process2` 确认为原生 x64 进程，而不是 WoW64 进程。

原生层先读取前台窗口类和焦点状态，区分桌面、Explorer、文件对话框、Everything、Directory Opus、MultiCommander、Internet Download Manager、FilePilot 和 DeskBox。随后按类型进入专用适配器。

Explorer 和桌面主路径使用 `IShellWindows`、`IShellBrowser`、`IShellView` 和 `IDataObject` 获取选择。普通文件先从 `CF_HDROP` 取得路径；长路径或 Shell Namespace 对象回退到 `CFSTR_SHELLIDLIST` 和 `IShellItem`。其他文件管理器可能使用窗口消息、隐藏窗口、剪贴板或应用自有 IPC。

传统文件对话框是目前唯一有直接证据要求匹配位数的分支。x64 Native 初始化时启动 Win32 `WoW64HookHelper`；目标对话框若是 32 位 WoW64 进程，x64 侧把请求交给 helper，由 helper 加载 `QuickLook.Native32.dll`、把钩子装入目标线程，再通过共享内存把选择返回。目标与 QuickLook 位数相同时，当前 Native DLL 直接安装钩子。Explorer/桌面走 Shell COM，Directory Opus 和 MultiCommander 走 `WM_COPYDATA`，Everything 走窗口文本或剪贴板；这些源码路径没有调用 WoW64 helper。

## 关键责任、数据和状态

- 前台窗口分类负责防止在搜索框、文本输入或不支持窗口中吞掉空格。
- Selection Adapter 只返回当前第一个选择，不负责插件匹配。
- 托管层负责当前进程架构的 DLL 选择、STA 线程和 `.lnk` 解析；它不要求发布另一套 x86 Broker/Viewer。
- 原生层的输出既可能是普通文件系统路径，也可能是 `::{CLSID}` Shell Namespace 标识。
- Win32 helper 的职责只是在 x64 主进程无法直接向 32 位对话框线程注入钩子时桥接选择结果，不加载插件，也不拥有预览会话。

## 关键分支与边界

- `CF_HDROP` 可能受 `MAX_PATH` 限制，源码用 Shell ID List 作为回退。
- Explorer 标签页使用 `ShellTabWindowClass` 参与窗口匹配。
- Everything 1.5 优先读取隐藏窗口文本，旧版本可能借用剪贴板且尚未实现完整备份/恢复。
- 第三方文件管理器适配依赖窗口类名和非公开或应用特定协议，兼容性比 Explorer 更脆弱。
- COM apartment、窗口线程输入附加和跨位数钩子属于动态运行约束，Rust 类型系统不能消除。
- “支持 32 位文件对话框”和“发布完整 x86 主程序”是两个不同范围；前者只需隔离的 helper，后者会把 WPF Host、插件原生依赖、Renderer 和测试矩阵全部扩成双架构。
- Windows 11 只支持 64 位处理器；PowerToys 当前也只发布 x64 和 ARM64 安装包。完整 x86 主程序只有在明确支持 32 位 Windows 时才有独立价值。
- ARM64 的 Native/托管项目能进入解决方案并不代表格式能力可交付：`ImageViewer` 的 Release 目标删除 ARM64 的 Magick/Skia/SQLite 原生文件，`PDFViewer` 删除 ARM64 pdfium；这会把 ARM64 决策从“构建目标”提升为“格式与依赖清单”决策。

## 影响范围

- 必须修改：Broker 内建立统一 Selection Resolver，并保留按窗口类型路由和 Shell Namespace 结果；首版保留一个只有选择能力的 x86 Dialog Adapter 进程/DLL 接缝，由 Broker 监督其启动、超时和回收。
- 需要验证：x64 Explorer/桌面、x64 与 x86 文件对话框、搜索框、长路径、快捷方式、虚拟 Shell 对象、不同位数的第三方文件管理器和剪贴板保持。
- 仍待调查：哪些第三方适配仍被真实用户依赖；是否有除传统文件对话框外必须注入目标进程的路径。ARM64 从后续 `master` 回移到 `4.5.0` 基线所需的原生依赖、Legacy Host 和安装发布变化留给未来独立事项。

## 仍未知

- 每种适配器在当前 Windows 10/11 版本上的成功率和耗时。
- UI Automation 能否替代部分应用特定适配而不产生明显性能或兼容回退。
- x86 Dialog Adapter 在真实 32 位文件对话框中的启动耗时、选择成功率、超时和异常退出行为仍需运行验证。

## 证据索引

- [QuickLook.slnx](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.slnx)：`4.5.0` 主解决方案的平台和 Native32/Native64 组合。
- [QuickLook.cs](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook/NativeMethods/QuickLook.cs)：按当前进程架构选择 DLL、STA 调用和快捷方式解析。
- [DialogHook.cpp](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.Native/QuickLook.Native32/DialogHook.cpp)：检测目标对话框位数并选择直接钩子或 WoW64 helper。
- [WoW64HookHelper.cpp](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.Native/QuickLook.WoW64HookHelper/QuickLook.WoW64HookHelper.cpp)：Win32 helper 加载 Native32 并通过共享内存返回结果。
- [Shell32.cpp](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.Native/QuickLook.Native32/Shell32.cpp)：前台窗口分类和选择适配分派。
- [Everything.cpp](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.Native/QuickLook.Native32/Everything.cpp)：Everything 隐藏窗口和剪贴板路径。
- [PowerToys Releases](https://github.com/microsoft/PowerToys/releases/latest)：成熟 Windows 工具当前只发布 x64 和 ARM64 安装包，不发布完整 x86 主程序。
- [QuickLook.NativeArm64.vcxproj](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Native/QuickLook.NativeArm64/QuickLook.NativeArm64.vcxproj)：后续 `master` 的 ARM64 Native 编译路径。
- [ImageViewer ARM64 packaging](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Plugin/QuickLook.Plugin.ImageViewer/QuickLook.Plugin.ImageViewer.csproj)：Release 目标明确移除 ARM64 原生依赖。
- [PDFViewer ARM64 packaging](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Plugin/QuickLook.Plugin.PDFViewer/QuickLook.Plugin.PdfViewer.csproj)：Release 目标明确移除 ARM64 pdfium。
- 本机运行验证（2026-07-20）：QuickLook `4.1.1` 的托管程序集为 AnyCPU，实际运行进程经 `IsWow64Process2` 确认为本机 x64，并同时安装 Native32/Native64 DLL。
