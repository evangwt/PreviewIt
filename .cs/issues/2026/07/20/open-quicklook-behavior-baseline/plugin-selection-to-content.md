# 文件路径如何匹配插件并成为窗口内容

## 读者先带走什么

QuickLook 从用户目录和应用目录反射加载 WPF 插件，按优先级寻找第一个 `CanHandle` 成功者，再创建新实例并通过共享 WPF Context 完成准备、显示和清理。

## 主路径

插件管理器构造时扫描用户插件目录和内置插件目录，查找 `QuickLook.Plugin.*.dll`。每个程序集的公开具体类型如果实现 `IViewer`，就创建实例加入列表；加载完成后按 `Priority` 降序排序并调用每个实例的 `Init`。

预览请求到来时，管理器按排序后的列表依次调用 `CanHandle(path)`，吞掉单个插件探测异常，使用第一个返回成功的插件；没有匹配项时使用默认信息插件。匹配后不是复用探测实例，而是按类型创建一个新的 `IViewer` 实例交给窗口。

插件先在 `Prepare` 中设置标题、首选大小、主题和窗口能力，再在 `View` 中向 `ContextObject.ViewerContent` 填入 WPF 内容并结束忙碌状态。切换或关闭时 Viewer 调用 `Cleanup` 释放原生和媒体资源。插件异常时窗口管理器关闭当前窗口、记录日志，并在非默认插件失败时回退到默认插件。

## 关键责任、数据和状态

- `IViewer` 是现有插件兼容性的最小公开生命周期：`Priority`、`Init`、`CanHandle`、`Prepare`、`View`、`Cleanup`。
- `ContextObject` 是共享 UI 状态，直接使用 `System.Windows.Size`、`INotifyPropertyChanged` 和任意 `ViewerContent` 对象。
- 插件自身拥有内容控件、异步加载和非托管资源，宿主只通过约定协调忙碌状态和清理。
- 用户插件先于应用内置插件加载；相同格式的覆盖行为还受优先级和扫描顺序影响。
- 仓库当前包含 25 个内置插件项目，全部以 `net462 + WPF` 构建，其中部分还使用 WinForms。

## 关键分支与边界

- `CanHandle` 是串行第一匹配，探测耗时直接进入预览关键路径。
- `Init` 在应用启动时对所有已加载插件执行，插件可以解压资源或初始化全局状态。
- 插件 DLL 在主进程中通过 `Assembly.LoadFrom` 加载；托管异常通常被捕获，但原生崩溃或进程级错误不会被语言级捕获隔离。
- 便携版插件可能受 Windows Zone Identifier 阻止，宿主会尝试解除阻止并重启。
- `ViewerContent` 是任意 WPF 对象，不能直接序列化成跨语言或跨进程协议。

## 影响范围

- 必须修改：Legacy Host 保留现有加载顺序、优先级、生命周期、Context 和默认插件回退；Broker/Viewer 协议只传高层状态，不传 WPF 对象。
- 需要验证：用户插件覆盖、插件 `Init` 副作用、CanHandle 顺序、插件安装/解除阻止、异步忙碌状态、窗口尺寸和 Cleanup。
- 仍待调查：外部插件对 `ContextObject.Source`、具体 ViewerWindow 类型、反射或未公开行为的依赖；这些依赖可能超过公开 `IViewer` 契约。

## 仍未知

- 真实第三方插件样本及其最低/最高 QuickLook.Common 版本。
- 插件在单独 .NET 进程中运行时是否依赖主程序集位置、当前目录或 AppDomain 全局状态。
- WPF Legacy Host 采用独立顶层窗口后，固定窗口、多窗口和焦点行为能否保持一致。

## 证据索引

- [IViewer.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Common/Plugin/IViewer.cs)：公开插件生命周期。
- [ContextObject.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Common/Plugin/ContextObject.cs)：WPF 尺寸、内容和窗口状态契约。
- [PluginManager.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/PluginManager.cs)：目录扫描、反射加载、优先级、探测和初始化。
- [ViewWindowManager.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/ViewWindowManager.cs)：插件失败回退和窗口生命周期。
- [QuickLook.Common README](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.Common/README.md)：第三方插件通过 NuGet 引用共享契约。
- [QuickLook.slnx](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook.slnx)：内置插件项目和目标架构清单。
