---
kind: issue
title: "Foundation CI gate 在 GitHub-hosted Windows 失败"
type: bug
status: open
created: 2026-07-28
epic: ""
---

# Foundation CI gate 在 GitHub-hosted Windows 失败

## 目标

`master` 的 GitHub Actions Foundation job 应在 GitHub-hosted Windows runner 上完成，并输出 `FOUNDATION_GATE_OK`。

## 归属

- 独立 issue。
- 相关 spec：`.cs/issues/2026/07/22/closed-preview-foundation-vertical-slice.md` 记录 Foundation gate 的既有成功契约。

## 当前证据

- 预期行为：`.github/workflows/foundation.yml` 调用 `tools/test-foundation.ps1` 后退出 0。
- 实际行为：`master@bd6a75b` 的 Actions run `30334754040`、job `90197157722` 在 `Run foundation gate` 以 exit code 1 结束。
- 最小场景：GitHub-hosted `windows-latest` runner 执行 workflow；2026-07-28 实际解析为 `windows-2025-vs2026`。
- 原始证据：完整 job log 在 `FOUNDATION_STEP=legacy-build` 后记录 `MSBuild version 18.7.8+1ac568fee for .NET Framework`，随后两个 native project 均报 `error C1083: Cannot open include file: 'atlcomcli.h': No such file or directory`。

## 反馈回路

- 命令或操作入口：GitHub Actions `Foundation` workflow，run `30334754040`。
- 断言的具体症状：`Run foundation gate` 不输出 `FOUNDATION_GATE_OK`，job exit code 为 1。
- 最近一次结果：失败；认证后 `gh run view 30334754040 --repo evangwt/PreviewIt --log-failed` 可稳定读出两个 `C1083` 错误。
- red-capable / 确定性 / 速度 / agent 可运行性：历史远端 job 是 red-capable；当前本机无 VS 2026 runner，不能直接重现 header 缺失。变更后须由新的远端 workflow run 验证。

## 复现与最小化

- 最小复现：待取得完整 job log 后确定实际失败的 `FOUNDATION_STEP`。
- 必要因素：`windows-latest` 选择 VS 2026/MSBuild 18；QuickLook native projects 引用 ATL 的 `atlcomcli.h`。workflow 仍安装 WiX 3.14.1、Rust 1.97.0 x64，并使用 NuGet/Cargo cache。
- 已排除因素：本机 `tests/baseline/legacy-build.tests.ps1` 退出 0 并输出 `LEGACY_BUILD_OK`；2026-07-28 在当前 `master@bd6a75b` 运行完整 `tools/test-foundation.ps1` 于 88 秒退出 0 并输出 `FOUNDATION_GATE_OK`。未证实为 legacy build 根因。

## 根因定位

- 假设：已证实。`windows-latest` 迁移至 VS 2026 后，`tools/build-legacy.ps1` 的 `vswhere -latest` 选择 MSBuild 18；该环境不能为 QuickLook Native32/Native64 提供可编译的 `atlcomcli.h`。
- 证据：job log 指明 `windows-2025-vs2026`、MSBuild 18.7.8 和两个 C1083；公开 VS 2026 image manifest 虽声明 ATL component，却与实际编译缺头文件矛盾。Windows Server 2022 image 仍提供 VS 2022/MSBuild 17.14；其官方 toolset JSON 明确包含 `Microsoft.VisualStudio.Component.VC.ATL` 与 `Microsoft.VisualStudio.Component.VC.ATLMFC`，且与本机成功环境同代。
- 根因链：`windows-latest` label 漂移到 VS 2026 -> `vswhere -latest` 选 MSBuild 18 -> QuickLook native compile 找不到 ATL header -> legacy build exit 1 -> Foundation gate 失败。
- 影响面：仅 Foundation CI runner 选择；不得改动固定 QuickLook 行为基线、native source 或警告策略来掩盖 runner 兼容性失败。

## 修复方案

用户于 2026-07-28 确认：Foundation workflow 固定为 `windows-2022`，维持 VS 2022/MSBuild 17 工具链。只改 `.github/workflows/foundation.yml` 的 runner label；保留 `tools/build-legacy.ps1` 的平台自动发现、QuickLook 基线、WiX/Rust provisioning 和 gate 顺序。远端新 run 必须重新通过完整 gate。

## 现状如何工作

Foundation workflow 在 GitHub-hosted Windows runner 上安装 WiX 与固定 Rust toolchain，再调用 `tools/test-foundation.ps1`。其中 legacy build wrapper 用 `vswhere -latest` 取得满足 MSBuild component 的 Visual Studio，并编译固定的 QuickLook solution。`windows-latest` 的镜像迁移改变了这个选择：VS 2026 成为最新实例，native project 因 ATL header 不可用失败，wrapper 将 MSBuild 非零码原样返回，gate 随即停止。

## 影响范围

- 必须修改：`.github/workflows/foundation.yml` 的 GitHub-hosted runner label；`tools/test-foundation.ps1` 将 workflow contract 作为首个 checked step。
- 需要验证：workflow YAML 的 runner selection、legacy build、后续 Rust/.NET gate steps，及新的 GitHub-hosted run。
- 仍待调查：VS 2026 image 的 ATL component 与缺失 header 的上游不一致；不属于本次修复范围。

## 实现设计

### 这次要怎么做

把 CI 运行环境从浮动的 `windows-latest` 收窄到 Windows Server 2022。QuickLook 4.5 基线已有 VS 2022/MSBuild 17 的本地成功证据，Windows 2022 hosted image 也提供同代工具链；因此解决环境漂移，而非改动 legacy source 或把缺失 header 路径硬编码进项目。

### 功能分工与边界

workflow 继续只负责提供可复现宿主环境与调用 Foundation gate；gate 先验证自身 runner contract，再执行既有步骤；build wrapper 继续负责在该环境中发现 MSBuild、设置 Git/WiX shim 并传播结果。不会新增 setup action、Visual Studio 安装步骤、header copy、native include path、QuickLook source 修改或 warning suppression。

### 设计侧重点

- 可靠性：固定 runner label 消除 `windows-latest` 的工具链漂移；同一次 CI run 仍覆盖全 gate。
- 可维护性：一个 YAML label 是唯一环境选择点，保留 `build-legacy.ps1` 的单一构建归属。
- 可测试性：先用 focused workflow-contract test 证明标签，再跑既有完整本地 gate；远端 run 作为真实环境回归。

### 一步步怎么改

1. 先添加 focused PowerShell contract test，期望 Foundation workflow 使用 `windows-2022`。
2. 运行测试并观察当前 `windows-latest` 的预期 RED。
3. 将 workflow runner label 改为 `windows-2022`，并把 contract test 作为 gate 首个 checked step。
4. 重跑 focused test、完整 Foundation gate、YAML syntax inspection；提交前不触发远端 run。

### 怎么确认做对

- focused contract test 在修复前失败、修复后通过。
- `pwsh -NoProfile -File tools/test-foundation.ps1` 继续输出 `FOUNDATION_GATE_OK`。
- 推送后的 GitHub Actions run 在 `windows-2022` 完整通过；该远端动作需单独授权。

## 验证

- `pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1`：本地 PASS，输出 `LEGACY_BUILD_OK`。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：2026-07-28 在当前 `master@bd6a75b` PASS，88 秒，输出 `FOUNDATION_GATE_OK`。
- `pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1`：先确认 RED，报 `Foundation job must use runs-on: windows-2022`；修改 workflow 后 PASS，输出 `FOUNDATION_WORKFLOW_OK`。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：runner pin 与 gate contract 接入后本地 PASS，110.8 秒，首步输出 `FOUNDATION_STEP=workflow-runner` 和 `FOUNDATION_WORKFLOW_OK`，最终输出 `FOUNDATION_GATE_OK`。

## 执行记录

- 2026-07-28：认证后读取完整 job log，定位 VS 2026/MSBuild 18 的 `atlcomcli.h` C1083；用户确认固定到 `windows-2022`。
- 2026-07-28：按 TDD 新增 `tests/baseline/foundation-workflow.tests.ps1`，先观察针对旧 `windows-latest` 的 RED，再将 workflow runner 改为 `windows-2022`，focused test 变绿。
- 2026-07-28：发现 standalone test 不能保护未来 CI 变更；扩展该 test 要求 gate 调用它，观察 `Foundation gate must run workflow runner contract` 的第二个 RED，再在 `tools/test-foundation.ps1` 添加首个 `workflow-runner` checked step。focused test 与完整本地 gate 通过。未改 QuickLook 基线、build wrapper 或远端状态。

## 顺手发现

- 上游 QuickLook workflow 将 NuGet restore 与 MSBuild 分开；PreviewIt legacy wrapper 使用 `msbuild /restore`。这只是环境差异候选，尚不能作为修复依据。

## 关闭回写

- project spec / epic spec：无，除非定位出长期 CI/构建约束。
- notes：若 GitHub-hosted runner 有可复用诊断限制或固定工具链约束，记录至 `.cs/notes/`。
- AGENTS.md / CLAUDE.md：无。
- tools：若确证 restore/build 需独立步骤，更新对应 build tool 与回归测试。

## 关闭结论

- 根因摘要：`windows-latest` 漂移至 VS 2026，`vswhere -latest` 选择 MSBuild 18，QuickLook Native32/Native64 均缺失 `atlcomcli.h`。
- 修复摘要：Foundation runner 固定为 `windows-2022`；新增 workflow contract test 防止恢复浮动标签。
- 验证摘要：新 test 已经历 RED/GREEN；完整本地 gate 于修复后 PASS，输出 `FOUNDATION_GATE_OK`。
- 遗留事项：需经用户授权提交/推送，并确认新的 GitHub-hosted Windows 2022 run 完整通过；之后才可关闭 issue。
