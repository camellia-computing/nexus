# Production verification / 生产级验证

This document defines the durable verification model for the desktop repository and its contract
with the management server. Tests are layered so a failure identifies the owning boundary; no single
GUI script is treated as proof of the complete product.

本文定义桌面仓库及其与授权服务协议的长期验证模型。测试按职责分层，使失败能够定位到真正的责任
边界；任何单一 GUI 脚本都不能代替完整产品验证。

## Verification layers / 验证分层

| Layer / 层级 | Authority proved / 验证职责 | Default environment / 默认环境 |
| --- | --- | --- |
| Rust unit, integration, and concurrency tests | Domain state machines, atomic commits, process-tree handling, licensing cryptography, secure-state rules, and typed protocol behavior | Ubuntu hosted runner; native target tests where operating-system semantics matter |
| Svelte checks and unit tests | Rendering decisions, localization, shared component behavior, and typed IPC use without duplicating Rust authority | Ubuntu hosted runner |
| Browser interaction tests | Dialog focus, keyboard traversal, responsive layouts, themes, long strings, recoverable errors, visual states, and WCAG A/AA Axe scans of dashboard, Team/settings, and dialog states | Playwright Chromium on Ubuntu |
| Final server runtime tests | PostgreSQL migrations, public and Admin APIs, real Admin authentication, Team mutations, worker startup, hardening, and the static Admin UI embedded in the final image | Docker on Ubuntu 24.04 |
| Native desktop E2E | The actual Tauri process, WebView2, IPC, OS credential store, OAuth/PKCE, device proof, signed leases, process lifecycle, and representative Team flows | `windows-2025` hosted runner |
| Native package acceptance | Installer/uninstaller, shell integration, tray, startup, file dialogs, signing display, DPI, and multi-monitor behavior | Maintained Windows 11 workstation before a public release |

Rust and the server remain authoritative during GUI tests. A visible or disabled control is UI
evidence only; native tests call the same commands and real service endpoints used in production.
The normal browser suite remains parallel; `pnpm --dir ui test:e2e:stability` separately repeats the
known layout-sensitive theme matrix in one worker so resource contention cannot conceal a race.

在 GUI 测试中，Rust 与服务端仍然是最终权威。控件的可见或禁用状态只证明界面表现；原生测试必须
调用生产环境使用的同一组命令和真实服务端点。
常规浏览器套件继续并行执行；`pnpm --dir ui test:e2e:stability` 另以单 worker 重复已知资源敏感的
主题布局矩阵，避免资源竞争掩盖真实竞态。

## Hosted native workflow / 托管原生工作流

`.github/workflows/native-e2e.yml` is the reusable cross-repository workflow:

1. An Ubuntu job checks out an explicit public server repository/ref, builds the final Linux image,
   runs its container contract, and exports the exact server and pinned PostgreSQL root filesystems.
2. The bundle manifest records immutable image metadata, byte sizes, SHA-256 digests, and its required
   x86_64 WSL2 shared-loopback runtime. It contains no credentials; generated test credentials exist
   only after import.
3. A Windows job requires the current WSL package, verifies the bundle, and imports two uniquely named
   WSL2 distributions. PostgreSQL, the server distribution, and Windows communicate over the shared
   loopback network.
4. The harness generates a schema-2 fixture set with one-run keys, keyrings, independent Pro and Team
   accounts, primary/recovery and multi-device activation codes, a real invoice/payment method, four
   isolated application-data directories, and four isolated operating-system credential namespaces.
5. WebdriverIO drives the real debug-only Tauri application. The embedded driver capability cannot
   be included in a release build, and production Tauri configuration never enables it.
6. The harness unregisters both distributions and removes one-run state. Failure diagnostics are
   short-lived Actions artifacts and must be reviewed as potentially sensitive test data.

`.github/workflows/native-e2e.yml` 是可复用的跨仓工作流：Ubuntu 任务构建并验证最终 Linux 镜像，
导出带摘要及 x86_64 WSL2 共享回环网络契约的服务端与 PostgreSQL 根文件系统；Windows 任务验证后
导入两个唯一命名的 WSL2 环境，生成单次密钥、数据库、Pro/Team 激活码及隔离的客户端数据，再由
WebdriverIO 驱动真实 Tauri/WebView2。调试驱动能力在发布构建中会被编译期拒绝，结束后销毁全部
环境。失败产物生命周期很短，但仍应按可能含测试敏感数据处理。

The application is built once and each phase relaunches it against the same isolated secure state.
`smoke` covers startup, minimum responsive dimensions, guarded unlicensed mutations, visible
OAuth/PKCE activation, a real external process/log/stop/delete journey, credential persistence, and
sign-out enforcement. `full` additionally controls the owned server to verify cached offline access,
redacted recovery errors, online recovery, billing submission, terminal account suspension and
process-tree shutdown, current-device removal, and same-identity recovery-code reactivation. Three
Team devices then exercise invalid/wrong-kind token recovery, accepted-token replay, invitation and
seat governance, suspension/restoration, one-use additional-device linking, shared configuration
content and publication, sync checkpoints, alert incidents, bounded audit, inactive HTTPS Webhook
secret rotation, ownership transfer, member departure, and final sign-out.

应用只构建一次，每个阶段使用同一隔离安全状态重新启动。`smoke` 覆盖启动、最小响应式尺寸、未授权
写入拦截、可见 OAuth/PKCE 激活、真实外部进程的日志/停止/删除、凭据持久化及退出强制停止；`full`
还会控制自有服务端，验证离线缓存、脱敏恢复错误、在线恢复、账单提交、账户暂停后的进程树停止及
当前设备移除后的同身份恢复码再激活。随后三台隔离 Team 设备串联无效/错误类型令牌恢复、已接受令牌
重放、邀请与席位、成员停用/恢复、一次性附加设备、共享配置内容与发布、同步点、告警事件、受限审计、
非活动 HTTPS Webhook 密钥轮换、所有权转移、成员退出与最终登出。

Normal standard validation runs `full`. Candidate validation runs `smoke` in parallel with the four
native package builds, while managed Release validation skips Native E2E because it may contain only
the generated release delta. Scheduled and manual reusable-workflow runs default to `full`.

普通 standard 门禁执行 `full`；candidate 门禁在四平台原生包并行构建时执行 `smoke`；受管 Release
门禁因只允许生成的发布差异而跳过 Native E2E。定时及手工可复用工作流默认执行 `full`。

## Cross-repository access / 跨仓库访问

Native E2E and the contract monitor read the organization variable
`NEXUS_REPOSITORY_MAP`, bind the current checkout to logical ID `nexus-client`,
and resolve sibling logical ID `nexus-management`. The map must contain exactly
the reviewed pair with valid, distinct physical names; a missing, malformed, or
stale map fails before authentication. The owner follows the repository running
the workflow. A rename therefore updates the centrally audited map rather than
workflow code.

原生 E2E 与协议监控读取组织变量 `NEXUS_REPOSITORY_MAP`，将当前仓库绑定到逻辑 ID
`nexus-client`，并解析同级逻辑 ID `nexus-management`。映射必须精确包含经审核的双仓、名称合法且
互不相同；缺失、格式错误或过期时会在认证前失败。owner 始终跟随当前工作流仓库，仓库改名只需同步
更新集中审计的映射，无需修改工作流代码。

Cross-repository authentication has two supported modes:

- With neither credential configured, the workflow verifies that the sibling reports
  `private: false`, then checks it out with the job-scoped `github.token`. A missing or private
  mapped sibling fails before checkout with a configuration error.
- With both credentials configured in a trusted same-repository event, the workflow mints a
  short-lived installation token restricted to the mapped sibling with read-only Contents and
  Metadata access. Configure repository variable `CROSS_REPO_READ_APP_CLIENT_ID` and repository
  secret `CROSS_REPO_READ_APP_PRIVATE_KEY` together; a partial configuration fails.
- A fork or Dependabot pull request never receives or mints an App token. It can use only the public
  fallback, so a private sibling intentionally requires a trusted maintainer run.

跨仓认证支持两种模式：两个凭据均未配置时，工作流先确认同级仓库返回 `private: false`，再使用任务
级 `github.token` checkout；映射目标缺失或私有时会在 checkout 前给出配置错误。可信的同仓事件中，
两个凭据均已配置时才签发仅限映射目标且只有 Contents/Metadata 只读权限的短期安装令牌。仓库变量
`CROSS_REPO_READ_APP_CLIENT_ID` 与仓库 secret
`CROSS_REPO_READ_APP_PRIVATE_KEY` 必须同时配置，缺少任意一项都会失败。fork 或 Dependabot PR 永不
接收或签发 App 令牌，只能使用公开回退；同级仓库为私有时应由维护者在可信上下文运行。

For private operation, create or reuse a GitHub App under the current owner, grant repository
Contents read-only access (Metadata read access is implicit), install it only on the fixed client and
server logical pair, and add the Client ID variable and private-key secret to both repositories.
Validate both contract-monitor workflows and a native E2E run before changing visibility. The
Release App credentials must not be reused, and checkout credentials remain non-persistent.

私有模式下，在当前 owner 创建或复用 GitHub App，仅授予仓库 Contents 只读权限（Metadata 只读为
固有权限），并只安装到映射中的客户端与服务端逻辑仓库；随后在两个仓库分别配置 Client ID 变量和私钥
secret。切换可见性前，应先验证双端协议监控和一次原生 E2E。不得复用 Release App 凭据，checkout
凭据也不会持久化。

For a public-to-private change, provision and validate the App credentials first. For a
private-to-public change, either retain the App mode or remove both values from each repository to
exercise public fallback; never leave a partial pair. After an owner migration, create/install an App
for the destination owner and replace both repositories' values before validation. To rotate a key,
add the replacement key, update the secret in both repositories, validate trusted runs, and only then
revoke the old key. Recreating the App also requires replacing the Client ID variable.

从公开切换为私有前，先配置并验证 App 凭据；从私有切换为公开后，可以继续使用 App，也可以在每个
仓库同时删除两个值以验证公开回退，不能留下不完整的凭据对。迁移 owner 后，应在目标 owner 创建并
安装 App，更新双仓值后再验证。轮换密钥时先增加新密钥、更新双仓 secret、验证可信运行，最后撤销
旧密钥；若重建 App，还必须同步更新 Client ID 变量。

## Windows entry point / Windows 执行入口

Use PowerShell 7.6 or newer and the exact versions declared by the repository:

```powershell
./scripts/e2e-native.ps1 -Action Doctor -Provider WslBundle
./scripts/e2e-native.ps1 -Provider WslBundle -BundlePath <bundle-directory> -Suite full
```

`WslBundle` is the hosted default. Local runs may select a disposable environment without embedding
machine names or paths:

```powershell
# Exactly one Docker-enabled WSL2 distribution may be auto-selected.
./scripts/e2e-native.ps1 -Provider Wsl2Compose -ServerRepository <server-checkout> -Suite full

# The remote checkout supplies scripts/provision-e2e-compose.sh; SSH uses one loopback session.
./scripts/e2e-native.ps1 -Provider SshCompose -SshTarget <host> -SshRepository <absolute-posix-path> -Suite full

# A separately provisioned service can be used by passing every trust and activation input explicitly.
./scripts/e2e-native.ps1 -Provider Existing -ServerBaseUrl <url> `
  -EntitlementAuthorityPath <authority-json> -ProCode <code> -TeamCode <code> -Suite smoke
```

`Wsl2Compose` invokes the sibling server's disposable Compose provisioner and auto-cleans its image,
volume, database, and secrets. Its Compose state lives under the Windows run directory so Docker
Desktop can bind the generated files through its engine-visible host path. `SshCompose` requires
pre-established host trust and key-based or agent authentication. One authenticated process owns
provisioning, the loopback tunnel, diagnostics, and cleanup; omitting `SshPort` preserves the selected
SSH config entry, while an explicit value overrides
it. Owned providers expose bounded pause/resume and account-state controls used by `full`; `Existing`
cannot safely mutate external state and is therefore smoke-only. Activation codes and authority files
must not be committed, printed, or retained with ordinary build logs. Cross-identity invitation and
enrollment tokens use an ACL-restricted temporary handoff and are deleted when consumed. The `full`
suite covers signed Free quotas and capability denials, Pro continuity and enforcement, and Team
governance/cloud workflows. Every created client identity is reset after success or failure, and the
runner verifies that its exact E2E-only Credential Manager namespace is empty.

`Wsl2Compose` 调用服务端仓库的一次性 Compose 提供器并自动清理镜像、卷、数据库和密钥；其
Compose 状态位于 Windows 本次运行目录下，使 Docker Desktop 能通过引擎可见宿主机路径挂载生成文件；
`SshCompose` 要求预先建立主机信任及密钥/Agent 认证，并由同一个认证进程负责部署、回环隧道、诊断
与清理；省略 `SshPort` 时沿用 SSH config，显式设置时才覆盖端口；
自有提供器为 `full` 提供有界暂停/恢复及账户状态控制；`Existing` 无法安全修改外部状态，因此仅允许
smoke。激活码与信任文件不得提交、打印或进入普通构建日志；跨身份邀请/注册令牌只通过 ACL 限制的
临时目录交接，并在消费后删除。`full` 套件覆盖 Free 的签名额度与能力拒绝、Pro 的连续性与执行
边界，以及 Team 治理和云工作流。无论场景成功还是失败，Harness 都会重置本次创建的全部客户端
身份，并验证对应的 E2E 专用 Windows Credential Manager 命名空间为空。

## Release acceptance / 发布验收

Hosted automation is the first choice and a required gate. A public Windows release additionally
needs a maintained Windows 11 acceptance pass for real-browser focus/callback handoff, installer
repair/removal, Explorer and native file dialogs, tray and startup behavior under a normal user token,
Authenticode shell presentation and embedded-signature verification, common DPI scales, multi-monitor
placement, and sleep/wake plus network transitions. A real public Webhook receiver is optional local
acceptance because hosted tests deliberately create an inactive endpoint. Record only the tested
version, platform class, pass/fail checklist, and redacted defect references; never store workstation
identity, account data, deployment addresses, or credentials in the repository.

托管自动化优先且属于强制门禁；公开 Windows 版本还必须在维护中的 Windows 11 环境完成真实浏览器
焦点/回调交接、安装修复/卸载、Explorer 与原生文件对话框、托盘与普通用户启动、Authenticode
外壳显示及嵌入签名、常用 DPI、多显示器、睡眠唤醒与网络切换验收。真实公网 Webhook 接收器可作为
可选本地验收，因为托管测试会刻意创建非活动端点。仓库只记录版本、平台类别、通过/失败清单和脱敏
缺陷引用，不保存工作站身份、账户数据、部署地址或凭据。

Privilege acceptance additionally covers a managed sing-box TUN configuration, explicit elevated
Generic mode, authorization approval and cancellation, broker absence/content-identity mismatch, complete
Job/process-group termination, broker disconnect fail-closed behavior, configuration-update restart,
and login autostart without an authorization prompt. Verify one authorization prompt for the first
explicit elevated start, reuse for later starts/stops/restarts in the same application session, and a
fresh prompt after application exit. Broker-identity mismatch is required to fail closed regardless of
whether native signing is configured. Repeat the matrix for ordinary, automatic, and forced-standard
policy; an unassessable external configuration must remain ordinary until the user explicitly
overrides it. On Windows, repeat the broker process test from a launcher-owned outer Job and verify
that the broker's suspended child joins its nested kill-on-close Job without requesting breakaway.
Because no persistent elevated service is installed, every unattended elevated start
without an active application-session broker must skip without prompting and retain a clear
manual-start recovery path.

权限验收还必须覆盖托管 sing-box TUN 配置、通用程序显式管理员模式、授权批准与取消、权限代理缺失或
内容身份不匹配、完整 Job/进程组终止、代理断连后的失败关闭、配置更新重启，以及登录自启动不弹出授权
窗口。应验证首次显式管理员启动只授权一次、同一应用会话内后续启动/停止/重启复用该会话，并在应用退出
后重新授权；无论是否配置原生签名，权限代理内容身份不匹配都必须失败关闭。普通、自动和强制普通三种
策略均需验证；无法可靠评估的外部配置必须保持普通权限，直至用户明确覆盖。在 Windows 上还要从启动
器拥有的外层 Job 中运行代理进程测试，确认暂停子进程无需请求 breakaway 即可加入嵌套的
kill-on-close Job。当前版本不安装持久提权服务，因此不存在活动应用会话代理时，所有无人值守的管理员
权限启动必须无提示地跳过，并保留清晰的手动启动恢复路径。

When adding a regression, place the lowest deterministic test at the owning layer, then add a native
or live-browser scenario only when the defect crosses an OS, WebView, network, database, or packaging
boundary. Tests must retain idempotent operation identities for retries and must never weaken product
timeouts, authorization, validation, or secure storage merely to improve test reliability.

新增回归时，应先在权威所属层增加最低成本且确定性的测试；只有问题跨越操作系统、WebView、网络、
数据库或打包边界时才增加原生或实时浏览器场景。重试必须保留同一个幂等操作标识，且不得为了测试
稳定性削弱产品超时、授权、校验或安全存储规则。

Release/signing changes additionally run `scripts/test-cross-platform-signing.sh`, both Unix and
Windows staging regressions, publication recovery tests, release-security audit fixtures and the
Authenticode catalog-versus-embedded-signature fixture. The cross-platform test exercises unsigned
and ad-hoc macOS resolution plus isolated OpenPGP sign/verify/tamper rejection; native certificate,
notarization and WinTrust acceptance remain on their owning operating-system runners.
`scripts/test_client_release_metadata.py` also proves exact schema-3 identity/trust combinations,
duplicate-key rejection, deterministic report generation and report-tamper rejection.

发布或签名变更还必须执行 `scripts/test-cross-platform-signing.sh`、Unix 与 Windows 两套暂存回归、
发布恢复测试、发布安全审计夹具，以及 Authenticode 目录签名与嵌入签名区分夹具。跨平台测试覆盖
macOS 未签名/ad-hoc 配置解析及隔离 OpenPGP 签名、验证和篡改拒绝；原生证书、notarization 与
WinTrust 验收仍在各自操作系统 runner 上完成。`scripts/test_client_release_metadata.py` 还会验证
schema 3 身份/信任组合、重复 JSON 键拒绝、报告确定性生成以及报告篡改拒绝。
