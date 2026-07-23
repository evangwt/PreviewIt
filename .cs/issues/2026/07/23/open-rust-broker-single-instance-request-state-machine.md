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

当前 Broker 在控制面初始化前通过 session-scoped、current-user/SYSTEM Mutex 完成选主。主实例创建确定命名的 local-only Tokio Named Pipe endpoint；accept 后先补充 listener，再由 connection task 完成统一 4-byte LE framing、1 MiB 上限、Protobuf decode 和 `BrokerControlContract` 校验。合法请求只以 `PendingCommand { ValidatedCommand, reply }` 进入容量为 8 的队列；pipe handle 始终归 endpoint task，状态线程只持 reply handle。

command endpoint 的容量现在显式包含 8 个 queued、1 个 active、4 个 decoding connection、1 个 admission/rejection reserve 和 1 个 standby listener，共 15 个 Win32 pipe instance。客户端接走 standby listener 后，replacement listener 即使在瞬时饱和时命中 `ERROR_PIPE_BUSY`，也只通过统一 `primary-busy` 路径拒绝该连接并重建 listener，不再终止 endpoint；组合满载测试同时占满 active、queue 与 decoder，并在拒绝后证明 endpoint 可以恢复 accepted round trip。

`BrokerRuntime` 消费 `InstanceLease`、endpoint、`CommandRouter` 和唯一 `EventSink`，是 session state 与 effect 的唯一所有者。Router 不接触 Protobuf、文件系统或 effect feedback，每次调用只执行一个 reducer step；Runtime 通过一个穷举 effect pump 消费所有 `SessionEffect`，当前只为没有真实执行资源的 `Cancel`/`Cleanup` 反馈一次 `CleanupComplete`。每个 reducer step 都按 `(phase, request_id)` 独立比较并发出 transition，因此 replacement 明确记录 `Resolving(request-1) -> Closing(request-1) -> Resolving(request-2)`。显式 shutdown 先停止并 join endpoint，再释放 lease；Drop 复用同一幂等顺序。

次实例发送一次 command，并用同一个 contract 校验 response version、command ID 和 ack shape 后才决定退出码；如果 endpoint 从未出现且旧 owner 已退出，contender 可以取得 lease 并成为新 primary。`accepted=true` 只表示命令已经进入 Broker session，不表示路径存在或预览成功。UTF-16LE、NUL、长度和 absolute path 属于同步控制边界；存在性、访问权限和文件身份属于未来 Shell Resolver 驱动的 `Resolving` 异步事件。

## 影响范围

- 必须修改：
  - `pipe.rs` 和 command endpoint：用同一套成熟的 Tokio Windows Named Pipe/framing 实现替换手写 Overlapped read/write/cancel，保留现有 DACL、local-only、PID 验证和 deadline。
  - Broker control contract：集中 wire request/response 转换、protocol version、`command_id`、ack shape 和稳定错误码；队列内部只传 `ValidatedCommand`，不传原始 Protobuf。
  - endpoint/runtime 生命周期：连接始终由 endpoint task 拥有，通过 reply handle 与唯一状态线程交互；shutdown 必须停止 accept、取消/等待连接任务、join endpoint，最后才释放 lease。
  - router/runtime：删除同步文件系统访问和重复路径解码；增加统一 `BrokerEvent`，明确 accepted/rejected、busy/full、duplicate、phase、stale、failure 和 shutdown。
  - acceptance remediation：修正组合容量与 listener replacement；把 immediate cleanup feedback 收回 Runtime；删除 release API 中的 raw inspection handle/SID helper；删除测试中的无界 `FlushFileBuffers`。
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
- **Command endpoint**：每个 task 从 accept 到 response 始终拥有自己的连接，accept 后先补下一个 listener，再有界 decode。容量显式包含 8 个 queued、1 个 active、4 个 decoder、1 个 admission/rejection instance 和 1 个 standby listener，共 15 个 Win32 instance；replacement 的 `ERROR_PIPE_BUSY` 是可恢复的饱和，不是 endpoint 终止条件。合法请求以 `PendingCommand { command, reply }` 进入状态队列；endpoint 不接触 reducer，状态线程不接触 pipe handle。endpoint 拥有 shutdown signal、连接任务和 join handle，Drop 返回前必须完成停止与 join。
- **Broker runtime**：消费 `InstanceLease`、endpoint、router 和 event sink，是唯一状态与 effect 所有者。它路由 `ValidatedCommand`、通过 reply 返回 `CommandAck`，穷举消费 reducer effect，并在没有真实执行资源时只为 `Cancel`/`Cleanup` 立即反馈一次 `CleanupComplete`。每次 reducer step 都独立观察 `(phase, request_id)` transition；退出时先停 endpoint、最后释放 lease。
- **Command router / Session reducer**：router 只做一次纯领域映射、request ID 生成和 duplicate cache；它返回 reducer effect，不执行或反馈 effect，不解码 Protobuf，也不调用文件系统。reducer 的 state/effect 不变量保持不变。
- **统一观测**：一个 typed `BrokerEvent` 和一个真实 `EventSink` 接缝同时服务生产日志与测试 recorder。事件覆盖 instance、endpoint、command、session transition、duplicate、queue、stale 和 failure；不存在真实状态变化的 `lease-lost` 从契约删除，以 `lease-released`/`takeover` 表达实际行为。
- **安全测试接缝**：真实 server handle 的 ACL、pipe flag 和 inheritance 检查留在 `command.rs` 的 test-only module，由测试直接持有 handle；release `BrokerCommandServer` 不保存或返回非拥有型 raw handle，crate root 不导出 inspection SID helper。

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

容量语义分开命名并从一个内部配置推导：queued commands、active routed command、concurrent decoding connections、admission/rejection reserve 和 standby listener reserve。endpoint 从未出现返回 `primary-not-ready`；endpoint 存在但 decoder/admission 槽耗尽返回 `primary-busy`；命令已解码但状态队列已满返回 `queue-full`。组合满载不得停止 endpoint，释放客户端后必须完成新的 accepted round trip。

状态转换的核心不变量：

- `Idle` 只接受新请求或无操作的 `Close`；新请求创建 active `request_id` 并进入 `Resolving`。
- 在任一活动阶段收到新请求时，进入 `Closing { old, next }` 并只为 old 产生一次 cancel effect；`Closing` 期间的新请求替换 `next`，被替换且尚未启动的 pending request 只产生 `superseded` 结果。
- old cleanup 完成后，有 `next` 就进入它的 `Resolving`，没有则进入 `Idle`。本 Issue 没有接入真实执行资源，因此 Runtime 对 `Cancel`/`Cleanup` 做一次同步完成反馈；未来接入 Worker/Resolver 时必须由同一个 Runtime effect 入口等待真实 cleanup，不得在 Router 内伪造完成。
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
6. 用 active + full queue + full decoder + next admission 的真实组合测试修正 pipe instance 上限，并证明饱和后恢复。
7. 把 security inspection 移到 test-only module，删除 release raw handle/SID API；用有 deadline 的客户端替换 `FlushFileBuffers` 协调。
8. 把 cleanup feedback 从 Router 移到 Runtime effect pump，逐 reducer step 记录 transition，并证明 replacement 的 `Resolving -> Closing -> Resolving` 事件序列。

### 怎么确认做对

- Contract：wrong/mismatched version、非法或超长 ID、mismatched response ID、accepted/rejected shape、结构化路径和错误优先级都有表驱动测试；非法 ID 不出现在响应。
- Transport：延迟部分 frame 后断开稳定为 `truncated-frame`；一个慢客户端存在时正常 secondary 仍成功；active、queue 和 decoder 同时占满后，下一客户端稳定得到 `primary-busy`，endpoint 不退出，释放容量后恢复；decoded queue 满返回 `queue-full`。
- Lifecycle：Drop server 后立即用同名 product ID 重建成功；graceful shutdown 先产生 endpoint-stopped/lease-released，再允许新 owner；crash takeover 仍由 abandoned/signaled Mutex 完成。
- Runtime：router 测试不创建文件 fixture、不访问文件系统，也不反馈 effect；absolute missing path 获得 accepted/request ID 并停在 `Resolving`，Runtime 穷举消费 effect；replacement 产生 `Resolving(old) -> Closing(old) -> Resolving(next)`，duplicate/stale 通过同一 event recorder 观察。
- Security：test-only module 读取真实 pipe security descriptor，ACE 只包含 SYSTEM 和当前用户；读取 server pipe flags 确认 remote rejection，读取真实 handle flag 确认不可继承；release API 不暴露 inspection handle/SID；当前用户真实 client 可以完成 round trip。
- 回归：Worker handshake、handle transfer、supervision、protocol parity、十进程选主和 500 次 command round trip 继续通过；`tools/test-foundation.ps1` 退出 0，PE 为 x64，installed targets 与仓库配置不含 ARM64。

## 验证与历史证据

### 当前收敛验证

2026-07-23 的 acceptance review 判定 `43efb8c..1738f0e` 证据不足以关闭 Issue：独立 queue/decoder 通过没有覆盖组合实例上限；security 测试依赖 release raw handle；Runtime 没有成为 effect 的唯一所有者；queue 测试含无 deadline 的 `FlushFileBuffers`。`1404001..ed37333` 已逐项修复这些根因，并按 remediation plan 重新建立以下证据；旧结果只保留为历史回归基线。

- Contract 与 response validation：`broker_control_contract` 为 13/13。覆盖 command ID/version 校验顺序、奇数 UTF-16LE、embedded NUL、超长/相对路径、missing command、accepted/rejected shape、response version/ID mismatch 和 id-less `primary-busy`；非法 ID 不回显。absolute missing path 结构校验成功，不依赖文件 fixture。
- Transport 与并发：`delayed_partial_frame_is_stably_truncated` 的既有 100/100、`broker_control_round_trips_open_path` 的既有 500/500 和 `one_slow_client_does_not_block_a_normal_command` 的既有 20/20 继续由完整 workspace gate 覆盖。`pipe.rs` 仍无手写 Overlapped I/O 或第二套 framing。
- 组合容量与恢复：`combined_capacity_rejects_without_stopping_endpoint` 在修复前稳定 RED 为 `Err(Transport(TruncatedControlFrame))`，修复后与 `broker_control` 全套 15/15 通过，并单独重复 20/20。它同时占满 active、8 个 queued 和 4 个 decoder，下一连接稳定得到 `primary-busy`，随后释放容量并完成 accepted round trip。`full_pending_queue_returns_stable_rejection` 与 `exhausted_decode_slots_report_primary_busy` 也各重复 20/20；前者返回 `queue-full`，后者返回 `primary-busy`，两者均证明恢复。测试通过有 deadline 的 `BrokerCommandClient` 和 `CommandQueueFull` event 协调，源码/测试均无 `FlushFileBuffers`。
- Runtime effect 所有权：TDD RED 明确暴露 Router 提前越过 `Closing` 和 Runtime 压扁 replacement transition；GREEN 后 `command_routing` 5/5、`broker_runtime` 4/4、`request_state_machine` 6/6。Router 只返回一次 reducer effect；Runtime 的单一、穷举 pump 反馈 cleanup，并逐步记录 `(phase, request_id)` transition。`replacement_emits_each_transition_while_runtime_drives_cleanup` 单独重复 20/20，严格观察 accepted、`Resolving(old) -> Closing(old)`、`Closing(old) -> Resolving(next)` 三个事件。
- Windows 安全与发布 API：安全检查已移入 `command.rs` 的 `#[cfg(test)]` module，由测试自己创建并拥有真实 server handle；`command_pipe_dacl_contains_only_system_and_current_user` 与 `command_pipe_rejects_remote_clients_and_allows_current_user` 为 2/2，所在 unit suite 为 9/9。测试仍用 `GetSecurityInfo`、ACL enumeration、`GetHandleInformation`、`GetNamedPipeInfo` 和当前 token SID 验证 SYSTEM/current-user DACL、不可继承、remote rejection 与真实 round trip。release `BrokerCommandServer` 不保存/返回 inspection raw handle，crate root 不导出 inspection SID helper；源码 guard 对 `inspection_handle|current_user_sid_for_inspection` 零匹配。没有声称执行远程网络连接测试。
- Lifecycle、进程与回归：`broker_single_instance` 4/4，覆盖十进程单 primary、secondary ack、crash takeover、held lease/no endpoint、参数先校验和 PE x64；Worker handshake 5/5、read-only handle transfer 2/2、instance lease 4/4、supervision/stale 3/3。runtime shutdown 仍证明 `endpoint-stopped` 先于 `lease-released`，随后同 product ID 可以取得新 lease。
- Focused gate：`cargo fmt --all -- --check`、workspace Clippy `--all-targets -- -D warnings` 和 `cargo test -p previewit-broker -- --test-threads=1` 均退出 0；Broker 共 70 个测试通过，Clippy 无警告。
- 完整 gate：`pwsh -NoProfile -File tools/test-foundation.ps1` 最终退出 0，实际输出 `QUICKLOOK_BASELINE_OK=b13df028f3cce1f84792f7043b57bf5cea3a3e4c`、`LEGACY_BUILD_OK`、`FOUNDATION_STEP=broker-single-instance` 和 `FOUNDATION_GATE_OK`。Gate 中 Broker 70 个测试、protocol 5 个测试、.NET protocol parity 6 个测试全部通过；rustfmt、workspace Clippy `-D warnings`、Release x64 Worker build、Broker build、legacy build 与 QuickLook provenance 均成功。security unit tests 已由 workspace gate 覆盖，因此 `tools/test-foundation.ps1` 无需修改。
- Gate 诊断记录：第一次经工具直接启动 gate 时外层只返回无 stdout/stderr 的 exit 1，没有可定位失败步骤，未计为通过。未修改生产代码的情况下按 gate 原顺序逐步捕获，九个 step 全部通过；随后再次执行原始脚本明确返回 0 并产生上述完整标记。保留这条 runner-output 异常，不把它包装成测试失败或静默忽略。
- x64-only 与边界：`rustup target list --installed` 只输出 `x86_64-pc-windows-msvc`；ARM search 对 `rust-toolchain.toml`、`src/rust`、foundation workflow 和 gate 脚本零匹配；VS x64 `dumpbin /headers src/rust/target/debug/previewit-broker.exe` 输出 `8664 machine (x64)`；removed-boundary search 对 `inspection_handle|current_user_sid_for_inspection|FlushFileBuffers` 零匹配；`git diff 3ef0edc..HEAD --check` 通过。

### 历史证据：`7e62a7b`

以下是收敛 review 前初始实现的历史证据，不是当前关闭证据。它曾通过完整 gate、Broker 46 个测试、protocol 5 个测试、.NET 6 个测试、command round trip 500/500、queue saturation 10/10、truncated-frame 100/100 和十进程/crash takeover 10/10；x64 PE 与 installed target 也曾通过。

Review 随后证明这些通过面没有覆盖四项根因：pending Overlapped I/O 没有保留内核仍引用的 buffer；slow client 可以占住唯一 listener；Drop 不 join endpoint；contract/path/event 责任分散且 router 同步调用 `Path::exists()`。真实 delayed partial-frame 当时错误地返回通用 `transport-error`，slow-client 场景中的正常 secondary 在 527 ms 后错误返回 `primary-not-ready`。这些失效证据推动 `43efb8c..0461a4b` 的替换实现，不能再用初始 gate 抵消。

## 执行记录

- 初始实现历史：`2a38db2..7e62a7b` 以严格 TDD 建立 protocol、reducer、instance lease、首版 command channel、单实例 CLI 与 foundation gate；随后 harsh review 的真实失效证据证明 control contract、I/O ownership、endpoint lifecycle、path semantics 和 observability 必须统一替换，历史通过面保留在上节但不再代表当前实现。
- Contract（`43efb8c refactor: unify broker control contract`）：集中 protocol constants、wire/domain 转换、`CommandId`、`ValidatedCommand`、`CommandAck`、response validation、错误优先级和结构路径校验，删除平行 response factory。
- Transport（`c5cd9a4 fix: unify safe named pipe transport`）：Worker/command 共用 Tokio framed Named Pipe，删除手写 Overlapped read/write/cancel 及 fallback；delayed broken pipe 统一为 `truncated-frame`。
- Endpoint（`80326c1 fix: bound broker command endpoint`）：connection task 始终拥有 pipe，状态线程只持 oneshot reply；独立限制 queue/active/decoder/listener，显式 shutdown/join，client 用同一 contract 校验 response。
- Router（`a1ee580 refactor: keep broker routing pure`）：Router 只接收 `ValidatedCommand`，删除 Protobuf、路径 decode、版本常量和文件系统访问；absolute missing path 使用 approved async `Resolving` 语义，duplicate disposition 显式化。
- Runtime（`e48b158 feat: centralize broker runtime events`）：`BrokerRuntime` 统一拥有 lease、endpoint、router 和 sink；`BrokerEvent::name()` 成为唯一事件词汇，main 不再维护 raw event 字符串或丢弃 effect。为避免复制 reducer，额外把 `CommandRouter::handle_event` 暴露为 crate-private 入口。
- Security/capacity（`0461a4b test: prove broker command security boundaries`）：通过真实 Win32 handle/ACL/pipe flag 检查安全边界，并证明 queue/decoder saturation 与断开恢复。只增加非拥有型初始 listener handle 和当前用户 SID 的隐藏只读 inspection 入口，没有第二个 descriptor builder。
- 组合容量修复（`1404001 fix: preserve broker endpoint at full capacity`）：把真实 instance 预算修正为 15，replacement listener 饱和改为可恢复拒绝，并以组合满载/恢复测试覆盖；删除 queue 测试中的无界 `FlushFileBuffers`。
- 安全接缝收敛（`50aadc8 test: hide broker pipe inspection from release api`）：删除 release inspection handle/SID API 与 integration seam，把真实 Win32 security characterization 移入 test-only module。
- Runtime effect 收敛（`ed37333 refactor: make broker runtime own session effects`）：Router 只做一次 reducer step，Runtime 成为唯一 effect pump，并按每个 `(phase, request_id)` step 发出 transition。
- 最终验证（本次证据提交）：focused suites、四个 hostile scenario 各 20/20、完整 foundation gate、x64 PE/target/ARM64 search、removed-boundary search 与 diff check 全部完成；security unit tests 已由 workspace gate 覆盖，因此没有修改 gate 脚本。
- Remediation acceptance review（2026-07-23）：主审复核 `3ef0edc..3cd1c5c` 的 11 个变更文件，CRITICAL/HIGH/MEDIUM/LOW 均为 0，Architectural Status 为 `CLEAR`，技术结论为 `APPROVE`。独立 code-reviewer/architect 通道因外部服务 403 未能运行，未将其伪装或计入通过证据；当前结论明确是主审技术验收，不替代用户关闭授权。
- 范围保持：没有接入 Shell、热键、x86 Dialog Adapter、Viewer、Renderer、installer 或 updater；没有 ARM64 target、条件编译、产物或声明。Issue 继续保持 `open`，behavior-baseline Explore 继续保持 `open`，architecture Epic 继续保持 `draft`，等待用户明确授权关闭与毕业回写。
- 首次 Acceptance review（2026-07-23）：发现组合满载会在 replacement listener 创建时停止 endpoint、release API 泄漏短生命周期 raw handle、effect feedback 仍在 Router 且 Runtime 丢弃多数 effect、queue 测试可在 `FlushFileBuffers` 无界挂起。用户确认采用结构收敛方案修复；原完整 gate 保留为历史回归证据，不代表 remediation 已完成。

## 关闭回写

- epic spec：回写已验证的会话选主、命令确认、状态不变量和 stale 事件边界；Epic 保持 `draft`，不因本 Issue 完成自动关闭。
- project spec：只有在真实热键、Shell、Viewer 路径验证后，才把稳定用户路径毕业到 Project Spec。
- notes/tools：若多进程竞态或 Windows pipe ACL 形成可复用操作经验，再另行沉淀；不在设计阶段复制成 notes。

## 关闭结论

- 关闭判断：acceptance remediation 已实现，完整门禁与新的 harsh review 均已通过，技术上可关闭；未经用户明确授权仍不得关闭，Issue 状态保持 `open`。
- 验证摘要：见“验证与历史证据”的当前收敛验证；`7e62a7b` 与 `1738f0e` 只保留为各自阶段的历史证据，当前验收以 `1404001..本次证据提交` 为准。
- 回写位置：获得用户关闭授权后，先把稳定的会话选主、command ack、状态不变量和 stale/event 边界回写所属 Epic；不自动关闭 Epic，也不提前写入 Project Spec。
- 遗留事项：Shell Resolver、x86 Dialog Adapter、Viewer/Legacy Host、Renderer 和完整 CLI 命令目录不属于本 Issue。
