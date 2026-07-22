# QuickLook 兼容性矩阵

## 使用说明

这份矩阵把静态源码证据和运行时验收分开。实现源码基线是 QuickLook `4.5.0`（提交 `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`）；已引用的 `master` 快照 `bca2fd4e863b8d9aef0d0d518837846a274c2b9a` 只用于补充发布后差异证据。`源码确认` 不表示当前机器或所有版本运行成功；`待实测` 需要用固定版本、Windows 版本和真实文件/插件完成验证。

## 用户触发与窗口上下文

| 场景 | 上游证据 | 当前状态 | Rust 混合架构验收 |
| --- | --- | --- | --- |
| Explorer 主窗口（`CabinetWClass` / `ExploreWClass`） | `Shell32::GetFocusedWindowType` 与 `getSelectedFromExplorer` | QuickLook 4.1.1 / Win11 x64 已验证选中文件空格预览 | 空格取得选中项；搜索框焦点不误触发 |
| 桌面（`WorkerW` / `Progman` + `SHELLDLL_DefView`） | `getSelectedFromDesktop` | 源码确认，运行待实测 | 桌面图标选择与 Explorer 具有相同 Toggle 语义 |
| 文件对话框（`#32770` + `DUIViewWndClassName`） | `DialogHook::GetSelected` 分支 | 源码确认，运行待实测 | 打开/保存对话框中不破坏输入，选择可预览 |
| Explorer 搜索框 | `IsExplorerSearchBoxFocused` | 源码确认，运行待实测 | 搜索框输入时不吞空格或方向键 |
| Everything | 隐藏窗口和剪贴板两条读取路径 | 源码确认，运行待实测 | 当前结果路径正确，剪贴板原内容不被意外破坏 |
| Directory Opus | `WM_COPYDATA` + XML 结果窗口 | 源码确认，运行待实测 | 首个选中项可预览，超时后有可恢复错误 |
| MultiCommander | 专用消息窗口和选择适配器 | 适配器存在，运行待实测 | 首个选中项可预览 |
| Internet Download Manager | 对话框标题和专用选择分支 | 适配器存在，运行待实测 | 不影响下载对话框操作 |
| FilePilot / DeskBox | 专用窗口匹配和选择适配器 | 适配器存在，运行待实测 | 依赖应用版本记录在测试结果中 |

## 键盘和窗口行为

| 行为 | 源码证据 | 当前状态 | 验收重点 |
| --- | --- | --- | --- |
| 空格按下 Toggle | `KeystrokeDispatcher` -> `PipeMessages.Toggle` | 源码确认，运行待实测 | 同路径关闭，不同路径替换 |
| 空格长按释放自动关闭 | 750ms 阈值和 `AutoCloseHolding` 配置 | 源码确认，运行待实测 | 按下/释放顺序、配置关闭时的差异 |
| 方向键切换 | 释放时发送 `Switch` | 源码确认，运行待实测 | 上下左右和相邻文件顺序 |
| Escape 关闭 | 释放时发送 `Close` | 源码确认，运行待实测 | 不影响宿主窗口输入 |
| F5 / F11 | `Reload` / `Fullscreen` | 源码确认，运行待实测 | 重载取消旧渲染，全屏可恢复 |
| Alt+Tab 后首次空格 | `4.5.0` 尚未包含；后续 `master` 用前台窗口事件清除无效键延迟 | 4.1.1 未包含该逻辑，并在自动化中复现 1 秒抑制 | 独立引入并验证上游 issue `#1939` 的修复；新前台窗口不继承旧按键抑制 |
| 固定窗口/焦点监视 | `ForgetCurrentWindow`、`FocusMonitor` | 源码确认，运行待实测 | 固定窗口和新窗口生命周期不串扰 |

## 插件与内容生命周期

| 能力 | 上游证据 | 当前状态 | 混合架构验收 |
| --- | --- | --- | --- |
| 用户插件目录优先参与扫描 | `PluginManager.LoadPlugins` | 源码确认 | Legacy Host 保持搜索路径和错误提示 |
| 内置插件反射加载 | `QuickLook.Plugin.*.dll` + `IViewer` | 源码确认 | 第三方二进制插件不改接口即可加载 |
| 优先级和第一匹配 | `Priority` 降序 + 串行 `CanHandle` | 源码确认 | 同扩展名覆盖行为保持 |
| `Init` 一次性初始化 | `InitLoadedPlugins` | 源码确认 | 宿主崩溃/重启后的初始化可重复 |
| `Prepare` 设置尺寸/主题 | `ContextObject` | 源码确认 | WPF Viewer 尺寸、主题和忙碌状态保持 |
| `View` 设置 WPF 内容 | `ViewerContent` | 源码确认 | 不把 WPF 对象泄漏进 Rust 协议 |
| `Cleanup` 释放资源 | `IViewer.Cleanup` | 源码确认 | 切换、关闭、取消和宿主重启均调用清理 |
| 插件失败回退默认信息页 | `ViewWindowManager.CurrentPluginFailed` | 源码确认 | Renderer/Legacy Host 失败都有可见降级 |

## 架构和部署维度

| 维度 | 上游证据 | 当前状态 | 首批验证 |
| --- | --- | --- | --- |
| x64 | `4.5.0` 的 AnyCPU/x64 主程序与 `QuickLook.Native64` | 本机 4.1.1 已确认以本机 x64 进程运行；4.5.0 源码确认 | Windows 10/11 x64 |
| x86 辅助适配 | `QuickLook.Native32`、Win32 `WoW64HookHelper`、`DialogHook` 位数判断 | 确认首版保留为受 Broker 监督的 x86 Dialog Adapter；不支持 32 位 Windows，不构建 x86 Broker/Viewer/Legacy Host/Renderer | x86 文件对话框选择、Adapter 启动/超时/退出与 Broker 恢复；x86 第三方文件管理器测试后再声明支持 |
| ARM64 | `4.5.0` 未包含 ARM64；后续 `master` 的 ARM64 编译和原生依赖证据保留作未来输入 | **本 Epic 和首版明确排除**；不纳入当前兼容承诺或验收矩阵 | 不验收；未来事项需重新定义格式集合、插件兼容、原生依赖和安装包 |
| 多显示器/DPI | Viewer 尺寸、显示器和窗口辅助逻辑 | 源码有相关能力，运行待实测 | 100/150/200% DPI，跨屏切换 |
| 深色/浅色主题 | `ThemeManager`、`ContextObject.Theme` | 源码确认，视觉待实测 | 主题切换和 Legacy Host 一致性 |
| Broker 崩溃恢复 | 目标架构，不是上游事实 | 未实现 | Renderer/Legacy Host 崩溃不带崩 Broker |
| Renderer 资源限制 | 目标架构，不是上游事实 | 未实现 | 超时、内存上限、Job Object 回收 |

## 运行时测量

### 本机基线（2026-07-20）

- Windows：Windows 11 专业版 `10.0.22631`，x64。
- 实现源码：QuickLook `4.5.0`，发布于 2026-04-14，标签最终指向提交 `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`。
- QuickLook：已安装并运行 `4.1.1`；发布标签指向提交 `55a069046f7e7d441c8978474fa887ca4ed0e499`。
- 进程：`QuickLook.exe` 是 AnyCPU 托管程序集；`IsWow64Process2` 报告实际运行进程为本机 x64，而不是 WoW64。安装目录同时包含 Native32/Native64 DLL，不能把 Native32 的存在解释为主程序以 x86 运行。
- 插件：安装目录有 18 个内置插件目录，用户插件目录有 17 个 `QuickLook.Plugin.*.dll`；这是后续 Legacy Host 二进制兼容测试的本机样本集。
- 暖态稳定快照：工作集 `154.6 MiB`、私有内存 `122.3 MiB`、983 句柄、30 线程。该快照在多轮预览后采集，不能当作干净启动或最小空闲数据。

通过启动第二个 `QuickLook.exe <path>` 实例触发现有单实例/Named Pipe 路径，轮询主进程可见顶层窗口并再次发送同一路径关闭。每种样本执行三轮：

| 样本 | Open ms | 中位数 | Close ms | 中位数 | 结果 |
| --- | --- | ---: | --- | ---: | --- |
| `QuickLook.exe.config` | 1796 / 1735 / 1743 | 1743 | 1763 / 1711 / 1719 | 1719 | 3/3 打开并关闭 |
| Epic `spec.md` | 1752 / 1740 / 1732 | 1740 | 1732 / 1755 / 1748 | 1748 | 3/3 打开并关闭 |
| `QuickLook.exe` | 1723 / 1722 / 1727 | 1723 | 1723 / 1726 / 1714 | 1723 | 3/3 打开并关闭 |

所有第二实例退出码均为 0，窗口最终恢复为无可见预览。三类样本结果集中在约 `1.72-1.80 s`，说明当前测量混入稳定的单实例、Dispatcher、窗口显示或关闭路径开销；它不是解码器首帧的精确测量。后续需要在进程内增加时间点或使用 ETW/UI Automation，把命令接收、插件匹配、窗口可见和内容就绪拆开。

Explorer 主路径通过新窗口 `/select` 选中同一 `spec.md`，同时验证前台 HWND 和 `FocusedItem.Path` 后发送空格。4 个有效样本的空格到可见窗口时间为 `162 / 167 / 96 / 174 ms`，中位数约 `165 ms`，每轮窗口标题都是 `QuickLook - spec.md`，并恢复到无可见预览。另一次没有满足前台校验，脚本未发送输入，不计入产品失败。

自动化为取得前台权限先发送 Alt。QuickLook 4.1.1 会把 Alt 记为无效按键，并在一秒内抑制随后空格；等待超过一秒后测试成功。源码核对确认 `4.5.0` 仍有相同行为，后续 `master` 才通过前台窗口变化事件清除此状态。因此上游 issue `#1939` 的变更是当前第一个明确的候选补丁，但仍需作为独立变化完成回归验证，不能因为它位于 `master` 就整批引入其他提交。

### 导入源码构建基线（2026-07-22）

- 命令：`pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1`。
- 环境：Windows 11 专业版 `10.0.22631` x64；Visual Studio Enterprise 2022 `17.14.20`；MSBuild `17.14.23.42201`；WiX Toolset `3.14.1.8722`。
- 前置条件：本机原先缺少 WiX Toolset；安装官方 WiX 3.14.1 后，构建 wrapper 从机器级 `WIX` 环境读取工具路径。subtree 不保留上游标签拓扑，因此 wrapper 仅在构建期间让上游 `git describe` 得到固定版本 `4.5.0`，未修改 tracked QuickLook 源码。
- 结果：完整 `QuickLook.sln` Release 构建成功并输出 `LEGACY_BUILD_OK`。构建包含 x64 主程序、Native64 和上游既有的 Native32/WoW64 helper；未发现或构建 ARM64 配置。
- 产物：`src/legacy/quicklook/Build/Release/QuickLook.exe`（12,507,648 bytes）、`src/legacy/quicklook/Build/QuickLook-4.5.0.msi`（94,425,456 bytes）、`src/legacy/quicklook/Build/QuickLook-4.5.0.zip`（122,532,526 bytes）。
- 警告：还原时 WiX project 报 `NU1503`，PDF/Thumbnail plugin 报既有 `System.IO.Compression` 版本冲突，Markdown plugin 报成员隐藏，WiX 报 `ICE61`；均未转化为构建错误。本轮只记录这些上游基线信号，不把它们改写为 PreviewIt 产品限制。

每个场景至少记录：

- Broker/旧程序启动到可接收空格的时间；
- 空格到首个可见结果的冷启动和热启动时间；
- 空闲 CPU、私有工作集和预览中峰值内存；
- 快速切换时旧请求被取消的时间和是否出现旧结果覆盖；
- 插件/Renderer 退出码、回退结果和恢复时间；
- 失败路径是否留下窗口、句柄、临时文件或剪贴板变化。

原始文件集应包含：普通图片、动画图片、文本、Markdown、压缩包、PDF、视频、Shell Namespace 对象，以及至少一个真实第三方插件。文件集、QuickLook 提交、Windows 版本和架构必须随结果保存。

## 证据索引

- [Shell32.cpp](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Native/QuickLook.Native32/Shell32.cpp)：窗口分类和 Shell/第三方选择分支。
- [HelperMethods.cpp](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Native/QuickLook.Native32/HelperMethods.cpp)：Shell DataObject、长路径和焦点判断。
- [KeystrokeDispatcher.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/KeystrokeDispatcher.cs)：键盘行为和时间阈值。
- [PluginManager.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/PluginManager.cs)：插件扫描、排序、初始化和异常。
- [QuickLook.slnx](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.slnx)：目标架构和内置项目清单。
