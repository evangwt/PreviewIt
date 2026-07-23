---
kind: epic
title: "Rust 混合预览架构"
status: draft
created: 2026-07-20
---

# Rust 混合预览架构

## 这个 Epic 要改变什么

在保留现有 QuickLook 插件兼容性的前提下，把常驻系统集成、请求路由和进程治理逐步迁移到 Rust，并让新的预览器以独立进程接入。现有 .NET/WPF Viewer 和插件宿主保留为长期兼容边界，不把彻底移除 WPF 作为本 Epic 的目标。

## 为什么现在做

QuickLook 的常驻进程同时承担全局热键、Shell 选择、WPF UI、插件加载和多种文件解码。第三方插件或原生解码器发生故障时，故障边界过大；插件初始化和预览能力也缺少稳定的跨语言协议。

混合方案可以先获得进程隔离、故障恢复和按需加载的收益，同时继续使用现有的 25 个内置插件及第三方插件生态。它把 Rust 的采用限制在真实变化边界内，不要求一次性重写所有格式支持。

## 关联 Project Spec

- `.cs/spec/index.md`：当前只是 CodeStable 初始骨架；Epic 关闭后，经过验证的系统边界、插件兼容策略和运行时职责再回写为项目当前真相。

## 当前方案

`PreviewIt` 作为 QuickLook 的 GPLv3 派生实现演进，允许直接复用 QuickLook.Common、WPF Viewer/Legacy Host、插件加载契约和必要的 Windows 原生适配；新 Rust 代码和分发产物保持 GPLv3 兼容。

实现以 QuickLook 最新稳定版 `4.5.0`（提交 `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`）为源码基线，不直接跟随持续变化的 `master`。该提交已连同 GPLv3 来源记录导入仓库，且完整 solution 可在不修改上游源码的前提下构建。本机已安装的 `4.1.1`（提交 `55a069046f7e7d441c8978474fa887ca4ed0e499`）及其 17 个用户插件程序集保留为向后兼容样本；`master` 只作为发布后关键修复的候选来源，每项修复都需单独验证后引入。

长期采用演进式混合内核：常驻的 Rust Broker 是控制面，负责单实例、全局键盘钩子、前台窗口识别、当前选择获取、预览会话、路由策略、缓存索引、Named Pipe IPC 和子进程监督；统一 Viewer Shell 是显示面，负责窗口、主题、DPI、导航和用户可见状态；Legacy Host、Renderer Worker 和系统预览处理器构成受监管执行面。

现有 `IViewer` 插件继续在独立的 .NET/WPF Legacy Host 中加载，插件接口和 WPF 控件模型不跨进程暴露给 Rust。新图片、文本、压缩包等低风险能力使用独立 Rust Renderer 进程；Windows 已有的 `IPreviewHandler` 优先复用系统宿主。

第一阶段保留现有 WPF Viewer，Rust Broker 通过协议驱动它。Viewer 是技术可替换但职责稳定的统一 Shell；只有协议、生命周期和故障恢复稳定后，才单独评估 Rust/WinUI Viewer。`ExternalWindow` 只作为旧插件、系统组件和复杂交互内容的兼容例外，不发展为普通 Renderer 的默认集成方式。

Renderer 使用自适应受监管 Worker：每个 Renderer 包和安全边界使用独立进程，安全且高频的 Renderer 可以复用热进程并在空闲 TTL 后回收，高风险或高成本任务使用单请求临时进程。Renderer manifest 只声明能力和成本提示，Broker 保留并发、超时、资源限制和回收的最终决定权。

新 Renderer 生态先使用内部 v0；图片、文本和压缩包 Renderer 已通过真实升级、取消和崩溃恢复验证后，再发布包含 manifest、语言无关协议、兼容测试套件和诊断工具的公共 v1。

位数兼容按责任拆分，而不是复制整套应用。首版只发布 x64 Broker、Viewer、Legacy Host 和 Renderer，不支持 32 位 Windows；同时保留现有 Native32/WoW64 helper，并把它收拢为只负责读取 32 位传统文件对话框选择的 x86 Dialog Adapter。该 Adapter 不加载插件、不拥有预览会话，由 Broker 监督启动、超时和回收。x86 第三方文件管理器只有通过兼容测试后才列为支持项。ARM64 明确排除在本 Epic 和首版交付之外；协议、manifest 和 Renderer 能力声明保留架构扩展字段，未来另立事项评估，不把后续 `master` 的 ARM64 代码回移到本 Epic。

## 需求变化

- 用户按空格预览的主流程保持不变。
- 当前选择、切换、关闭、重载、置顶、全屏和取消都通过带 `request_id` 的请求完成。
- 旧插件的 `IViewer`、`ContextObject`、WPF 控件和插件搜索路径保持兼容。
- 首版主程序只支持 x64 Windows；32 位传统文件对话框通过受限的 x86 Dialog Adapter 保持选择能力，不扩展为完整 x86 应用。
- 首版及本 Epic 不交付 ARM64；ARM64 原生依赖、Legacy Host 和格式覆盖留给未来独立事项。
- 新渲染器不再以 Rust 动态库 ABI 加载，而以版本化进程协议接入。
- 插件或解码器崩溃时，目标结果是当前预览失败或降级，Broker 继续运行。
- 预览处理不应要求 Broker 直接加载第三方解码器或不受信任的插件 DLL。
- Broker 打开并校验文件，再向执行单元传递最小权限的只读句柄或受控流；必须使用路径的系统组件作为显式例外。
- 新 Renderer 分为产品签名、签名第三方和开发者模式未签名三个信任层级；旧插件继续由 Legacy Host 隔离，不套用新 SDK 契约。

## 架构考量

### 进程职责

- **Rust Broker**：系统集成和会话状态的唯一归属；不拥有 WPF 控件，不直接承载第三方解码器。
- **Viewer**：用户可见窗口和交互状态的唯一归属；第一阶段沿用 WPF，后续 UI 技术可独立评估而不改变 Renderer 输出契约。
- **Legacy Host**：只负责 .NET 插件加载、WPF 控件生命周期和旧插件错误隔离。
- **x86 Dialog Adapter**：只在 x64 Broker 需要读取 32 位传统文件对话框选择时运行；不加载插件、不渲染内容、不持有会话，由 Broker 监督和回收。
- **Rust Renderer**：只负责声明格式能力、准备数据和生成预览结果；由 Broker 管理启动、取消和回收。
- **System Preview Handler**：优先复用 `IPreviewHandler`/`prevhost.exe`，避免重复实现 Windows 已有能力。

### 协议和会话

旧的 `message|path|options` 字符串协议不作为新边界。新协议使用当前用户专属 Named Pipe 上按长度分帧的 Protobuf，至少包含协议版本、`request_id`、来源窗口类型、主题、DPI、首选尺寸、能力和错误类型。协议按 `major.minor + capabilities` 协商：主版本不兼容时拒绝连接，次版本通过能力协商向后兼容。

Foundation 已验证协议 `0.1` 的最小子集：Protobuf envelope 使用 4-byte little-endian 长度前缀，单个控制帧最大 1 MiB，Rust 与 `net462` 保持编码和错误行为一致。`read-handle-v0` 作为显式 capability 协商；超出该子集的 Viewer、Renderer 和路由消息仍须在后续 Issue 中按真实调用路径扩展。

Broker 单实例控制面也已验证：每个交互式用户会话先通过 current-user/SYSTEM 的 session-scoped lease 选出一个 x64 primary，secondary 不初始化第二套控制面，而是通过确定命名、local-only、当前用户专属的 Named Pipe 发送一次有界 `OpenPath` 或 `Close` 并等待 typed ack。endpoint 未就绪、连接/解码饱和和已解码队列满分别使用 `primary-not-ready`、`primary-busy` 和 `queue-full`；过载拒绝不得停止 endpoint。`accepted` 只表示命令已经进入 Broker session，不承诺文件存在或预览成功。

请求状态按 `Idle -> Resolving -> Preparing -> Rendering -> Ready -> Closing` 管理。新请求取消旧请求；所有异步结果必须携带 `request_id`，过期结果丢弃。Router 只做 request ID 生成、命令去重和单步领域归约，Broker Runtime 是状态与 effect 的唯一所有者；每个 reducer step 都独立观察 `(phase, request_id)`，旧请求完成清理后才允许最新 pending request 进入 `Resolving`。真实 Shell Resolver、Worker 或 Viewer 尚未接入时，Runtime 只在同一 effect 入口完成最小 cleanup feedback，后续执行资源不得绕过该入口另建状态所有者。

渲染结果支持三个形态：`DocumentModel` 承载有界、版本化的语义内容，`SharedSurface` 通过共享内存或共享图形资源承载栅格、视频和 GPU 输出，`ExternalWindow` 用于 Legacy Host、系统组件和复杂交互内容。第一阶段不把跨进程任意 HWND 嵌入作为统一方案。

每个请求使用有界候选链。系统已有可靠 `IPreviewHandler` 时优先复用；被内置 Renderer 明确接管的格式可以使用内置实现作为主路径。连续崩溃的 Renderer 进入退避或本次运行期隔离，同一请求不能无限重试。

### 插件与安全边界

新插件通过 manifest 声明 ID、协议版本、优先级、扩展名/MIME、架构和能力。Broker 先用声明完成粗路由，再对少数候选执行探测，避免每次选择都串行调用全部插件。

Renderer 使用 Job Object 管理，分别设置启动、探测、首帧、总渲染和取消回收超时；安装目录只读，Named Pipe 限制到当前用户 SID，并验证连接进程身份。Job Object 只负责生命周期和资源治理，不被视为安全沙箱；第三方 Renderer 还需受限令牌，并在兼容时使用 AppContainer。路径校验、Shell Namespace 路径处理和外部程序启动规则集中在 Broker，不由各个 Renderer 自行解释。

Foundation 已证明最小 Windows 边界可行：local-only pipe 的 DACL 只允许当前 token 用户与 SYSTEM，Broker 在读取协议前核对已启动子进程的 PID；Broker 以只读权限打开文件，把 non-inheritable same-access handle 复制到该子进程，不提供路径回退。一个 Worker 由一个带 `KILL_ON_JOB_CLOSE` 的 Job Object 管理，超时、崩溃、挂起和过期结果不会拖垮 Broker；这证明生命周期机制，不把 Job Object 提升为安全沙箱。

Broker 统一管理以文件身份、修改时间和大小、Renderer ID/版本及输出参数为键的有界、可丢弃缓存。网络位置、临时文件、加密文件和隐私敏感内容默认不持久缓存；Renderer 不能拥有权威缓存状态。

### 不选的替代方案

- 不采用 clean-room 重建现有插件契约的路线。
- 不把所有现有 WPF 插件一次性翻译成 Rust。
- 不使用 Rust trait object 作为长期插件 ABI。
- 不把 PDFium、FFmpeg、WebView2、ImageMagick 等重型依赖同时替换为自研纯 Rust 实现。
- 不把 Rust/WinUI UI 作为第一阶段前置条件。

## 统一语言

- **Broker**：常驻的 Rust 系统集成进程；拥有预览会话和子进程治理。
- **Viewer**：拥有用户可见预览窗口的进程；第一阶段是现有 WPF Viewer。
- **Legacy Host**：承载旧 .NET/WPF 插件的兼容宿主；不是 Rust 插件适配层。
- **x86 Dialog Adapter**：首版唯一的 x86 运行组件；把 32 位传统文件对话框的当前选择返回给 x64 Broker。
- **Renderer**：实现一个或一组格式预览能力的独立进程。
- **Preview Protocol**：Broker、Viewer、Legacy Host 和 Renderer 之间的版本化消息契约。
- **request_id**：一次预览请求的唯一标识；用于取消、幂等和丢弃过期结果。
- **ExternalWindow**：由 Renderer 或 Legacy Host 拥有窗口，Viewer/Broker 只同步其生命周期和位置。
- **DocumentModel**：Renderer 输出的有界、版本化语义树，由 Viewer 使用原生控件呈现。
- **SharedSurface**：Renderer 写入共享内存或共享图形资源、由 Viewer 显示的栅格或 GPU 内容。

## 当前推进

### 可推进范围

- 继续以已导入、可复现构建的 QuickLook `4.5.0` 固定提交为兼容基线；后续 `master` 补丁保持逐项来源记录和独立验证。
- 建立现有行为基线：热键、Shell 选择、窗口焦点、DPI、插件和失败恢复。
- 从已验证的 Rust Broker 单实例、typed command ack 和请求状态边界继续接入全局热键与选择获取，不把尚未验证的 Shell/Viewer 路径算作已完成。
- 把现有 Native32/WoW64 helper 收拢为由 Broker 监督的 x86 Dialog Adapter，并验证 32 位文件对话框选择、超时和异常退出恢复。
- 从已验证的协议 `0.1` framing、版本协商、`request_id` 和 `read-handle-v0` 边界继续扩展请求、取消、错误和版本兼容行为。
- 把现有插件加载和 WPF Viewer 逐步收拢为 Legacy Host 边界。
- 以图片、文本和压缩包为第一批 Rust Renderer 候选，保留系统预览器和旧插件作为回退。
- 建立 Broker 管理的缓存、Renderer 信任分层、内部 v0 SDK 和兼容测试套件；公共 v1 在内置 Renderer 积累真实证据后再发布。

### Issues

- [ ] `.cs/issues/2026/07/20/open-quicklook-behavior-baseline/index.md`：固定空格到预览、Shell 选择和插件内容三条主路径；补齐运行时兼容矩阵后关闭。
- [x] `.cs/issues/2026/07/22/closed-preview-foundation-vertical-slice.md`：导入并构建固定基线，验证 x64 Rust/.NET 协议、身份、只读句柄与单 Worker 故障恢复边界。
- [x] `.cs/issues/2026/07/23/closed-rust-broker-single-instance-request-state-machine.md`：在每个交互式会话中选出一个 x64 Broker，转发有界 command，并验证 `request_id` 状态机与 stale/cancel 规则。
- [ ] 需要创建 Feature：Shell Resolver 与受限 x86 Dialog Adapter 边界。
- [ ] 需要创建 Refactor：Legacy .NET/WPF Plugin Host 边界。
- [ ] 需要创建 Feature：Renderer registry、manifest、supervisor policy 与 cache。
- [ ] 需要创建 Feature：系统 `IPreviewHandler` 路由。
- [ ] 需要创建 Feature：Rust 图片/文本 Renderer MVP。
- [ ] 需要创建 Feature：公开 Renderer SDK v1 readiness。

### 暂停或废弃

- 暂无。Foundation vertical slice 已完成，生产路径尚未接管。

### 剩余阻碍

- Rust Broker 的单实例、外部 command ack 与纯请求状态转换已经验证；全局热键、真实 Shell 选择和 Resolver/Viewer effect adapter 尚未接入该 Runtime。
- Shell Resolver、x86 Dialog Adapter、Viewer/Legacy Host 和 Renderer 的生产进程边界仍需按后续 Issue 逐项验证，不能从 WorkerProbe 直接外推。

## 暂不推进范围

- 完全移除 WPF 或把所有旧插件迁移为 Rust UI。
- 一次性重写 Office、PDF、视频、网页和全部 RAW 格式解码器。
- 改变旧 QuickLook 插件的签名要求；新 Renderer 使用独立的签名和开发者模式信任策略。
- 重写安装器、自动更新器和商店发布流程，除非当前部署验证证明它们阻塞 Broker 交付。
- 发布完整 x86 Broker、Viewer、Legacy Host 或 Renderer，以及支持 32 位 Windows。
- 交付 ARM64 Broker、Viewer、Legacy Host、Renderer 或安装包；未来事项必须重新评估原生依赖和插件覆盖。

## 未确认问题

- 首批 Renderer 的最小格式集合和验收文件集是什么；不同答案会影响 MVP 的依赖和测试规模。
- 不同 Renderer 在受限令牌或 AppContainer 下的实际兼容范围，以及默认资源和 TTL 参数，需要通过原型和基准测试确定。

## 关闭条件

- Broker、Viewer、Legacy Host 和 Renderer 的职责边界通过真实用户路径验证。
- x64 主程序通过 x86 Dialog Adapter 取得 32 位传统文件对话框选择；Adapter 超时或退出不会拖垮 Broker，且不加载插件或预览内容。
- 现有内置及第三方插件兼容策略有可执行测试和失败降级行为。
- Preview Protocol 有版本、取消、超时、错误和崩溃恢复测试。
- 至少一批 Rust Renderer 在 Explorer、桌面、DPI、多显示器和快速切换场景通过行为验证。
- x64 首版安装包不宣称 ARM64 支持，且架构字段不会误将 ARM64 候选能力显示为已交付格式。
- Legacy 热路径中位延迟不劣于约 165 ms 基线且 P95 回归不超过 15%；图片和文本新 Renderer 热路径目标改善至少 20%。
- Worker 回收后稳定空闲私有内存目标相对约 122 MiB 基线降低 50% 以上；性能、崩溃恢复和日志指标使用同机同语料对比。
- 用户确认稳定结论后，Epic 才关闭并回写 Project Spec。

## 合并回 Project Spec 的候选

- QuickLook 的系统集成、用户界面、旧插件和新 Renderer 的稳定职责边界。
- 旧插件通过 Legacy Host 保持兼容的长期策略。
- Preview Protocol 的稳定术语、会话状态和错误恢复规则。
- 已验证的协议 framing、当前用户 pipe 身份验证、只读句柄所有权和 Worker 回收边界。
- 已验证的会话级单实例、typed command ack、有界过载语义和 Runtime 单一状态/effect 所有权。
- 哪些格式由系统预览器、旧插件或 Rust Renderer 负责。

## 关闭回写

- 状态：关闭时改为 `closed`。
- 合并位置：待 Project Spec 建立对应的能力和架构章节后确定。
- 保留材料：迁移过程、被否决的 UI 方案、协议演进和兼容性证据保留在本 Epic。

## 相关材料（按需）

- [讨论收束稿](../../../../../talks/2026/07/20/rust-hybrid-preview-architecture.md)：查看用户确认的兼容性约束、WPF 宿主决策和未决分叉。
- [长期架构设计](../../../../../../docs/plans/2026-07-22-rust-hybrid-preview-architecture-design.md)：查看已批准的完整组件边界、数据流、安全、SDK、迁移和验证设计。
- [QuickLook 4.5.0](https://github.com/QL-Win/QuickLook/releases/tag/4.5.0)：实现源码基线；标签提交固定为 `b13df028f3cce1f84792f7043b57bf5cea3a3e4c`。
- [QuickLook 主程序](https://github.com/QL-Win/QuickLook)：核对现有用户路径和运行时边界。
- [QuickLook 插件接口](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook.Common/Plugin/IViewer.cs)：按实现基线核对第三方兼容约束。
- [QuickLook 插件加载器](https://github.com/QL-Win/QuickLook/blob/b13df028f3cce1f84792f7043b57bf5cea3a3e4c/QuickLook/PluginManager.cs)：按实现基线核对旧插件发现、优先级和初始化行为。
- [PowerToys Peek 预览器工厂](https://github.com/microsoft/PowerToys/blob/main/src/modules/peek/Peek.FilePreviewer/Previewers/PreviewerFactory.cs)：参考预览器分工和回退策略。
- [PowerToys Shell Preview Handler](https://github.com/microsoft/PowerToys/blob/main/src/modules/peek/Peek.FilePreviewer/Previewers/ShellPreviewHandlerPreviewer/ShellPreviewHandlerPreviewer.cs)：参考系统预览器和 `prevhost.exe` 隔离方式。
- [ArcThumb](https://github.com/citrussoda-com/ArcThumb)：参考 Rust 实现 Windows COM Shell Extension 的边界处理。
