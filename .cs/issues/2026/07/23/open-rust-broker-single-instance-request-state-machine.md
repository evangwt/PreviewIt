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

当前 Broker 已通过 session-scoped Mutex 在控制面初始化前完成选主。主实例创建确定命名的 current-user/SYSTEM Named Pipe，由 listener thread 串行接受、读取并验证一个 Protobuf command，再把合法请求连同 pipe handle 放入容量为 8 的 `sync_channel`；`main` 的单一循环调用 `CommandRouter`/`SessionReducer` 并写回 ack。次实例连接 endpoint、发送一次命令并按 `accepted` 决定退出码；连接超时后只重新检查一次 lease。

这条路径已经证明并发选主、crash takeover、framing、基础输入校验和 reducer 行为，但 review 发现四个责任分歧。第一，`pipe.rs` 自己管理 Overlapped I/O，超时取消后只保留 `OVERLAPPED`/event，没有同时保留仍被内核引用的 buffer，存在 use-after-free 风险。第二，listener 在完整 decode 后才创建下一 pipe instance，一个慢客户端会占住唯一入口；Drop 只设置停止位，不 join listener。第三，request/response 的版本、ID、shape 和错误码规则分散在 `command.rs`、`router.rs`、`main.rs`，router 还会重新解码路径。第四，router 在唯一状态线程同步执行 `Path::exists()`，而 ack、phase/stale/duplicate/queue 观测没有共同事件模型。

经用户确认，后续统一采用这一语义：`accepted=true` 只表示命令已经进入 Broker session，不表示路径存在或预览成功。UTF-16LE、NUL、长度和 absolute path 属于同步控制边界；存在性、访问权限和文件身份属于 `Resolving`，通过带 `request_id` 的异步成功/失败事件推进状态。

## 影响范围

- 必须修改：
  - `pipe.rs` 和 command endpoint：用同一套成熟的 Tokio Windows Named Pipe/framing 实现替换手写 Overlapped read/write/cancel，保留现有 DACL、local-only、PID 验证和 deadline。
  - Broker control contract：集中 wire request/response 转换、protocol version、`command_id`、ack shape 和稳定错误码；队列内部只传 `ValidatedCommand`，不传原始 Protobuf。
  - endpoint/runtime 生命周期：连接始终由 endpoint task 拥有，通过 reply handle 与唯一状态线程交互；shutdown 必须停止 accept、取消/等待连接任务、join endpoint，最后才释放 lease。
  - router/runtime：删除同步文件系统访问和重复路径解码；增加统一 `BrokerEvent`，明确 accepted/rejected、busy/full、duplicate、phase、stale、failure 和 shutdown。
- 需要验证：
  - 既有 Worker handshake、handle transfer、supervision、十进程选主、crash takeover 和 duplicate/reducer 行为在 transport 迁移后保持不变。
  - 单个慢/部分 frame 客户端不能阻断正常 secondary；连接槽耗尽、已解码队列满和 endpoint 尚未创建使用不同稳定语义。
  - delayed broken pipe 归一化、伪造/串线 response 拒绝、Drop 后同名 endpoint 立即重建，以及 shutdown/lease 的真实顺序。
  - current-user + SYSTEM DACL 的 ACE、remote-rejection flag、当前用户真实连接、1 MiB framing 和不记录完整路径。
  - foundation 仍只安装/生成 `x86_64-pc-windows-msvc`，PE 仍为 `8664 machine (x64)`，不引入 ARM64 target、产物或承诺。
- 仍待调查：
  - 后续 Shell Resolver 如何有界地完成存在性、访问权限和文件身份解析，以及 x86 Dialog Adapter 的失败/回收语义；本 Issue 只固定事件和 ack 边界，不提前实现 Resolver。
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

保留已经成立的 Mutex 选主、单状态所有者和 reducer，不围绕八个 review finding 各打一层补丁。把根因收敛为四个深模块：共享 framed Named Pipe、Broker control contract、有界 command endpoint、唯一 Broker runtime。旧的手写 Overlapped read/write/cancel 被成熟的 Tokio Windows Named Pipe 完整替换，不保留平行 I/O 路径；Protobuf 只存在于 endpoint 边界，内部只传已经验证的 command 和 ack。

主实例仍在任何其他控制面之前取得 lease，但 runtime 消费 lease 并拥有 endpoint，使 shutdown 顺序成为模块不变量。secondary 仍发送一次命令并等待有界响应；response 必须通过与 request 相同的 contract 验证后才能决定退出码。

### 功能怎么分工

- **共享 pipe transport**：`pipe.rs` 使用 Tokio `NamedPipeServer`/`NamedPipeClient`，在一个位置实现 4-byte LE framing、1 MiB 上限、timeout 和 Windows error 归一化。Worker pipe 与 command pipe 都调用它；current-user/SYSTEM `SECURITY_ATTRIBUTES`、`reject_remote_clients`、`first_pipe_instance` 和 `max_instances` 保持现有安全含义。
- **Broker control contract**：集中拥有 `0.1` 常量、wire-to-domain 转换、`CommandId`、`ValidatedCommand`、`CommandAck`、response shape 和错误码。校验顺序固定为 frame、Protobuf、command ID、version、command、路径结构；非法 ID 在任何错误响应中都不得回显。
- **Command endpoint**：每个 task 从 accept 到 response 始终拥有自己的连接，accept 后先补下一个 listener，再有界 decode。合法请求以 `PendingCommand { command, reply }` 进入状态队列；endpoint 不接触 reducer，状态线程不接触 pipe handle。endpoint 拥有 shutdown signal、连接任务和 join handle，Drop 返回前必须完成停止与 join。
- **Broker runtime**：消费 `InstanceLease`、endpoint、router 和 event sink，是唯一状态所有者。它路由 `ValidatedCommand`、通过 reply 返回 `CommandAck`、执行最小 effect runner，并在退出时先停 endpoint、最后释放 lease。
- **Command router / Session reducer**：router 只做纯领域映射、request ID 生成和 duplicate cache；不再解码 Protobuf，也不调用文件系统。reducer 的 state/effect 不变量保持不变。
- **统一观测**：一个 typed `BrokerEvent` 和一个真实 `EventSink` 接缝同时服务生产日志与测试 recorder。事件覆盖 instance、endpoint、command、session transition、duplicate、queue、stale 和 failure；不存在真实状态变化的 `lease-lost` 从契约删除，以 `lease-released`/`takeover` 表达实际行为。

### 请求 / 数据怎么走

```text
second Broker process
  -> acquire existing session lease (fails)
  -> Tokio current-user command pipe
  -> BrokerControlRequest protobuf
  -> BrokerControlContract -> ValidatedCommand
  -> bounded PendingCommand { command, reply }
  -> BrokerRuntime -> CommandRouter -> SessionReducer
  -> CommandAck -> reply -> endpoint
  -> BrokerControlContract validates/encodes response
  -> secondary accepts exit status only after response validation
```

`request_id` 仍由主 Broker 生成，`command_id` 只用于传输幂等。`accepted=true` 的唯一含义是 command 已进入 session：Open 返回非空 `request_id`，Close 可以为空；accepted 不带 error code。同步 rejection 表示 command 未进入 session，必须带稳定 reason 且不带 request ID。路径不存在、拒绝访问或后续解析失败属于 `Resolving` 的异步 session failure，不反向改变已经返回的 ack。

容量语义分开命名并从一个内部配置推导：queued commands、active routed command、concurrent decoding connections 和 listener reserve。endpoint 从未出现返回 `primary-not-ready`；endpoint 存在但连接槽耗尽返回 `primary-busy`；命令已解码但状态队列已满返回 `queue-full`。同一数值不再同时代表 queue、decoder 和 Win32 pipe instance。

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
- 不在本 Issue 实现真实 Shell Resolver；只把 path existence/access/identity 明确移入 `Resolving` 的未来 effect 边界。
- 不用全局可变 singleton 让 pipe task 直接修改状态，不以 sleep/retry 代替明确的 lease、ack、capacity 和 deadline。
- 不保留手写 Overlapped transport 作为 fallback，也不再制造第二套 framing、版本常量、ack factory 或日志字符串表。

### 设计侧重点

- **可靠性**：用成熟 runtime 管理异步 I/O 生命周期；一个慢连接不能占住唯一 listener，所有连接/queue/deadline 都有独立上限；显式 shutdown/join 后才释放 lease。
- **可维护性**：wire/domain、framing、版本、ack shape、错误码和事件名各只有一个权威实现；旧 helper 被替换而不是叠加。
- **安全性**：迁移不改变 current-user + SYSTEM DACL、local-only、非继承 handle、frame 上限或 Worker PID 验证；结构校验仍在 trust boundary，资源解析留在 Broker session 内。
- **可测试性与可观测性**：纯 contract/router/reducer 通过公开领域接口测试；真实 Windows 集成只验证 pipe、ACL、deadline、lifecycle 和进程竞态；生产与测试消费同一 `BrokerEvent`。

### 一步步怎么改

1. 先以测试固定 `BrokerControlContract`、response validation 和新的 accepted/path 语义，再让 router 只接收 `ValidatedCommand`。
2. 引入 Tokio Windows Named Pipe，把共享 frame I/O 和 Worker `PipeServer` 迁移到一个 transport，删除手写 Overlapped read/write/cancel。
3. 用 endpoint-owned connection task 和 reply handle 重建 command server/client，加入有界 decode、独立 capacity、显式 shutdown/join 和响应一致性校验。
4. 引入最小 `BrokerRuntime`/`BrokerEvent`，把 main 收成启动、运行和退出；删除 router 的 `Path::exists()`，记录 duplicate、queue、phase、stale 和 failure。
5. 补齐 DACL/remote flag、slow client、delayed broken pipe、graceful same-name takeover、forged response、capacity 和 x64 回归证据，再运行完整 foundation gate。

### 怎么确认做对

- Contract：wrong/mismatched version、非法或超长 ID、mismatched response ID、accepted/rejected shape、结构化路径和错误优先级都有表驱动测试；非法 ID 不出现在响应。
- Transport：延迟部分 frame 后断开稳定为 `truncated-frame`；一个慢客户端存在时正常 secondary 仍成功；所有 decode 槽耗尽时稳定 `primary-busy` 且资源有界；decoded queue 满返回 `queue-full`。
- Lifecycle：Drop server 后立即用同名 product ID 重建成功；graceful shutdown 先产生 endpoint-stopped/lease-released，再允许新 owner；crash takeover 仍由 abandoned/signaled Mutex 完成。
- Runtime：router 测试不创建文件 fixture、不访问文件系统；absolute missing path 获得 accepted/request ID 并停在 `Resolving`，未来 `Failed(request_id)` 才进入 cleanup；duplicate/phase/stale 通过同一 event recorder 观察。
- Security：读取真实 pipe security descriptor，ACE 只包含 SYSTEM 和当前用户；读取 pipe flags 确认 remote rejection；当前用户真实 client 可以完成 round trip。
- 回归：Worker handshake、handle transfer、supervision、protocol parity、十进程选主和 500 次 command round trip 继续通过；`tools/test-foundation.ps1` 退出 0，PE 为 x64，installed targets 与仓库配置不含 ARM64。

## 已有验证与失效证据

以下记录描述 `7e62a7b` 的既有通过面，不是关闭证据。2026-07-23 review 已证明其中部分错误语义和生命周期结论需要由上述实现替换后重新验证。

- 正向 command round trip 额外重复 500/500，通过；因此没有把 response handle 立即关闭视为已复现故障。
- delayed partial-frame client 在 read pending 后断开，真实 Broker 输出 `error_code=transport-error`，证明同步断管测试没有覆盖 `GetOverlappedResult(ReadFile)` 的错误归一化。
- 一个 partial-frame client 占住 listener 时，健康 primary 下的正常 `--close` secondary 在 527 ms 后以 `primary-not-ready` 退出 1，证明 pre-decode connection 没有被 queue/capacity 保护。
- `wait_for_overlapped` 的 cancellation grace 只保留 `OVERLAPPED` 和 event，没有保留仍被内核引用的 read/write buffer；这项内存安全问题由代码所有权直接证明，不以现有测试通过抵消。
- `BrokerCommandServer::Drop` 只设置 stop flag，不保存/join listener；`Path::exists()` 仍在唯一状态线程；client 只 decode response，不验证 version、command ID 或 ack shape。

- 完整 gate：`pwsh -NoProfile -File tools/test-foundation.ps1` 退出 `0`。实际出现 `QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c`、`LEGACY_BUILD_OK`、`FOUNDATION_STEP=broker-single-instance`、`FOUNDATION_GATE_OK`；rustfmt、workspace Clippy、Release x64 Worker build、显式 Broker build、legacy build 和 .NET build 均成功。
- Rust workspace：Broker 46 个测试与 protocol 5 个测试全部通过。Broker 明细为 command raw/deadline 6、command transport 10、single-instance process 4、router 6、Worker handshake 5、handle transfer 2、instance lease 4、request reducer 6、supervision/stale 3；无失败、忽略或遗留子进程。
- .NET protocol parity：`dotnet test tests/dotnet/PreviewIt.Protocol.Tests/PreviewIt.Protocol.Tests.csproj -c Release` 共 6 个测试全部通过；Broker control 与 Worker envelope 的 `0.1` round trip 保持一致。
- 竞态稳定性：`full_pending_queue_returns_stable_rejection` 重复 10/10；truncated-frame 提前断管竞态重复 100/100；十进程选主和 crash takeover 场景重复 10/10。每轮都只有一个 primary 存活、九个 secondary 收到 accepted ack 并成功退出，kill primary 后新进程取得同一 session lease。
- 输入/超时证据：wrong major、oversized/truncated frame、malformed Protobuf、缺失/超长 command ID、奇数 UTF-16LE、embedded NUL、超长路径分别返回稳定码；容量 8 的 pending queue 对第 9 个等待命令返回 `queue-full`；无 endpoint 返回 `primary-not-ready`，response read 与 write timeout 有稳定 code，错误和进程输出不包含完整路径。
- 状态/路由证据：6 个 reducer 测试覆盖 happy path、latest-wins replacement、Close、Failed cleanup 和 stale result；6 个 router 测试覆盖绝对且存在的路径、相对/缺失路径、Close 幂等、duplicate replay、FIFO eviction 和即时 cleanup 后只保留最新请求。
- x64-only：VS `dumpbin /headers src/rust/target/debug/previewit-broker.exe` 输出 `8664 machine (x64)`；`rustup target list --installed` 只输出 `x86_64-pc-windows-msvc`；`rg -n "aarch64|ARM64" rust-toolchain.toml src/rust .github/workflows/foundation.yml tools/test-foundation.ps1` 无匹配。WorkerProbe 继续固定 `<Platforms>x64</Platforms>`、`<PlatformTarget>x64</PlatformTarget>`、`<Prefer32Bit>false</Prefer32Bit>`。

## 执行记录

- 设计阶段：基于 QuickLook `4.5.0` 的 Mutex/pipe 实现、foundation vertical slice 的 pipe/supervisor 证据、PowerToys `appMutex.h`/Runner 和 Tauri Windows single-instance 插件完成方案比较。
- Task 1（`2a38db2 feat: define broker control protocol`）：先让 Rust/.NET parity 测试因缺少 `BrokerControlRequest`、`OpenPath`、`BrokerControlResponse` 符号失败，再加入独立于 Worker `Envelope` 的最小 schema；GREEN 为 Rust protocol 5/5、.NET protocol 6/6。
- Task 2（`34b13c5 feat: add broker request state machine`）：先因缺少 session module/API 进入 RED，再实现纯 `SessionReducer`；6/6 覆盖完整 phase、replacement、Close、cleanup 和 stale event，状态只由 `handle(event)` 改变。
- Task 3（`ad2fd10 feat: elect one broker per user session`）：先因缺少 `InstanceLease`/`InstanceRole` 进入 RED，再实现 session-local Mutex、contender takeover 与 current-user + SYSTEM security；lease 4/4、既有 Worker handshake 5/5。通用 HANDLE 不声明 `Send`，避免破坏 Win32 Mutex 的线程所有权。
- Task 4（`d8a6dde feat: add broker command channel`）：正向 RED 是 unresolved `BrokerCommandClient`/`BrokerCommandServer`；随后逐轮用缺失 error variant、通用 `transport-error`、accepted oversized ID、queue Broken Pipe 和 blocking `FlushFileBuffers` 证明负向/queue/deadline 行为。最终以局部 `PipeHandle: Send` 在线程间转移 overlapped Named Pipe，保留通用 `OwnedHandle` 的非 `Send`；command unit 6/6、transport 10/10、Worker handshake 5/5。
- Task 5（`5f48783 feat: run single-instance broker control loop`）：router RED 是缺少 `CommandRouter`；process RED 证明 probe stub 让十个实例全部退出且错误地对 invalid/primary-not-ready 返回成功。最终 router 6/6、真实进程 4/4；CLI 在选主前验证，primary 路由自身命令并常驻，secondary 只转发一次，connect deadline 后只重查 Mutex。
- Task 6（本提交 `test: gate broker instance state machine`）：在 Worker build 后、general Rust tests 前加入显式 `broker-build` 与串行 `broker-single-instance` gate；workflow 已调用同一脚本，因此无需修改 `.github/workflows/foundation.yml`。
- 小设计偏差：命令响应不调用不可取消的 `FlushFileBuffers`，因为真实 RED 证明不读响应的 client 会让 server 越过 I/O deadline 并最终以 Broken Pipe 失败；改为以 overlapped `WriteFile` 完成为交付边界，正向 round trip 与 non-reading client deadline 同时通过。listener 使用容量 8 的 `sync_channel`，合法请求交入 bounded channel 后立即创建下一 pipe instance；queue 满时在当前连接直接拒绝，因此开放连接数仍受容量加 listener 限制。
- 范围保持：没有接入 Shell、热键、x86 Dialog Adapter、Viewer、Renderer、installer 或 updater；没有 ARM64 target、条件编译、产物或声明。Issue 继续保持 `open`，behavior-baseline Explore 继续保持 `open`，architecture Epic 继续保持 `draft`，等待用户明确授权关闭与毕业回写。

## 关闭回写

- epic spec：回写已验证的会话选主、命令确认、状态不变量和 stale 事件边界；Epic 保持 `draft`，不因本 Issue 完成自动关闭。
- project spec：只有在真实热键、Shell、Viewer 路径验证后，才把稳定用户路径毕业到 Project Spec。
- notes/tools：若多进程竞态或 Windows pipe ACL 形成可复用操作经验，再另行沉淀；不在设计阶段复制成 notes。

## 关闭结论

- 关闭判断：待实现和验证完成后填写。
- 验证摘要：待实现和验证完成后填写。
- 回写位置：待实现和验证完成后填写。
- 遗留事项：Shell Resolver、x86 Dialog Adapter、Viewer/Legacy Host、Renderer 和完整 CLI 命令目录不属于本 Issue。
