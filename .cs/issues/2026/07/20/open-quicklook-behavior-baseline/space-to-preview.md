# 用户按空格如何产生预览窗口

## 读者先带走什么

空格键先经过低级键盘钩子和上下文过滤，再被转换为带当前选择的 Toggle 命令；窗口管理器完成路径校验和插件匹配后，才让 Viewer 显示内容。

## 主路径

应用启动时先完成单实例检查，然后初始化托盘、原生选择适配、插件管理器、窗口管理器、键盘分发器和命名管道。低级键盘钩子监听系统按键，但只有 Explorer、桌面、受支持的文件窗口或 QuickLook 自身位于前台时，空格才会进入预览请求路径。

按下空格时，键盘分发器发送 Toggle 命令。窗口管理器在没有显式路径时请求当前选择：如果同一路径的预览已显示就关闭，否则校验文件/目录、扩展过滤和 CLSID 特例，选择匹配插件并开始显示新窗口。

Viewer 先卸载上一个插件，再用新插件和路径开始显示。源码中的命令分发会把 UI 操作投递到 WPF Dispatcher；命名管道还允许第二个进程或 CLI 把相同类型的命令发送给首个实例。

## 关键责任、数据和状态

- 键盘钩子只采集事件并决定是否吞掉按键，不负责取得文件或打开窗口。
- 键盘分发器持有空格按下、长按和最近无效按键时间，决定 Toggle、Switch、Close、Reload 和 Fullscreen 命令。
- 命令分发器把跨线程/跨进程消息转到 UI Dispatcher，并中止仍在排队的旧窗口操作。
- 窗口管理器持有当前路径和当前 ViewerWindow，负责同路径切换、固定窗口、焦点监视和插件失败回退。
- 路径是主请求数据；当前协议还把选项编码成逗号分隔字符串，没有请求 ID 或显式取消契约。

## 关键分支与边界

- Windows 键、修饰键、非支持按键和 Explorer 搜索框状态会阻止请求进入预览。
- QuickLook 4.1.1 和实现基线 `4.5.0` 都会在任意无效按键后一秒内抑制有效键，Alt 切换到 Explorer 后可能影响首次空格；后续 `master` 为上游 issue `#1939` 增加前台窗口变化钩子清除此状态，该变更需要作为独立候选补丁验证。
- 空格按住超过阈值后释放可以触发自动关闭；普通按下/释放语义需要运行验证。
- 第二实例通过命名管道向首实例发送 Toggle，而不是再创建完整 UI。
- 快速切换时只会中止尚未执行的 DispatcherOperation；已经进入插件工作线程的操作如何取消，源码没有统一契约。
- 当前管道消息使用分隔字符串，路径/选项编码和版本演进能力有限。

## 影响范围

- 必须修改：Broker 接管键盘上下文和请求状态；Preview Protocol 表达 Toggle/Switch/Close/Reload/Fullscreen、请求 ID 和取消；WPF Viewer 只消费高层命令。
- 需要验证：按下/释放、长按、修饰键、前台窗口切换、同路径关闭、快速连续切换和 CLI 第二实例。
- 仍待调查：固定窗口、多窗口和焦点监视是否由 Broker 还是 Viewer 拥有，需运行观察后决定。

## 仍未知

- 本机 QuickLook 4.1.1 的 Explorer 空格到可见窗口中位数约为 `165 ms`；内容首帧、冷启动和其他格式的分布仍未知。
- 插件已经开始 `View` 后切换文件的资源清理和竞态表现。
- 不同 Windows 版本和 Explorer 标签页下的前台窗口识别差异。

## 证据索引

- [App.xaml.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/App.xaml.cs)：启动顺序、单实例和命名管道入口。
- [GlobalKeyboardHook.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/Helpers/GlobalKeyboardHook.cs)：低级键盘钩子和事件转发。
- [KeystrokeDispatcher.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/KeystrokeDispatcher.cs)：有效按键、空格长按和命令映射。
- 本机运行验证（2026-07-20）：Windows 11 22631 x64、QuickLook 4.1.1、Explorer 选中 `spec.md` 后发送空格，4 个有效样本为 `96-174 ms`，中位数约 `165 ms`。
- [PipeServerManager.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/PipeServerManager.cs)：字符串协议和 UI Dispatcher 分发。
- [ViewWindowManager.cs](https://github.com/QL-Win/QuickLook/blob/bca2fd4e863b8d9aef0d0d518837846a274c2b9a/QuickLook/ViewWindowManager.cs)：路径校验、插件选择、窗口切换和失败回退。
