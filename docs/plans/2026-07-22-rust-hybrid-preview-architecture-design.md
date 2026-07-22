# PreviewIt Rust 混合预览架构设计

日期：2026-07-22
状态：已批准

## 目标

PreviewIt 以 QuickLook 4.5.0 为实现基线，在不破坏现有 QuickLook 插件生态的前提下，把常驻系统集成、请求路由和进程治理迁移到 Rust，并为新的预览能力建立可隔离、可恢复、可跨语言演进的 Renderer 进程协议。

这项设计首先解决故障边界和长期演进问题。Rust 带来的启动、内存和热路径收益是重要验收指标，但不是以兼容性换性能的理由。

首版只交付 x64 主程序。32 位传统文件对话框选择由受限的 x86 Dialog Adapter 处理；ARM64 不属于本 Epic 或首版范围。

## 选定路线

采用演进式混合内核：Rust Broker 是控制面，Unified Viewer Shell 是显示面，Legacy Host、Renderer Worker 和系统预览处理器组成受监管执行面。

没有选择 QuickLook 主体加 Rust Sidecar，因为它会形成两个会话所有者，核心故障边界仍留在原进程，未来还需要再次迁移。没有选择 Rust-first 全面重建，因为窗口、输入、DPI、媒体和旧插件兼容会同时成为首版阻塞项。

```text
Windows / Shell
      │
      ▼
Rust Broker ─────── 会话、路由、策略、监督、缓存
      │
      ▼
Unified Viewer Shell ── 窗口、输入、主题、DPI、导航
      │
      ├── DocumentModel ── 语义内容
      ├── SharedSurface ── 图像、视频、GPU 输出
      └── ExternalWindow ─ 旧插件与复杂兼容例外
              ▲
              │
   ┌──────────┼────────────┐
Legacy Host  Renderer Workers  System Preview Handler
```

PowerToys Peek 的预览器分工、工厂路由和系统 `IPreviewHandler` 回退方式是直接参考；PreviewIt 在此基础上增加更严格的进程监督、第三方 Renderer 信任分层和 QuickLook 插件兼容宿主。

## 职责与不变量

### 控制面：Rust Broker

Broker 唯一拥有预览会话、Shell 选择、路由策略、缓存索引和子进程监督。它管理请求取消、超时、资源限制、崩溃退避和降级，但不加载第三方插件、WPF 控件或文件解码器。

Broker 内部保持少量职责明确的模块：

- `ShellResolver` 识别 Explorer、桌面、传统文件对话框和通过兼容验证的第三方文件管理器，并取得当前选择。
- `SessionCoordinator` 维护唯一活动会话及状态机。
- `RendererRegistry` 读取 manifest，按文件类型、能力、优先级和信任等级产生候选。
- `RoutePlanner` 根据显式格式策略选择执行路径。已有可靠系统处理器时优先复用；被内置 Renderer 明确接管的格式可以使用内置实现作为主路径。
- `ProcessSupervisor` 使用 Job Object 管理进程、资源预算、取消、超时和崩溃退避。
- `CacheService` 管理有界、版本化、可丢弃缓存。
- `ProtocolEndpoint` 集中处理 Named Pipe、身份校验、版本和能力协商。

### 显示面：Unified Viewer Shell

Viewer 唯一拥有主预览窗口、输入、主题、DPI、导航、置顶和全屏状态。第一阶段继续使用 WPF；未来可以独立评估 WinUI 或其他实现，而不修改 Renderer 的核心输出契约。

`ExternalWindow` 只服务旧插件、系统组件和确实需要独立交互窗口的复杂内容。它是显式兼容例外，不发展为普通 Renderer 的默认集成方式。

### 执行面

- Legacy Host 长期承载 QuickLook `.NET/WPF IViewer` 插件，旧接口和 WPF 控件不跨进程暴露给 Rust。
- Renderer Worker 运行新的进程协议 Renderer。
- System Handler Host 优先复用 Windows `IPreviewHandler` 和系统隔离设施。
- x86 Dialog Adapter 只读取 32 位传统文件对话框选择，不渲染、不加载插件，也不拥有预览会话。

所有实现必须保持以下不变量：

1. 一个活动预览只对应一个由 Broker 管理的会话。
2. 所有异步结果都绑定 `request_id`；过期结果不能改变当前窗口。
3. 文件由 Broker 验证并以最小只读能力交给执行单元。
4. Renderer 崩溃最多导致当前预览失败，不能带走 Broker 或主窗口。
5. 旧 QuickLook 插件契约和新 Renderer SDK 是两个独立兼容层。
6. Viewer 技术可以替换，但窗口语义和 Renderer 输出契约保持稳定。

## 协议与数据流

控制协议使用长度分帧的 Protobuf，通过限制为当前用户的 Named Pipe 传输。Protobuf 只承载控制消息和有界语义数据；位图、视频帧等大块内容通过共享内存、DXGI Shared Handle 或受控临时产物传递。

一次预览按以下路径运行：

```text
Space
  → ShellResolver 取得选择
  → Broker 创建 request_id
  → RoutePlanner 选择候选
  → Broker 打开只读文件句柄
  → Supervisor 获取或启动 Worker
  → 协议和能力协商
  → Renderer 生成结果
  → Viewer 验证 request_id 后显示
```

Renderer 不依赖任意路径重新打开文件。Broker 校验文件身份后，以只读句柄或受控流传递内容；必须使用路径的系统组件作为显式例外，并由 Broker 限定适用范围。

会话使用 `Idle → Resolving → Preparing → Rendering → Ready → Closing` 状态机。新请求立即取消旧请求；响应、进度和错误都携带 `request_id`。取消超过预算后，Broker 终止对应 Worker，并在剩余预算内尝试下一个合理候选或显示统一错误状态。

Renderer 支持三类输出：

- `DocumentModel`：有界、版本化的语义树，适合文本、代码、压缩包目录和元数据。
- `SharedSurface`：共享位图或 GPU 表面，适合图片、视频帧和复杂栅格输出。
- `ExternalWindow`：Legacy Host、系统组件或复杂交互内容的兼容模式。

协议按 `major.minor + capabilities` 协商。主版本不兼容时拒绝连接；次版本通过能力集保持向后兼容。旧 QuickLook 插件继续使用 Legacy Host 内部契约，不暴露到新协议。

## Renderer 生命周期与信任

Renderer 使用自适应受监管 Worker。每个 Renderer 包和安全边界使用独立进程；高频、低风险 Renderer 可以复用热进程，并在空闲 TTL 后回收；高风险解码器或高成本任务采用单请求临时进程。Manifest 可以声明成本、并发和隔离提示，但 Broker 保留最终调度权。

Renderer 分为三个信任层级：

- 随产品发布且签名的 Renderer 可以使用声明并审核过的完整能力。
- 签名第三方 Renderer 默认使用受限权限和资源配额。
- 未签名 Renderer 只在开发者模式启用，并明确显示来源。

Broker 只传递最小权限的只读句柄。Renderer 默认无网络、不能任意创建子进程，也不能写安装目录。Job Object 只负责生命周期和资源治理，不被描述成安全沙箱；第三方 Renderer 还需要受限令牌，并在兼容时使用 AppContainer。Legacy 插件始终留在 Legacy Host，不因历史兼容而取得 Broker 权限。

## 故障、降级与缓存

Broker 分别限制启动、探测、首帧、总渲染和取消回收时间。错误统一归类为不支持、文件损坏、权限不足、依赖缺失、超时、崩溃和协议错误，Viewer 只负责一致呈现。

RoutePlanner 为每种格式建立有界候选链，通常由策略选定的首选实现开始，再按条件使用系统处理器、Legacy 插件和通用文本或元数据回退。同一请求不能无限重试。连续崩溃的 Renderer 进入退避或本次运行期隔离；Broker 和 Viewer 不随 Worker 重启。

Named Pipe 限制为当前用户并验证连接进程身份。临时产物使用私有目录、原子创建和确定性清理。日志默认不记录文件内容，路径按诊断级别脱敏。

缓存由 Broker 统一管理，键至少包含文件身份、修改时间和大小、Renderer ID 和版本以及输出参数。缓存必须有容量上限、TTL 和版本失效规则。Renderer 只能维护进程内临时缓存。网络位置、临时文件、加密文件和隐私敏感内容默认不落持久缓存；首阶段只缓存元数据、缩略结果和明确安全的转换产物。

## Renderer SDK 与升级

Renderer 生态分两阶段开放：

- 内部 v0 只服务内置 Renderer，允许协议和 `DocumentModel` 根据真实文件集调整。
- 公共 v1 在 manifest、协议、示例、兼容测试套件和诊断工具稳定后开放第三方接入。

SDK 是语言无关的进程协议工具包，不是 Rust 动态 ABI。Manifest 至少声明 Renderer ID 和版本、协议范围、扩展名或 MIME、探测能力、输出类型、架构和入口程序、信任来源以及成本、并发和隔离提示。

Renderer 包在安装前执行静态校验和兼容测试，并采用原子安装。升级失败时保留上一可用版本；新旧版本只在受控升级窗口内并存。Broker 根据兼容范围选择版本，崩溃率或协议错误超过阈值时停用新版本并回退。Legacy 插件版本策略独立维护。

公共 v1 的发布门槛是内置图片、文本和压缩包 Renderer 已经使用相同协议运行，并积累真实升级、取消和崩溃恢复证据，而不是接口表面完整。

## 迁移顺序

迁移使用可逐阶段回退的绞杀模式：

1. 固定 QuickLook 4.5.0 源码、行为、插件和性能基线。
2. 由 Rust Broker 接管单实例、热键、Shell 选择、会话和 x86 Dialog Adapter，内容暂时仍走原 WPF 路径。
3. 通过新协议拆出 Viewer 与 Legacy Host，保持旧插件行为。
4. 加入 Supervisor、系统 `IPreviewHandler` 和统一降级链。
5. 依次把图片、文本、压缩包迁移到内部 Renderer v0。
6. 兼容套件稳定后发布 Renderer SDK v1；Viewer 技术替换另立事项评估。

每一阶段都必须保留上一条已验证路径作为短期回退，不能在同一批次同时替换控制面、显示面和某个复杂格式实现。

## 验证与性能门槛

行为验证覆盖 Explorer、桌面、传统文件对话框、首个 Space、快速切换、置顶、全屏、DPI 和多显示器，并使用固定的内置插件与 17 个用户插件样本。故障测试覆盖 Renderer 崩溃、卡死、超内存、协议损坏、过期响应、取消竞争、损坏或超大文件、网络位置以及预览中被修改的文件。发布测试还覆盖升级失败、版本不兼容、缓存失效和回滚。

首轮性能门槛使用相对指标：

- Legacy 热路径中位延迟不得劣于当前约 165 ms 的基线，P95 回归控制在 15% 内。
- 图片和文本新 Renderer 的热路径目标至少改善 20%。
- Worker 回收后，稳定空闲私有内存目标比当前约 122 MiB 降低 50% 以上。
- 单个 Renderer 崩溃时 Broker 和 Viewer 保持可用，下一次预览无需重启应用。

所有指标必须在同一机器、同一文件集上分别记录冷启动和热状态。故障隔离、恢复能力和兼容性优先于单项微基准。

## 不在本设计范围内

- 完全移除 WPF 或一次性迁移全部旧插件。
- 一次性重写 Office、PDF、视频、网页和全部 RAW 解码器。
- 完整 x86 主程序或 32 位 Windows 支持。
- ARM64 Broker、Viewer、Legacy Host、Renderer 或安装包。
- 在协议与执行边界稳定前替换 Viewer UI 技术。
- 永久支持所有历史 Renderer 协议版本。

## 仍需在实施事项中确定

- 首批图片、文本和压缩包 Renderer 的最小格式集合与验收语料。
- Protobuf v0 的具体消息字段、尺寸上限和超时默认值。
- 不同 Renderer 在受限令牌或 AppContainer 下的兼容清单。
- 缓存容量、TTL 和热 Worker 空闲 TTL 的基准测试结果。
