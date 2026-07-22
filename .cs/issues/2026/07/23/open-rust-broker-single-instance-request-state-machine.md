---
kind: issue
title: "Rust Broker single-instance and request state machine"
type: feature
status: open
created: 2026-07-23
epic: ".cs/epics/2026/07/20/rust-hybrid-preview-architecture/spec.md"
---

# Rust Broker single-instance and request state machine

## 目标

在每个交互式用户会话中只运行一个 x64 Rust Broker。后续启动的 Broker 不再建立第二套控制面，而是通过当前用户的受限控制通道把一个有界命令转发给已运行的实例并退出；主实例为被接受的命令生成 `request_id`，按统一会话状态机处理新请求、取消和过期事件。

这个 Issue 的完成标志是：两个或多个并发启动只产生一个主实例，次实例可以收到明确的接受或拒绝结果；连续请求会取消旧请求，旧请求的异步结果不会改变当前状态；主 Broker 在 malformed、oversized、duplicate、timeout 或旧事件出现时仍保持可用。

## 范围

- 包含：
  - 每个交互式用户会话的 deterministic instance lease 和崩溃后重新选主。
  - 当前用户 + SYSTEM DACL、local-only Named Pipe 上的 Broker command endpoint、有限连接/请求队列和确认响应。
  - 最小 typed command（`OpenPath` 和 `Close`），并把路径只作为 Broker 控制面的输入；执行单元仍只接收 Broker 校验后的最小能力。
  - `Idle -> Resolving -> Preparing -> Rendering -> Ready -> Closing` 状态机、`request_id` 生成、取消旧请求、stale event 丢弃和 bounded duplicate command 去重。
  - 纯状态 reducer 的单元测试，以及两个 Broker 进程之间的 Windows 集成测试。
- 不包含：
  - 全局键盘钩子、Shell Resolver、x86 Dialog Adapter、Viewer/WPF 窗口或真实 Renderer 接入；这些由后续 Issue 负责。
  - Worker pool、缓存、系统 `IPreviewHandler`、完整 CLI 兼容命令目录和跨会话转发。
  - 32 位 Broker、ARM64 target、ARM64 产物或 ARM64 行为承诺。

## 归属

- 隶属 Epic：`.cs/epics/2026/07/20/rust-hybrid-preview-architecture/spec.md`。
- 相关 spec：Epic 的“协议和会话”“插件与安全边界”以及长期架构设计的迁移第 2 阶段。
- 前置证据：`.cs/issues/2026/07/22/closed-preview-foundation-vertical-slice.md` 已验证 x64 Rust/.NET framing、当前用户 pipe、`request_id` stale rejection 和单 Worker 回收。

## 背景与证据

- 当前 `previewit-broker` 的 `main` 仍是 integration probe；`WorkerSupervisor` 只拥有一个测试 Worker 的生命周期和 `current_request_id`，尚无 Broker 自身的选主或请求入口。
- QuickLook `4.5.0` 的 `App` 用 `Mutex(true, "QuickLook.App.Mutex")` 选出首实例，次实例把文件命令写入 `PipeServerManager`；旧 pipe 使用未分帧的 `message|path|options`，并在 Dispatcher 上取消上一项待处理操作。这是要保留的用户语义，不是新边界的实现契约。
- PowerToys 的 `appMutex.h`/Runner 使用 `Local\\...` 命名 Mutex 选主；Tauri single-instance Windows 插件使用 Mutex 加隐藏窗口消息转发。两者证明选主应在初始化控制面之前发生；本项目选择 Named Pipe 是因为 foundation 已有 Protobuf、长度上限、DACL 和确认响应，不让 Broker 控制命令依赖 UI HWND 或字符串拼接。
- 长期架构设计规定一个活动预览只对应一个 Broker 会话，所有异步结果带 `request_id`，新请求取消旧请求，旧结果不能改变当前窗口。Foundation 只验证了这些规则在 Worker 边界的最小部分，本 Issue 把它们提升到 Broker 会话入口。

## 待确认问题

- 没有阻塞本 Issue 的产品分叉。会话级选主、命令集合和状态阶段沿用已批准的 Epic 口径；Shell 取得真实选择、Viewer 如何呈现状态、以及完整 CLI 兼容目录留给对应后续 Issue。

## 现状如何工作

当前只有测试调用方直接创建随机 Worker pipe：测试进程启动 WorkerProbe，核对子进程 PID，协商 `0.1`，再把请求交给 `WorkerSupervisor`；没有一个常驻 Broker 接收第二次启动或多个用户请求。

QuickLook 的旧路径是：进程启动时先取得 Mutex；首实例启动监听和 UI，次实例把 `Toggle`/`Close` 等字符串命令写入按用户命名的 pipe 后退出；pipe 服务在 UI Dispatcher 上取消上一项 pending operation，再调用 `ViewWindowManager`。旧路径没有统一的 `request_id`、帧上限或 stale response 规则。

## 影响范围

- 必须修改：
  - Broker 启动入口和 Windows instance lease；必须在创建热键、Shell、Viewer 或 Worker 控制面前完成选主。
  - Broker 控制命令的 Protobuf schema、pipe server/client、确认错误和有限队列。
  - 会话 reducer 及其与当前 WorkerSupervisor 的边界，使状态所有权集中在 Broker 事件循环而不是 pipe 线程。
  - Rust Windows 集成测试、测试启动器和日志/指标中的 instance、request、stale、queue-full 结果。
- 需要验证：
  - 并发启动竞态、主实例初始化期间的次实例重试、主实例崩溃后的重新选主、重复 command 的幂等确认。
  - pipe 的 current-user + SYSTEM DACL、拒绝远程客户端、1 MiB framing、UTF-16LE/path 边界和协议 major mismatch。
  - `request_id` 在每个异步事件上的传播；新请求取消旧请求后，旧的 resolve/prepare/render/cancel 完成事件只能产生诊断记录，不能改写当前状态。
  - foundation 的 x64-only 工具链和 PE 架构检查仍然通过，且不出现 ARM64 target 或产物。
- 仍待调查：
  - Shell Resolver 如何把窗口选择转换为 `OpenPath` 之前的受控输入，以及 x86 Dialog Adapter 的失败/回收语义。
  - Viewer 的可见错误、进度和 `Ready` 消费方式；本 Issue 只定义 Broker 内部可观察事件和确认响应。
  - 完整的 QuickLook `Toggle`、`RunAndClose`、`Forget`、`Fullscreen`、插件调用等兼容命令何时进入稳定协议。

## 方案判断

### 选定：会话级 Mutex + 当前用户 Named Pipe

Broker 先以 current-user + SYSTEM 安全描述符创建 `Local\\PreviewIt.Broker.<session-id>` Mutex，并用零等待判断所有权。`WAIT_OBJECT_0` 或 `WAIT_ABANDONED` 的进程成为主实例并持有 lease；`WAIT_TIMEOUT` 的进程只作为 command client 运行，不初始化控制面。主实例随后监听确定的 pipe 名称（名称由产品标识和会话标识构成，不把随机 nonce 当成发现机制），每个连接只承载一个有界 Protobuf command 和一个 ack。次实例在 startup deadline 内重试 pipe；如果旧 owner 在此期间退出，它重新竞争 lease，取得所有权后把自己的启动命令作为主实例首个请求处理。

Pipe 使用 foundation 已验证的 framing、1 MiB 上限、拒绝远程客户端和 current-user + SYSTEM DACL。控制命令的安全含义是“同一用户会话请求 Broker 做事”，不是把未经校验的路径交给 Worker；Broker 仍负责路径存在性、文件身份和只读能力。主实例内部用单一事件循环消费 bounded command queue，避免多个 pipe 线程同时修改会话状态。

### 不选的替代方案

- **Mutex + hidden HWND/`WM_COPYDATA`**：PowerToys 和 Tauri 证明它可用，但把命令接收绑到 UI 消息循环，数据边界和错误确认也容易退化为字符串；PreviewIt 的控制面必须在 Viewer 之前可运行，因此不采用。
- **仅依赖 pipe 首个监听者**：没有独立 lease 时，两个进程会在 endpoint 创建、崩溃重启和旧 pipe 清理之间产生选主竞态；也无法把“主实例仍在初始化”与“无人持有控制面”区分开。
- **文件锁或 PID 文件**：Windows 崩溃、重启和多会话清理会留下 stale 文件，且需要自行补充权限和存活校验；内核 Mutex 已提供生命周期语义。

## 实现设计

### 这次要怎么做

把 Broker 启动拆成两个明确阶段。第一阶段只完成 session-scoped lease：拿到 lease 才能继续初始化；拿不到 lease 的进程连接确定的 command pipe，发送一次 typed command，等待有界 ack 后退出。第二阶段由持有 lease 的主实例启动 pipe listener 和一个会话事件循环；listener 只负责验证 frame、解码和把 command 放进 bounded queue，状态 reducer 是唯一可以改变当前会话的模块。

状态 reducer 不直接调用 Windows API 或 Worker。它接收带 `request_id` 的事件，输出新的 phase 和有限 effect；事件循环执行 effect 并把完成事件送回 reducer。这样状态转换可以在不启动进程、不依赖窗口和不伪造时间的情况下测试，同时保留真实 pipe、Mutex 和 Worker 作为集成边界。

### 功能怎么分工

- **Instance lease**：封装 Mutex 的安全描述符、创建、`WaitForSingleObject(0)` 结果、lease 生命周期和会话命名。它不负责发送命令，也不启动 Broker 子系统；Drop 顺序保证 endpoint 和事件循环先停、lease 最后释放。
- **Command endpoint**：封装 pipe 安全属性、单连接读取、Protobuf framing、协议版本/长度校验、ack 和 bounded queue。它只把合法 command 交给会话事件循环，不直接改变 `SessionPhase`。
- **Command router**：把 `OpenPath`、`Close` typed command 映射为 session event；用有界的 `command_id -> ack/request_id` 去重表处理 client 重试，超出容量返回 `queue-full`，不静默丢弃。`OpenPath` 使用 UTF-16LE bytes 保留 Windows `OsString`，边界层拒绝奇数字节、embedded NUL 和超过 Win32 长路径上限的输入。
- **Session reducer**：唯一拥有 `Idle -> Resolving -> Preparing -> Rendering -> Ready -> Closing` 的状态。活动请求存在时，新请求进入 `Closing { old, next }`，只保留最新 `next` 并为旧请求发出一次 cancel；旧资源 cleanup 完成后才把 `next` 提升到 `Resolving`。任何不匹配当前活动/closing 请求的完成事件只产生 stale 诊断，不产生状态变化。
- **Effect runner**：在现有 WorkerSupervisor 和未来 Shell/Viewer/Renderer adapter 之间执行 reducer effect。这个 Issue 只接入可测试的最小 fake effect runner 和 Broker 存活检查，不把后续生产组件提前塞进 Broker。

### 请求 / 数据怎么走

```text
second Broker process
  -> acquire existing session lease (fails)
  -> current-user command pipe
  -> bounded Protobuf command + command_id
  -> primary validates frame, version and dedupe entry
  -> ack(accepted/rejected, request_id when accepted)
  -> bounded command queue
  -> Session reducer
  -> cancel(old request_id) + Closing { old, next }
  -> cleanup(old request_id)
  -> Resolving(next request_id)
  -> effect runner emits phase completion/error
  -> reducer accepts only the active/closing request_id
```

`request_id` 由主 Broker 生成，不由第二进程决定；`command_id` 只用于转发确认幂等。Broker control request/response 使用独立的 Protobuf message，复用 v0 版本规则和 framing，但不复用要求 `request_id` 的 Worker `Envelope`。接受 command 的 ack 表示请求已进入 Broker 会话，不表示已经 `Ready`。如果命令在输入校验、队列容量或协议版本阶段被拒绝，ack 携带稳定错误代码，次实例退出非零；如果渲染后来失败，错误作为带 `request_id` 的异步 session event 记录并走 `Closing -> Idle`，不会让次实例重新提交同一命令。

状态转换的核心不变量：

- `Idle` 只接受新请求或无操作的 `Close`；新请求创建 active `request_id` 并进入 `Resolving`。
- 在任一活动阶段收到新请求时，进入 `Closing { old, next }` 并只为 old 产生一次 cancel effect；`Closing` 期间的新请求替换 `next`，被替换且尚未启动的 pending request 只产生 `superseded` 结果。
- old cleanup 完成后，有 `next` 就进入它的 `Resolving`，没有则进入 `Idle`；因此旧 Worker 和新请求不会同时拥有活动执行资源。
- `Resolving -> Preparing -> Rendering -> Ready` 只接受对应 active `request_id` 的成功事件。
- `Close` 或当前请求失败进入不带 `next` 的 `Closing`；旧 cleanup 或过期结果不能关闭、推进或覆盖新的请求。
- 过期、重复或未知事件不改变 phase；日志包含 event kind、actual/expected request id 和 reason，但不记录完整文件路径。

### 哪些边界不碰

- 只支持 Windows x64；Rust toolchain 继续只配置 `x86_64-pc-windows-msvc`，不引入 ARM64 target、条件编译或安装产物。
- 只实现当前交互式会话内的单实例；不为同一用户的其他 Terminal Services 会话转发命令，也不把 `Global\\` 对象暴露给跨会话客户端。
- pipe 的 current-user DACL 是访问边界，不是完整沙箱；同一用户权限下的命令仍须做输入校验和队列限制。Worker 仍不能通过路径重新打开预览文件。
- 不在本 Issue 接管热键、Shell 选择、Viewer 窗口、旧插件、Renderer 路由或安装/更新；它们只能通过未来的 adapter 消费已定义的 Broker session events。
- 不用全局可变 singleton 让 pipe 线程直接修改状态，不以 sleep/retry 代替明确的 lease、ack 和 deadline。

### 设计侧重点

- **可靠性**：Mutex 是主实例所有权，pipe connect/ack、queue 和 duplicate cache 都有上限；主实例启动失败或崩溃后，等待者通过 signaled/abandoned Mutex 重新选主，不依赖 stale PID 文件；`Closing` 串行交接防止旧执行资源与新请求并存。
- **可测试性**：reducer 是纯输入/输出模块；测试通过 command/event 接口检查 phase、effect 和 stale 行为，Windows 集成测试只覆盖真实 Mutex、pipe 和多进程竞态。
- **安全性**：命令必须通过 current-user + SYSTEM DACL、local-only pipe、major/version、frame size 和路径输入校验；命令 pipe 只请求 Broker 行为，不传递可被 Worker 直接使用的句柄或未审查路径。
- **可观测性**：ack 错误码、phase transition、queue-full、duplicate、lease-lost 和 stale event 使用稳定事件名；日志只带 request/command id 和经过归一化的原因，避免把文件路径作为常规诊断内容。

### 一步步怎么改

1. 在协议中增加独立的最小 `BrokerControlRequest`/`BrokerControlResponse` message，复用 v0 的版本规则和 framing，但不复用 Worker `Envelope`；固定 UTF-16LE path、字段上限、稳定错误代码和 `command_id` 语义。
2. 在 Broker 内实现 session-scoped `InstanceLease` 和确定命名的 command pipe。先在测试启动器中证明并发选主、初始化竞态、崩溃重启和 DACL/remote rejection，再接入 `main`。
3. 实现纯 `SessionReducer` 与 effect 列表，覆盖全部 phase、`Closing { old, next }` 的 latest-wins 合并、cancel 一次性语义、cleanup 和 stale event；用可控 fake effect runner 完成单元测试。
4. 实现 primary/secondary command router：输入校验、bounded queue、bounded duplicate cache、ack deadline 和非零失败退出；把 `OpenPath` 先保留为控制面输入，不连接 Shell 或 Renderer。
5. 把最小事件循环接到现有 Broker probe/WorkerSupervisor 的存活边界，验证 primary 不因 secondary malformed command、Worker crash 或 stale event 退出；记录证据后再拆下一个 Shell/Viewer Issue。

### 怎么确认做对

- Reducer 单元测试：覆盖所有合法 phase 转换、无效转换、快速 request replacement、cancel/cleanup 竞态、stale result、重复 event 和 `Close` 幂等。
- Windows instance 集成测试：并发启动至少 10 个 x64 Broker，断言恰有一个 lease owner；所有 secondary 收到 ack 并退出，owner 在整个测试中保持存活。
- Recovery 集成测试：杀掉 owner 后重新启动，断言新进程取得 lease、重建 endpoint，旧 command 不会被重复执行；主实例初始化尚未完成时，secondary 在 startup deadline 内重试，lease 仍被持有则返回稳定 `primary-not-ready`，lease 变为 signaled/abandoned 则由一个等待者接管并处理自己的启动命令。
- Protocol/security 测试：wrong major、oversized frame、invalid UTF-16LE/path、queue full、duplicate `command_id`、远程连接拒绝和当前用户 pipe 访问均有稳定错误结果。
- Request integration：连续提交 `request-1`、`request-2`，断言先进入 `Closing { request-1, request-2 }`，`request-1` cleanup 后才启动 `request-2`，且只有 `request-2` 能推进到 `Ready`；迟到的 `request-1` 完成不能改变 phase。Broker/WorkerProbe 仍须报告 `8664 machine (x64)`，Rust target 列表不得出现 ARM64。
- 运行方式：新增测试纳入 `tools/test-foundation.ps1`，并保留现有 Rust fmt/Clippy、.NET parity、legacy build 和 foundation gate；本 Issue 不以 GitHub-hosted run 作为本地关闭前提。

## 验证

- `cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test instance_lease`
- `cargo test --manifest-path src/rust/Cargo.toml -p previewit-broker --test request_state_machine`
- `cargo fmt --manifest-path src/rust/Cargo.toml --all -- --check`
- `cargo clippy --manifest-path src/rust/Cargo.toml --workspace --all-targets -- -D warnings`
- `pwsh -NoProfile -File tools/test-foundation.ps1`
- 复核 Broker/WorkerProbe PE 为 x64，且 `rust-toolchain.toml` 仅包含 `x86_64-pc-windows-msvc`。

## 执行记录

- 设计阶段：基于 QuickLook `4.5.0` 的 Mutex/pipe 实现、foundation vertical slice 的 pipe/supervisor 证据、PowerToys `appMutex.h`/Runner 和 Tauri Windows single-instance 插件完成方案比较。
- 尚未实现代码或新增协议字段；本 Issue 关闭前必须补充实际命令、测试输出和偏差说明。

## 关闭回写

- epic spec：回写已验证的会话选主、命令确认、状态不变量和 stale 事件边界；Epic 保持 `draft`，不因本 Issue 完成自动关闭。
- project spec：只有在真实热键、Shell、Viewer 路径验证后，才把稳定用户路径毕业到 Project Spec。
- notes/tools：若多进程竞态或 Windows pipe ACL 形成可复用操作经验，再另行沉淀；不在设计阶段复制成 notes。

## 关闭结论

- 关闭判断：待实现和验证完成后填写。
- 验证摘要：待实现和验证完成后填写。
- 回写位置：待实现和验证完成后填写。
- 遗留事项：Shell Resolver、x86 Dialog Adapter、Viewer/Legacy Host、Renderer 和完整 CLI 命令目录不属于本 Issue。
