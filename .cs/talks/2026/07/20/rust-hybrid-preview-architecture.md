# Rust 混合预览架构 talk

## 原始想法

评估是否使用 Rust 实现 [QL-Win/QuickLook](https://github.com/QL-Win/QuickLook)，并在确认可行性后设计一套混合架构。

## 真问题

问题不是 Rust 能否调用 Windows API，而是如何在不破坏 QuickLook 现有插件生态的前提下，获得更小的故障边界、可控的解码器隔离和渐进式迁移路径。

## 术语

- **Broker**：常驻 Rust 系统集成进程，负责热键、Shell 选择、请求路由和子进程治理。
- **Viewer**：拥有用户可见预览窗口的进程。
- **Legacy Host**：长期承载旧 .NET/WPF 插件的兼容宿主。
- **Renderer**：实现预览能力的独立进程，可由 Rust 或系统 `IPreviewHandler` 提供。
- **Preview Protocol**：Broker、Viewer、Legacy Host 和 Renderer 之间的版本化协议。

## 已确认决策

- 现有及第三方 QuickLook 插件兼容性是硬约束。
- WPF Viewer/Legacy Host 长期保留，不把彻底移除 WPF 作为本 Epic 的目标。
- `PreviewIt` 采用 QuickLook GPLv3 派生路线，允许直接复用 QuickLook.Common、WPF Legacy Host、插件契约和必要的原生适配。
- Rust 首先承担 Broker、进程治理和新 Renderer，不一次性重写全部插件与格式支持。
- 新插件使用进程协议，不使用 Rust trait object 作为长期 ABI。
- Windows 系统 `IPreviewHandler` 优先复用，复杂或高风险解码器不直接加载到 Broker。

## 约束

- 空格预览、切换、关闭、重载、置顶、全屏等用户路径应保持稳定。
- 旧 `IViewer`、`ContextObject`、WPF 控件和插件搜索路径必须继续工作。
- Broker、Viewer、Legacy Host 和 Renderer 的故障边界必须可观察、可恢复。
- 新协议必须支持版本、取消、超时、错误和过期响应丢弃。

## 影响面、风险与取舍

- 进程隔离会增加部署、IPC、窗口同步和调试复杂度，但能把插件/解码器故障限制在当前预览。
- 保留 WPF 宿主能保护兼容性，但不会立即消除 WPF 的运行时开销。
- Rust Renderer 可以逐步迁移图片、文本和压缩包等能力；PDF、视频、Office、WebView2 等重型能力暂时依赖现有插件或系统处理器。
- 跨进程任意 HWND 嵌入、ARM64/x86 辅助组件和首批 Renderer 格式仍需通过探索和原型确认。

## 分歧

- x86 是否是必须长期支持的兼容目标尚未确认。
- Windows ARM64 是否纳入第一批交付尚未确认。
- Preview Protocol 的具体序列化实现尚未确认，Protobuf 是当前候选。
- 旧插件窗口与新 Renderer 的视觉布局是否必须像素级一致尚未确认。

## 初步出口草案

- 建议出口：新建 Epic `Rust 混合预览架构`。
- 判断理由：变化跨越常驻进程、WPF 插件宿主、IPC、Renderer、系统预览器和测试验证，需要活规格承载，并且会分多批 issue 推进。
- 候选事项：先建立现有行为基线和兼容性矩阵，再实现 Broker/协议/Legacy Host 边界，最后迁移首批 Rust Renderer。
- 暂不纳入：彻底移除 WPF、一次性迁移所有插件、同时替换全部媒体和文档解码器、重写安装器和更新器。
