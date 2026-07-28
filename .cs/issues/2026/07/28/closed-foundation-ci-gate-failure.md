---
kind: issue
title: "Foundation CI gate 在 GitHub-hosted Windows 失败"
type: bug
status: closed
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
- 最近一次结果：commit `b143a2d` 的 run `30349804395` 在 GitHub-hosted `windows-2022` runner 完整成功；日志显示 `WIX: D:\a\_temp\wix314\`、MSBuild 17.14.51，并输出 `FOUNDATION_GATE_OK`。
- red-capable / 确定性 / 速度 / agent 可运行性：历史远端 job 是 red-capable；当前本机无 VS 2026 runner，不能直接重现 header 缺失。变更后须由新的远端 workflow run 验证。

## 复现与最小化

- 最小复现：GitHub-hosted Windows 2022 runner 执行 installer；workflow 将 `WIX` 写为无尾随分隔符的 `$wixRoot`，固定 QuickLook Installer PreBuildEvent 直接拼接 `"$(WIX)bin\heat"`。
- 必要因素：固定 runner 后 native build 成功；WiX 3.14.1 archive 安装、`WIX` 无尾随 `\`、QuickLook Installer 的固定自定义 `heat` 调用。
- 已排除因素：本机 `tests/baseline/legacy-build.tests.ps1` 退出 0 并输出 `LEGACY_BUILD_OK`；2026-07-28 在当前 `master@bd6a75b` 运行完整 `tools/test-foundation.ps1` 于 88 秒退出 0 并输出 `FOUNDATION_GATE_OK`。未证实为 legacy build 根因。

## 根因定位

- 假设：两项均已证实。`windows-latest` 迁移至 VS 2026 后，`tools/build-legacy.ps1` 的 `vswhere -latest` 选择 MSBuild 18，native compile 找不到 ATL header。固定到 Windows 2022 后，installer 继续暴露 WiX provisioning 的第二项错误：无尾随分隔符的 `WIX` 被 QuickLook 自定义 PreBuildEvent 与 `bin\heat` 直接连接；`WixToolPath` 并非该命令输入。
- 证据：初始 job log 指明 `windows-2025-vs2026`、MSBuild 18.7.8 和两个 C1083；Windows 2022 run 指明 MSBuild 17.14.51、Native32/Native64 成功，随后报 `D:\a\_temp\wix314bin\heat` 与 `MSB3073`。run `30348585289` 的环境已显示正确的 `WixToolPath=D:\a\_temp\wix314\bin\`，但 `QuickLook.Installer.wixproj` 的实际命令仍为 `"$(WIX)bin\heat"`，证明责任在 `WIX` 值。
- 根因链：`windows-latest` label 漂移 -> MSBuild 18 -> ATL header 缺失 -> 固定 Windows 2022 后 native build 成功 -> `WIX` 无尾随 `\` -> QuickLook PreBuildEvent 找不到 `heat` -> legacy build exit 1 -> Foundation gate 失败。
- 影响面：Foundation CI runner 与 WiX provisioning；不得改动固定 QuickLook 行为基线、native/installer source、include path 或警告策略来掩盖环境配置错误。

## 修复方案

用户于 2026-07-28 确认 CI 修复与提交/推送。Foundation workflow 固定为 `windows-2022`，维持 VS 2022/MSBuild 17 工具链；并以 `$wixRoot\` 写入 `WIX`，满足固定 QuickLook Installer PreBuildEvent 的路径连接契约。保留 `tools/build-legacy.ps1` 的平台自动发现、QuickLook 基线、WiX/Rust provisioning 语义和既有 gate 顺序。远端新 run 必须重新通过完整 gate。

## 现状如何工作

Foundation workflow 在 GitHub-hosted Windows runner 上安装 WiX 与固定 Rust toolchain，再调用 `tools/test-foundation.ps1`。其中 legacy build wrapper 用 `vswhere -latest` 取得满足 MSBuild component 的 Visual Studio，并编译固定 QuickLook solution；installer 导入 workflow 提供的 WiX targets，随后其固定 PreBuildEvent 用 `$(WIX)bin\heat` 生成 `heat` 路径。`windows-latest` 的镜像迁移使 VS 2026 成为最新实例，native project 因 ATL header 不可用失败。固定 Windows 2022 后该分支通过，随后无尾随分隔符的 `WIX` 生成不存在的路径并返回非零码。

## 影响范围

- 必须修改：`.github/workflows/foundation.yml` 的 GitHub-hosted runner label 与 `WIX` 赋值；`tests/baseline/foundation-workflow.tests.ps1` 覆盖 QuickLook 实际读取的 WiX root path contract；`tools/test-foundation.ps1` 保持 workflow contract 为首个 checked step。
- 需要验证：workflow YAML 的 runner selection、legacy build、后续 Rust/.NET gate steps，及新的 GitHub-hosted run。
- 仍待调查：VS 2026 image 的 ATL component 与缺失 header 的上游不一致；不属于本次修复范围。

## 实现设计

### 这次要怎么做

把 CI 运行环境从浮动的 `windows-latest` 收窄到 Windows Server 2022。QuickLook 4.5 基线已有 VS 2022/MSBuild 17 的本地成功证据，Windows 2022 hosted image 也提供同代工具链；因此解决环境漂移，而非改动 legacy source 或把缺失 header 路径硬编码进项目。

### 功能分工与边界

workflow 继续只负责提供可复现宿主环境与调用 Foundation gate；gate 先验证 runner 与 WiX path contract，再执行既有步骤；build wrapper 继续负责在该环境中发现 MSBuild、设置 Git/WiX shim 并传播结果。不会新增 setup action、Visual Studio 安装步骤、header copy、native include path、QuickLook source 修改或 warning suppression。

### 设计侧重点

- 可靠性：固定 runner label 消除 `windows-latest` 的工具链漂移；`WIX` 显式保留 QuickLook PreBuildEvent 所需分隔符；同一次 CI run 仍覆盖全 gate。
- 可维护性：一个 YAML label 是唯一环境选择点，保留 `build-legacy.ps1` 的单一构建归属。
- 可测试性：先用 focused workflow-contract test 证明标签，再跑既有完整本地 gate；远端 run 作为真实环境回归。

### 一步步怎么改

1. 扩展 focused PowerShell contract test，期望 QuickLook 使用的 WiX root path 以 `\` 结束。
2. 运行测试并观察无尾随分隔符的预期 RED。
3. 只将 workflow 的 `WIX` 写入值改为 `$wixRoot\`。
4. 重跑 focused test、完整 Foundation gate、YAML syntax inspection；提交、推送并观察远端 run。

### 怎么确认做对

- focused contract test 在 WiX root path 修复前失败、修复后通过。
- `pwsh -NoProfile -File tools/test-foundation.ps1` 继续输出 `FOUNDATION_GATE_OK`。
- 推送后的 GitHub Actions run 在 `windows-2022` 完整通过，含 Installer 的 `heat` 命令；用户已授权该远端动作。

## 验证

- `pwsh -NoProfile -File tests/baseline/legacy-build.tests.ps1`：本地 PASS，输出 `LEGACY_BUILD_OK`。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：2026-07-28 在当前 `master@bd6a75b` PASS，88 秒，输出 `FOUNDATION_GATE_OK`。
- `pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1`：先确认 RED，报 `Foundation job must use runs-on: windows-2022`；修改 workflow 后 PASS，输出 `FOUNDATION_WORKFLOW_OK`。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：runner pin 与 gate contract 接入后本地 PASS，110.8 秒，首步输出 `FOUNDATION_STEP=workflow-runner` 和 `FOUNDATION_WORKFLOW_OK`，最终输出 `FOUNDATION_GATE_OK`。
- `pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1`：WiX path assertion 先 RED，报 `Foundation WiX tool path must end with a backslash`；workflow value修复后 PASS。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：WiX path 修复后可复现 PASS，238.6 秒，输出 `FOUNDATION_GATE_OK`。
- `pwsh -NoProfile -File tests/baseline/foundation-workflow.tests.ps1`：针对 `WIX` root 断言先 RED，报 `Foundation WiX root must end with a backslash`；写入 `$wixRoot\` 后 PASS。
- `pwsh -NoProfile -File tools/test-foundation.ps1`：`WIX` root 修复后 PASS，237.7 秒，输出 `FOUNDATION_GATE_OK`。
- GitHub Actions run `30349804395`：`b143a2d` 在 `windows-2022` 完整 PASS，job `90244365530` 于 2026-07-28 10:17:29 UTC 完成；环境记录 `WIX: D:\a\_temp\wix314\`，最终输出 `FOUNDATION_GATE_OK`。

## 执行记录

- 2026-07-28：认证后读取完整 job log，定位 VS 2026/MSBuild 18 的 `atlcomcli.h` C1083；用户确认固定到 `windows-2022`。
- 2026-07-28：按 TDD 新增 `tests/baseline/foundation-workflow.tests.ps1`，先观察针对旧 `windows-latest` 的 RED，再将 workflow runner 改为 `windows-2022`，focused test 变绿。
- 2026-07-28：发现 standalone test 不能保护未来 CI 变更；扩展该 test 要求 gate 调用它，观察 `Foundation gate must run workflow runner contract` 的第二个 RED，再在 `tools/test-foundation.ps1` 添加首个 `workflow-runner` checked step。focused test 与完整本地 gate 通过。未改 QuickLook 基线、build wrapper 或远端状态。
- 2026-07-28：提交并推送 `1123a32` 后，run `30347238521` 以 MSBuild 17.14.51 成功完成 Native32/Native64，Installer 随后暴露 WiX tool-path bug。
- 2026-07-28：按 TDD 扩展 workflow contract，先观察 WiX path 的 RED，再以尾随分隔符修复 `WixToolPath`；focused test 与完整本地 gate 通过。待提交并验证第二次远端 run。
- 2026-07-28：commit `014d0fd` 的 run `30348585289` 仍失败；日志同时显示带尾随分隔符的 `WixToolPath` 和错误的 `D:\a\_temp\wix314bin\heat`。读取固定 QuickLook Installer PreBuildEvent 后，确认命令直接读取 `WIX`，而非 `WixToolPath`。
- 2026-07-28：按 TDD 新增 `WIX` root contract，先观察预期 RED，再将 workflow 写入值改为 `$wixRoot\`。focused contract 与完整本地 Foundation gate 均 PASS；待提交并验证下一次 GitHub-hosted run。
- 2026-07-28：提交并推送 `b143a2d` 后，run `30349804395` 完整通过；GitHub-hosted 环境确认 `WIX` 含尾随分隔符、MSBuild 17.14.51 与 `FOUNDATION_GATE_OK`。issue 等待用户明确关闭。

## 顺手发现

- 上游 QuickLook workflow 将 NuGet restore 与 MSBuild 分开；PreviewIt legacy wrapper 使用 `msbuild /restore`。这只是环境差异候选，尚不能作为修复依据。

## 关闭回写

- project spec：`.cs/spec/index.md` 的“Foundation build gate”记录固定 `windows-2022`、VS 2022/MSBuild 17 与 `WIX` root 分隔符契约；它们是固定 QuickLook 4.5 基线的长期构建边界。
- epic spec：无。
- notes：无；具体诊断链与远端证据留在本 issue，未形成独立复用流程。
- AGENTS.md / CLAUDE.md：无。
- tools：无；未确证 restore/build 必须拆分。

## 关闭结论

- 根因摘要：`windows-latest` 漂移至 VS 2026，MSBuild 18 不能编译 QuickLook ATL；Windows 2022 解除该阻断后，workflow 的无分隔符 `WIX` 被固定 QuickLook PreBuildEvent 与 `bin\heat` 拼接。
- 修复摘要：Foundation runner 固定为 `windows-2022`；workflow contract 覆盖 runner、QuickLook 实际读取的 `WIX` root 与 gate 接入；`WIX` 现在以 `\` 结束。
- 验证摘要：三个 contract 均经历 RED/GREEN；完整本地 gate 与 GitHub-hosted runs `30349804395`、`30350253823` 均输出 `FOUNDATION_GATE_OK`。
- 关闭判断：用户于 2026-07-28 明确确认关闭；目标、范围与验证全部满足。
- 遗留事项：无。
