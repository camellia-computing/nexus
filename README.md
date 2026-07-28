# Camellia Nexus

## 中文

Camellia Nexus 是一款 Windows 优先、兼容 Linux 与 macOS 的桌面程序生命周期管理器。它用于统一管理本地后台二进制程序的可执行文件、参数、工作目录、环境变量、运行状态、日志、配置和授权状态，适用于 generic command、sing-box、Xray、Mihomo 以及后续扩展的专用程序类型。

### 功能概览

- 独立管理每个 Program Profile 的状态、资源、日志和进程树
- 支持启动、停止、重启、失败退避、错峰自动启动和批量操作
- Windows 使用 Job Object，Unix 平台使用 process group 终止完整进程树
- 支持 managed 程序副本与 external 原位置运行模式
- 支持普通程序、sing-box、Xray、Mihomo，并为特定程序提供独立扩展能力
- 支持配置校验、格式化/导出预览、原子保存、失败回滚和实时日志
- 支持 sing-box/Xray 原生 JSON 与 Mihomo 原生 YAML 配置源的有序合并、手动刷新与定时刷新
- 支持 sing-box 原生 API、Clash API、Xray 本地 API Dashboard 与 Mihomo 外部 Web Dashboard
- 支持托盘控制、窗口状态恢复、系统登录启动、中英文界面和多套外观
- 支持授权核心、设备注册、短期 entitlement 租约、能力限制和数量限制
- 支持 Team 成员、邀请、设备入组与所有权治理，以及共享加密配置、云同步、告警、审计导出和 Webhook 管理

Team 新成员需先使用属于同一 Team 授权的激活码完成设备激活，再接受一次性邀请令牌；已有成员添加设备时使用由已绑定设备创建的独立 15 分钟入组令牌，不能复用成员邀请。无效、过期、已消费或类型错误的 Team 令牌只会产生可恢复的输入错误，不会清除仍然有效的设备授权会话。每次公开 Team 写入都携带一个规范 UUIDv4 操作标识；若结果不明确，界面会保留原请求并提供显式重试，服务端返回同一已提交结果，而不会重复创建邀请、成员或设备绑定。修改请求内容、命令或设备却复用操作标识会得到冲突。已完成关联的同一设备也可幂等重试已接受的令牌，即使令牌随后到期，也能从响应丢失中恢复。主动离组只有在服务端确认提交后才清除本地 Team 授权；丢失响应时会先以原会话、操作标识、成员 ID 与 row version 查询精确提交状态，其他已提交 Team 操作不能冒充离组结果。移除设备会在同一事务中撤销其会话并解除成员绑定；设备随后重新激活时保持未分配状态，不会恢复旧成员关系。Owner/Admin 可为仍处于活动状态且已无绑定设备的成员签发一次恢复入组令牌。

### 运行模型

参数以单行命令文本输入，并在提交前解析为独立 argv。进程通过操作系统 API 启动，不经过 shell。

- `managed`：导入程序目录并维护隔离副本，适用于独立部署或多实例运行
- `external`：直接使用现有可执行文件，不复制程序文件

两种模式均以可执行文件所在目录作为工作目录。sing-box、Xray 与 Mihomo 可选择手动配置或托管配置。sing-box/Xray 的手动配置保留用户命令行配置参数并可提供最终覆盖；Mihomo 可在外部配置路径与应用内存储的 YAML 配置之间选择。托管配置由有序原生配置源生成主配置，并禁止命令行配置路径覆盖。Generic 类型保持原始 argv，不进行语义改写。

配置源按界面顺序合并，后置值优先，最终结果必须通过对应二进制程序的原生校验后才会原子应用。本地源可使用绝对路径或工作目录相对路径；远程源仅接受 HTTPS，并可选 HTTP Basic 认证。自动更新开关只控制调度，不会重置已选间隔；关闭后再次启用仍恢复原间隔。单源限制 4 MiB，总读取限制 16 MiB。

配置编辑器将通用 JSON/YAML 语法、通用 JSON Schema 能力和程序专属语义分层处理。sing-box `1.14.0-beta.2` 及以上版本会从当前 Profile 的确切二进制文件按需生成 Draft 2020-12 Schema，用于结构诊断、属性/值补全以及 sing-box tag 引用补全；结果按可执行文件路径、文件元数据和已探测版本缓存，任一项变化后自动失效。客户端不会跟随配置或 Schema 中的外部引用，也不会自动下载任意 `$schema` 地址。Schema 暂时不可用时编辑器会降级为语法模式，而保存前的目标程序原生校验始终是最终语义门禁。

Mihomo 映射字段递归合并；同名 `proxies`、`proxy-groups` 与 `listeners` 由后置源原位替换，`rules` 等有序列表按界面中的源顺序连接，因此界面顺序同时决定规则优先级。

同一 external executable 只能关联一个 Program Profile。managed Profile 使用独立副本，可在目标程序支持并行运行时创建多个实例。

### 架构

项目由领域核心、授权核心、桌面集成和 Svelte 管理界面组成。Program Controller 负责单个程序的状态机；每种具体程序类型拥有独立模块，其 Adapter 只生成执行计划；平台层负责进程、文件系统和系统集成。

参考文档：

- [SECURITY.md](SECURITY.md)
- [docs/dependency-management.md](docs/dependency-management.md)
- [docs/licensing-architecture.md](docs/licensing-architecture.md)
- [docs/production-readiness-audit.md](docs/production-readiness-audit.md)
- [docs/testing.md](docs/testing.md)

### 开发与验证

要求：Rust 1.97、Node.js 24 LTS、pnpm 11，以及目标平台所需的 [Tauri 2 系统依赖](https://v2.tauri.app/start/prerequisites/)。

```bash
pnpm --dir ui install --frozen-lockfile
pnpm --dir ui desktop:dev
```

本地质量检查：

```bash
bash scripts/ci-local.sh
# ci-local 在默认相邻路径存在 management-server 时会同时校验跨仓 wire/semantic 协议及其 mutation test
# 非默认路径需依次执行：
# node scripts/check-cross-repo-contract.mjs <management-server-path>
# node scripts/test-cross-repo-contract.mjs <management-server-path>
```

```powershell
./scripts/ci-local.ps1
```

桌面目标检查与构建：

```bash
bash scripts/ci-local.sh --desktop-check
bash scripts/ci-local.sh --desktop-build
bash scripts/ci-local.sh --windows-check
bash scripts/ci-local.sh --windows-build --bootstrap-mingw
```

```powershell
./scripts/ci-local.ps1 -Mode DesktopCheck
./scripts/ci-local.ps1 -Mode DesktopBuild
```

Windows 原生端到端测试由 GitHub 官方 `windows-2025` Runner 默认执行，并使用一次性 WSL 服务端、
隔离的系统凭据命名空间及真实 Tauri/WebView2。维护工作站可选择动态 WSL2 Compose、SSH 隧道或显式
已有服务，不需要在脚本中固定机器和部署路径。执行范围与入口见[生产级验证](docs/testing.md)。

发布构建必须通过 Tauri CLI 完成，以启用 `custom-protocol` 并嵌入前端资源：

```bash
pnpm --dir ui desktop:build
```

### CI/CD 与签名

客户端以根 `Cargo.toml` 为唯一版本真源，并与服务端独立演进。`main` 门禁通过后，`Release Manager` 自动提出版本与变更日志；只有最终 head 获得人工批准且按规定 squash 合并后，才会在复核仓库策略与不可变 Release 设置后创建 tag 和 draft Release。原生平台签名和 Linux OpenPGP 制品签名均为可选项：未配置时允许发布未签名包，完整配置会启用对应签名，半套配置则立即失败；所有公开制品始终经过校验和与 Sigstore 验证。手动构建只产生按提交 SHA 标识的短期候选产物。完整流程与一次性 GitHub App 配置见 [发布规范](docs/releasing.md)。

Windows 桌面界面和登录自启动始终使用调用者的普通用户令牌；应用不会通过 UAC 或提权计划任务重新启动整个界面。Program Profile 默认为“自动检测 + 启动时授权”：Camellia Nexus 通过每个内置 Program Adapter 声明的配置入口和权限分析能力检查 TUN、透明代理和 Unix 特权监听端口。第一次由用户显式启动需要管理员权限的程序时，系统授权会建立一个仅在本次 Camellia Nexus 运行期间存在的最小权限代理会话；同一会话内的后续启动、停止、重启以及配置更新所需重启不再重复请求授权。代理可同时管理多个 Program，但只接受每个 Program 在本次会话首次批准的有界启动定义，并通过版本化回环协议关联每个请求和生命周期事件。应用退出或通道断开时会终止代理及其受管进程树；Windows 继续使用 Job Object，Unix 继续使用进程组。

当前版本不安装持久提权服务。登录自启动和其他无交互操作不会自行弹出系统授权窗口：已有会话时可复用，没有会话时则安全跳过并提示用户显式启动。普通模式与权限代理均不以代码签名作为功能开关；运行时会核对随该客户端构建固定的权限代理可执行文件内容身份，并忽略 Authenticode/Mach-O 原生签名容器本身，因此签名与未签名正式构建采用同一功能规则。Windows MSI 和便携 ZIP 都包含主程序与独立 `camellia-nexus-privilege-broker.exe`，两者必须保持在同一目录。公开分发仍应使用受信代码签名和 RFC 3161 时间戳来确认发布者身份并降低操作系统警告，但签名不是管理员功能的授权条件，也不保证 SmartScreen 信誉。

```powershell
$password = Read-Host "PFX password" -AsSecureString
./scripts/ci-local.ps1 -Mode DesktopBuild -Sign -PfxPath ".\codesign.pfx" -PfxPassword $password
```

签名配置与免费/自建开发 CA 指南见 [Windows code signing](docs/windows-code-signing.md)、[macOS code signing](docs/macos-code-signing.md) 与 [Linux artifact signing](docs/linux-artifact-signing.md)。跨仓库的 [CI/CD 基线](https://github.com/camellia-computing/.github/blob/main/docs/CI_CD_BASELINE.md) 和 [签名身份登记册](https://github.com/camellia-computing/.github/blob/main/config/signing-identities.json) 是自动审计与证书同步的组织级真源；仓库只保存公开身份，绝不保存私钥材料。

### 授权服务配置

客户端默认使用发布时嵌入的授权信任配置。生产构建可通过环境变量覆盖服务地址和 OAuth 参数：

- `CAMELLIA_NEXUS_LICENSE_URL`
- `CAMELLIA_NEXUS_AUTHORIZATION_ENDPOINT`
- `CAMELLIA_NEXUS_OAUTH_CLIENT_ID`
- `CAMELLIA_NEXUS_OAUTH_REDIRECT_URI`
- `CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH`

`CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH` 仅用于受控构建或测试替换授权信任配置。客户端只将刷新会话、设备凭据、授权缓存和可信时间记录保存到操作系统凭据存储。

授权请求固定使用单一产品 scope `camellia.nexus.license`。它不是可部署覆盖项；客户端和服务端必须对该协议值精确一致，且本服务不声明 OpenID Connect/ID token 能力。

### 数据目录

Windows 数据保存在 `%LOCALAPPDATA%\camellia-nexus`。Linux 与 macOS 使用系统提供的应用数据目录。managed Profile 保存独立程序副本、配置和日志；external Profile 仅保存必要元数据、日志及可选托管配置。

### 扩展程序类型

新增具体程序类型需要建立独立的 core、desktop 和 UI 模块，扩展 `ProgramKind` 与 `ProgramType`，实现并注册 `ProgramAdapter`，并补充身份探测、配置语义和参数测试。Adapter 不执行 I/O；短命令统一由受限 `ToolRunner` 执行。

### 许可证

Camellia Nexus 是需要 Camellia Computing 明确授权的专有软件。源代码可见性、仓库访问权或收到副本均不构成使用、复制、修改、分发、托管或再许可授权。完整条款见 [LICENSE](LICENSE)；第三方组件继续适用各自许可证，详见 [NOTICE](NOTICE)。

## English

Camellia Nexus is a Windows-first desktop lifecycle manager for local background binaries, with Linux and macOS support. It manages executable paths, arguments, working directories, environment variables, runtime state, logs, configuration and licensing state for generic commands, sing-box, Xray, Mihomo and future specialized program types.

### Features

- Isolated state, resources, logs and process-tree management for each Program Profile
- Start, stop, restart, failure backoff, staggered autostart and bulk operations
- Full process-tree termination through Windows Job Objects or Unix process groups
- Managed package copies and external in-place executable mode
- Generic Program, sing-box, Xray and Mihomo support with type-specific extensions
- Configuration validation, formatting/export actions, atomic save, rollback and live logs
- Ordered local or HTTPS native JSON sources for sing-box/Xray and native YAML sources for Mihomo
- Native sing-box API, Clash API, Xray local API and Mihomo external Web dashboards
- Tray controls, window-state restore, login startup, Chinese/English UI and multiple appearance themes
- Licensing core with device registration, short-lived entitlement leases, capability gates and numeric limits
- Team membership, invitations, device enrollment and ownership governance, plus encrypted shared configurations, cloud sync, alerts, audit export and Webhook management

A new Team member first activates the device with a code for the same Team license and then accepts the single-use invitation token. An existing member adds another device with a distinct 15-minute enrollment token created on an already bound device; member invitations are never reused for that purpose. Invalid, expired, consumed, or wrong-kind Team tokens produce recoverable input errors and never clear an otherwise valid device authorization session. Every public Team write carries a canonical UUIDv4 operation identity. If the result is ambiguous, the UI retains the original request behind an explicit retry and the service returns the same committed result instead of creating another invitation, member, or device binding; changing the request, command, or device while reusing that identity returns a conflict. A newly linked device can also replay its accepted token idempotently after later expiry. Voluntary leave clears local Team authorization only after the server confirms the commit; after a lost response, the client queries the exact status with the original session, operation identity, member ID, and row version, so another committed Team operation cannot masquerade as leave. Removing a device transactionally revokes its sessions and severs its member binding; later reactivation leaves it unassigned instead of restoring that relationship. An Owner or Admin can issue one recovery enrollment token for an active member who has no remaining bound device.

### Runtime model

Arguments are entered as one command line and parsed into argv before submission. Processes are launched through operating-system APIs and never through a shell.

- `managed`: imports a program directory and maintains an isolated copy
- `external`: uses an existing executable in place

Both modes use the executable directory as the working folder. sing-box, Xray and Mihomo support manual or managed configuration. sing-box/Xray manual mode preserves user-provided configuration arguments and may include a final override; Mihomo can use either an external configuration path or an application-stored YAML configuration. Managed mode builds the main configuration from ordered native sources and blocks command-line configuration path overrides. Generic Program keeps argv unchanged.

Configuration sources are merged in UI order. Later sources take precedence, and the generated result must pass the target binary’s native validation before atomic application. Local sources may be absolute paths or paths relative to the working folder. Remote sources must use HTTPS and may use HTTP Basic authentication. The automatic-update switch controls scheduling only and preserves the selected interval while disabled. Each source is limited to 4 MiB, with a 16 MiB total read limit.

The configuration editor separates generic JSON/YAML syntax, generic JSON Schema behavior, and program-specific semantics. sing-box `1.14.0-beta.2` or newer lazily generates a Draft 2020-12 Schema from the exact binary owned by the current Profile. It drives structural diagnostics, property/value completion, and sing-box tag-reference completion. Results are cached by executable path, file metadata, and detected version, then invalidated when any of those values changes. The client never follows external references from configuration or Schema content and never downloads an arbitrary `$schema` URL. If Schema support is temporarily unavailable, the editor falls back to syntax mode; the target program’s native validator remains the final semantic gate before saving.

For Mihomo, mappings merge recursively; later sources replace same-name `proxies`, `proxy-groups`, and `listeners` in place, while ordered lists such as `rules` are concatenated in UI source order, so that order also defines rule priority.

The same external executable can be referenced by only one Program Profile. Managed Profiles use independent copies and may run multiple instances when the target program supports it.

### Architecture

The project is organized into domain core, licensing core, desktop integration and the Svelte management UI. The Program Controller owns the state machine for one program. Each concrete program type has its own module; its Adapter only produces execution plans. Platform modules own process, filesystem and operating-system integration.

References:

- [SECURITY.md](SECURITY.md)
- [docs/dependency-management.md](docs/dependency-management.md)
- [docs/licensing-architecture.md](docs/licensing-architecture.md)
- [docs/testing.md](docs/testing.md)

### Development and verification

Requirements: Rust 1.97, Node.js 24 LTS, pnpm 11 and the target platform’s [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
pnpm --dir ui install --frozen-lockfile
pnpm --dir ui desktop:dev
```

Run local quality checks:

```bash
bash scripts/ci-local.sh
# ci-local also checks the cross-repository wire/semantic contract and its mutation test when the
# management-server is at its default adjacent path. For another path, run both contract scripts.
# node scripts/check-cross-repo-contract.mjs <management-server-path>
# node scripts/test-cross-repo-contract.mjs <management-server-path>
```

```powershell
./scripts/ci-local.ps1
```

Desktop checks and builds:

```bash
bash scripts/ci-local.sh --desktop-check
bash scripts/ci-local.sh --desktop-build
bash scripts/ci-local.sh --windows-check
bash scripts/ci-local.sh --windows-build --bootstrap-mingw
```

```powershell
./scripts/ci-local.ps1 -Mode DesktopCheck
./scripts/ci-local.ps1 -Mode DesktopBuild
```

Native Windows end-to-end tests run by default on GitHub's `windows-2025` runner with a disposable
WSL server, isolated operating-system credential namespace, and the real Tauri/WebView2 application.
A maintained workstation can instead select dynamic WSL2 Compose, an SSH tunnel, or an explicitly
provided service without embedding a machine or deployment path. See
[production verification](docs/testing.md) for scope and commands.

Release builds must go through Tauri CLI so `custom-protocol` is enabled and frontend assets are embedded:

```bash
pnpm --dir ui desktop:build
```

### CI/CD and signing

The client uses the root `Cargo.toml` as its sole version source and advances independently from the server. After `main` passes its gates, `Release Manager` proposes the version and changelog. Only an exact-head human-approved Release PR merged with the required squash policy can create the tag and draft Release after repository policy and immutable-Release checks. Native platform signing and Linux OpenPGP artifact signing are optional: absent configuration produces unsigned packages, a complete group enables its corresponding signing mode, and a partial group fails closed. Checksums and Sigstore verification remain mandatory for every public asset. Manual builds produce short-lived candidates identified by commit SHA. See the [release policy](docs/releasing.md) for the workflow and one-time GitHub App setup.

Windows builds run the desktop UI with the caller's normal user token, including Start at login. The application never relaunches the full UI through UAC or an elevated scheduled task. Program Profiles default to automatic detection with authorization at start: each built-in Program Adapter declares its configuration inputs and privilege-analysis capability. The first explicit start that needs administrator access opens a minimal broker for the current Camellia Nexus application session. Later starts, stops, restarts, and configuration-driven restarts reuse that session without repeated prompts. The broker multiplexes multiple Programs while retaining the first bounded launch definition approved for each Program, and correlates every lifecycle event over its versioned loopback protocol. Application exit or connection loss terminates the brokered process trees. Windows Job Objects and Unix process groups remain authoritative for cleanup.

No persistent elevated service is installed. Non-interactive login startup and automatic work never create a system authorization prompt: they reuse an existing application-session broker or safely defer until an explicit start. Code signing is not a functional gate for the broker. Runtime verifies the broker executable's content identity pinned into the matching desktop build while normalizing native Authenticode/Mach-O signature containers, so signed and unsigned formal builds follow the same authorization behavior. Both the Windows MSI and portable ZIP contain `camellia-nexus.exe` and `camellia-nexus-privilege-broker.exe` side by side. Public distributions should still use trusted SHA-256 Authenticode signing with an RFC 3161 timestamp to establish publisher identity and reduce operating-system warnings; it does not guarantee SmartScreen reputation.

```powershell
$password = Read-Host "PFX password" -AsSecureString
./scripts/ci-local.ps1 -Mode DesktopBuild -Sign -PfxPath ".\codesign.pfx" -PfxPassword $password
```

See [Windows code signing](docs/windows-code-signing.md), [macOS code signing](docs/macos-code-signing.md) and [Linux artifact signing](docs/linux-artifact-signing.md) for CI configuration and controlled/self-managed signing guidance. The organization-wide [CI/CD baseline](https://github.com/camellia-computing/.github/blob/main/docs/CI_CD_BASELINE.md) and [signing identity registry](https://github.com/camellia-computing/.github/blob/main/config/signing-identities.json) are the audited sources for cross-repository policy and public certificate metadata; private key material is never recorded there.

### License service configuration

The client uses the embedded license trust configuration by default. Production builds may override service and OAuth settings through:

- `CAMELLIA_NEXUS_LICENSE_URL`
- `CAMELLIA_NEXUS_AUTHORIZATION_ENDPOINT`
- `CAMELLIA_NEXUS_OAUTH_CLIENT_ID`
- `CAMELLIA_NEXUS_OAUTH_REDIRECT_URI`
- `CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH`

`CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH` is only for controlled builds or tests that replace license trust metadata. The client stores refresh sessions, device credentials, entitlement cache and trusted-time records in the operating-system credential store.

Authorization requests use the single fixed product scope `camellia.nexus.license`. It is not a deployment override: client and server must agree on the exact protocol value, and this service does not claim OpenID Connect or ID-token support.

### Data directory

Windows data is stored in `%LOCALAPPDATA%\camellia-nexus`. Linux and macOS use the platform application-data directory. Managed Profiles store isolated package copies, configuration and logs. External Profiles store only required metadata, logs and optional managed configuration.

### Adding program types

Add a dedicated core, desktop and UI module for each concrete program type. Extend `ProgramKind` and `ProgramType`, implement and register `ProgramAdapter`, then add probing, configuration semantics and argument tests. Adapters must not perform I/O; short-lived commands run through the restricted `ToolRunner`.

### License

Camellia Nexus is proprietary software and requires express authorization from Camellia Computing. Source visibility, repository access, or possession of a copy does not grant rights to use, copy, modify, distribute, host, or sublicense it. See [LICENSE](LICENSE). Third-party components remain under their respective licenses as described in [NOTICE](NOTICE).
