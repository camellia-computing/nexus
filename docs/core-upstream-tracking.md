# Core upstream version tracking / Core 上游版本跟踪

This document defines how Camellia Nexus tracks, distinguishes, reviews, and tests the two supported
upstream lines for each built-in proxy Core. It is a source-compatibility contract, not an automatic
binary updater. The canonical machine-readable moving-selector snapshot is
[`core-upstream-versions.json`](../crates/camellia-nexus-core/core-upstream-versions.json); historical
version/feature evidence is [`core-compatibility-catalog.json`](../crates/camellia-nexus-core/core-compatibility-catalog.json).

本文规定 Camellia Nexus 如何跟踪、区分、审查和测试每个内置代理 Core 的两条上游版本线。它是源
代码兼容性契约，不是二进制自动更新器。机器可读的动态选择器快照是
[`core-upstream-versions.json`](../crates/camellia-nexus-core/core-upstream-versions.json)，历史版本/功能
证据是 [`core-compatibility-catalog.json`](../crates/camellia-nexus-core/core-compatibility-catalog.json)。

## Channel contract / 通道契约

| Core | Development policy / 开发通道策略 | Stable policy / 稳定通道策略 |
| --- | --- | --- |
| Xray | exact head of `XTLS/Xray-core:main` / `main` 精确头提交 | GitHub latest stable Release / GitHub 最新稳定 Release |
| Mihomo | exact head of `MetaCubeX/mihomo:Alpha` / `Alpha` 精确头提交（大小写敏感） | GitHub latest stable Release / GitHub 最新稳定 Release |
| sing-box | exact head of `SagerNet/sing-box:testing` / `testing` 精确头提交 | GitHub latest stable Release / GitHub 最新稳定 Release |

`main`, `Alpha`, `testing`, and `latest` are moving selectors. They must never appear alone as a
reproducible compatibility identity. Every observed selector is resolved to an exact 40-character
commit SHA; a stable resolution additionally records the exact release tag. The manifest records only
the moving-selector policy and exact upstream evidence. The historical compatibility catalog is a
separate reviewed artifact; it owns feature events, surface history, and extractor revision.

`main`、`Alpha`、`testing` 与 `latest` 都是会移动的选择器，不能单独作为可复现兼容身份。每次观察
都必须解析到 40 位精确 commit SHA；稳定通道还必须保存精确 Release tag。清单只保存选择器策略与
精确上游证据；功能事件、字段历史与提取器修订由独立的历史兼容目录负责。

The stable selector means GitHub's current `releases/latest` result with both `draft=false` and
`prerelease=false`. It does not mean “the greatest tag that happens to parse as SemVer.” A repository
owner can control which qualifying Release is marked latest, so both the returned tag and its peeled
commit are pinned.

稳定选择器表示 GitHub 当前 `releases/latest` 的结果，并要求 `draft=false`、`prerelease=false`；它
不表示“所有可解析 SemVer tag 中数值最大者”。仓库维护者可以控制哪个合格 Release 被标记为 latest，
因此必须同时固定返回的 tag 及其最终 commit。

## Current resolved baseline / 当前解析基线

Observed at `2026-08-11T10:39:26Z`:

| Core | Channel / 通道 | Resolved source / 精确来源 | Compatibility catalog / 兼容目录 |
| --- | --- | --- | --- |
| Xray | `main` | [`bc6e966af890`](https://github.com/XTLS/Xray-core/commit/bc6e966af890d0ef481501ec171321ec802c6857) | `core-history-v1-20260811` |
| Xray | stable `latest` | [`v26.3.27` / `d2758a023cd7`](https://github.com/XTLS/Xray-core/releases/tag/v26.3.27) | `core-history-v1-20260811` |
| Mihomo | `Alpha` | [`e183c580082a`](https://github.com/MetaCubeX/mihomo/commit/e183c580082a32e567157924613af8fe14f4e070) | `core-history-v1-20260811` |
| Mihomo | stable `latest` | [`v1.19.29` / `e26714a181ac`](https://github.com/MetaCubeX/mihomo/releases/tag/v1.19.29) | `core-history-v1-20260811` |
| sing-box | `testing` | [`4902660f8424`](https://github.com/SagerNet/sing-box/commit/4902660f8424fef3c2a60dfcdce7aeadfe3f3b88) | `core-history-v1-20260811` |
| sing-box | stable `latest` | [`v1.13.18` / `45ca32dcb966`](https://github.com/SagerNet/sing-box/releases/tag/v1.13.18) | `core-history-v1-20260811` |

The JSON manifest, rather than this rendered table, is authoritative when they disagree. Updating the
table and manifest belongs to the same reviewed change.

若表格与 JSON 清单不一致，以 JSON 清单为准；二者必须在同一个经审查的变更中同步更新。

## Version and capability architecture / 版本与能力架构

The Core crate separates five concepts that must not be collapsed:

- `CoreBinaryFingerprint` is the exact SHA-256/size/mtime identity of the bytes selected by a Profile.
  Bounded streaming fingerprint refresh invalidates native evidence when the binary changes.
- `CoreProbeReport` is untrusted binary output (`version`, CLI observations, and reported commit). It
  may inform compatibility resolution but never proves official origin.
- `CoreCompatibilityPreference` is the user's Automatic, catalogued Release, known Commit, or Unknown
  choice. `CoreTargetIdentity` records the resulting coordinate, basis, report, and fingerprint.
- `CoreCompatibilityProfile` resolves the historical catalog's version/feature events. Unknown and
  uncatalogued targets remain attemptable; decisions are diagnostics, not a hard support gate.
- `CoreValidationEvidence` is candidate-only and binds native acceptance to the exact fingerprint,
  profile hash, and candidate hash. It never upgrades a global support conclusion.

Core crate 将以下五个概念严格分离：

- `CoreBinaryFingerprint` 表示 Profile 当前选择的二进制字节的精确 SHA-256/大小/修改时间身份；有界
  流式 fingerprint 刷新后，二进制变化会使 native evidence 失效。
- `CoreProbeReport` 表示二进制输出的非信任报告（version、CLI 观察与报告 commit），可以参与兼容
  解析，但不能证明官方来源。
- `CoreCompatibilityPreference` 表示用户选择 Automatic、目录 Release、已知 Commit 或 Unknown；
  `CoreTargetIdentity` 独立记录坐标、依据、报告和 fingerprint。
- `CoreCompatibilityProfile` 解析历史目录的版本/功能事件。Unknown 与 Uncatalogued 仍允许尝试；能力
  决定是诊断信息，不是硬性支持门禁。
- `CoreValidationEvidence` 仅属于 candidate，绑定 native acceptance 的精确 fingerprint、profile hash
  与 candidate hash，不会提升全局支持结论。

Installed binaries therefore remain user-selected/custom binaries even if their output contains
`alpha`, `beta`, or a commit-looking suffix. A future managed downloader may assign trusted origin
only when its package manifest cryptographically or transactionally binds the downloaded bytes to the
recorded repository/ref/SHA. The UI may display reported text and a “reported version is not verified
official source” diagnostic, but it must not present an inferred channel as fact.

因此，即使已安装二进制输出包含 `alpha`、`beta` 或类似 commit 的后缀，也仍是用户选择的自定义
二进制。未来托管下载器只有在包清单以可验证方式将下载字节绑定到记录的 repository/ref/SHA，才能
赋予可信来源。UI 可以显示报告文本并提示“报告版本不等于已验证官方来源”，但不得把推测通道当作事实。

Every translated share item records parser revision, translator revision, target identity, profile
hash, catalog revision, feature decisions, and translation fidelity. A target/profile change reparses
the original sidecar, rebuilds Guided/Raw intent, and requires native validation again. If a share has
zero accepted items for the new target, its prior snapshot, Applied, and LKG remain intact while the
new candidate is blocked for review.

每个分享转换项记录 parser revision、translator revision、目标身份、profile hash、catalog revision、
功能决定与转换保真度。目标/profile 变化会从原始 sidecar 重解析、重建 Guided/Raw intent，并再次执行
native validation。若新目标下分享来源零项可接受，则保留旧 Snapshot、Applied 与 LKG，并阻止新
candidate 进入 Apply，等待用户处理。

## Catalog and feature change rules / 目录与功能变更规则

The append-only compatibility catalog indexes every official release and prerelease plus reviewed
feature-change anchors; it intentionally does not claim knowledge of every ordinary development
commit. A feature has a stable id, lifecycle, behavior revision, and versioned evidence event. Moving
a selector with no behavioral difference still updates the exact upstream manifest and review record;
a parser, translator, schema, validation, launch, merge, or runtime behavior change also updates the
catalog event/behavior revision and its fixtures.

追加式兼容目录索引全部官方正式版、预发布版及已审查的功能变更锚点，但不会假装掌握每个普通开发
commit。每项功能具有稳定 id、生命周期、行为修订和版本化证据事件。即使选择器移动后没有行为差异，
也必须更新精确上游清单与审查记录；若 Parser、Translator、Schema、校验、启动、merge 或 runtime
行为变化，还必须同步更新目录事件/行为修订及 fixture。

Development and stable profiles are independent. Never copy a development capability into stable
because “the development version is newer,” and never hide a development regression behind the
stable profile. If an exact semantic combination cannot be represented by the selected profile, the
translator rejects it or emits an explicitly classified safe warning. Native Core validation remains
the final gate, but it does not replace client-side fidelity checks.

开发和稳定能力档案互相独立。不得因为“开发版更新”就把开发能力复制给稳定版，也不得用稳定档案
掩盖开发版回归。选中档案无法表达完整语义组合时，Translator 必须拒绝，或仅对明确安全的降级给出
分类警告。原生 Core 校验始终是最终门禁，但不能替代客户端的语义保真检查。

## Required update workflow / 必须执行的更新流程

1. Run `python3 scripts/check-core-upstreams.py --live`. A stale result is expected when upstream
   moved; copy nothing blindly from terminal output.
2. Resolve the symbolic selector again through the GitHub API and record the exact SHA, tag (stable),
   upstream timestamp, canonical URL, and a fresh UTC `observedAt`.
3. Review the exact old-to-new compare range for each moved channel. Inspect at least:
   CLI/probe output and flags; configuration schema, defaults and validators; share protocols,
   transports, authentication and security extensions; renamed/deprecated/removed fields; merge and
   ordered-list behavior; runtime reload/start behavior; platform/build-tag differences.
4. Classify every relevant change as no client impact, parser change, translator/capability change,
   semantic adapter change, native-validation change, fixture-only change, or blocking uncertainty.
   An uncertainty blocks claiming support.
5. Update feature events, behavior revisions, Adapter/Translator code, and fixtures when behavior
   changes. Never modify a feature decision without boundary-version tests and an unaffected target.
6. Update the JSON manifest and this current-baseline table together. Append an update record below;
   never rewrite or delete previous records merely because a newer upstream exists.
7. Run the offline and live manifest checks, Core/desktop Rust suites, Clippy, UI type/tests/build,
   share compatibility fixtures, source reparse/rebase state tests, and native Core compatibility
   matrix where reproducible binaries are available.
8. Commit the source identity, compatibility catalog, behavior logic, fixtures, documentation, and
   test evidence as one reviewable transaction. Do not merge a “version-only” pin that leaves the
   historical catalog or evidence stale.

1. 执行 `python3 scripts/check-core-upstreams.py --live`。上游移动时出现 stale 属于预期，不得盲目
   复制终端内容。
2. 重新通过 GitHub API 解析符号选择器，记录精确 SHA、稳定通道 tag、上游时间、规范 URL 和新的
   UTC `observedAt`。
3. 审查每条移动通道的精确 old-to-new compare 范围，至少检查：CLI/probe 输出与参数；配置 Schema、
   默认值和 validator；分享协议、传输、认证与安全扩展；重命名/弃用/删除字段；merge 与有序列表
   行为；runtime reload/start 行为；平台和 build-tag 差异。
4. 将相关变化分类为：客户端无影响、Parser 变化、Translator/能力变化、Semantic Adapter 变化、
   原生校验变化、仅 fixture 变化或阻塞性不确定。存在不确定性时不得声明支持。
5. 行为改变时更新功能事件、行为修订、Adapter/Translator 代码及 fixture。任何功能决定修改都必须
   同时覆盖边界版本与一个不受影响的目标。
6. 同步更新 JSON 清单和本文当前基线表，并在下方追加更新记录；不得因出现新上游而改写或删除历史。
7. 执行离线/在线清单校验、Core/Desktop Rust、Clippy、UI 类型/测试/构建、分享兼容 fixture、Source
   reparse/rebase 状态测试，以及在有可复现二进制时执行原生 Core compatibility matrix。
8. 将源身份、兼容目录、行为逻辑、fixture、文档和测试证据作为一个可审查事务提交。禁止只移动版本
   pin、却让历史目录或证据继续停留在旧版本。

## Update record / 更新记录

### 2026-08-11 — Dual-channel baseline established / 建立双通道基线

- Added Xray `main` + stable latest, Mihomo `Alpha` + stable latest, and sing-box `testing` + stable
  latest as six independently resolved identities.
- Preserved the prior stable research points (`v26.3.27`, `v1.19.29`, `v1.13.18`) as the current
  stable resolutions, while adding exact development SHAs.
- Established the initial exact selector manifest. Historical features and candidate evidence are now
  owned by the separate compatibility catalog and profile model described above.
- Version-scoped share translation provenance records target/profile/catalog decisions. Xray TUIC v5
  remains explicitly unsupported.
- Initial evidence: embedded-manifest unit tests, share compatibility tests, native validator as final
  apply gate, and offline/live GitHub manifest validation. Native binary matrix evidence must be added
  when reproducible six-channel binaries are available.

- 新增 Xray `main` + 稳定 latest、Mihomo `Alpha` + 稳定 latest、sing-box `testing` + 稳定 latest，
  共六个独立解析身份。
- 原研究稳定基线 `v26.3.27`、`v1.19.29`、`v1.13.18` 继续作为当前稳定解析结果，同时补入精确开发
  SHA。
- 建立初始精确选择器清单；历史功能与 candidate evidence 现由上文独立的兼容目录/profile 模型负责。
- 分享转换 provenance 记录 target/profile/catalog 决定；Xray 的 TUIC v5 保持明确 Unsupported。
- 初始证据包括：嵌入清单单元测试、分享兼容测试、Apply 前原生 validator 最终门禁、离线/在线 GitHub
  清单校验。获得可复现的六通道二进制后，必须继续补充原生矩阵证据。

### 2026-08-12 — Historical compatibility catalog / 全历史兼容目录

- Added all official release/prerelease identities and reviewed configuration-surface events: Xray
  131 versions/873 fields, Mihomo 104/1038, and sing-box 611/1199.
- Separated fingerprint, probe report, preference, target, profile, and candidate validation evidence.
  A catalogued release commit inherits its release feature events, while an arbitrary development SHA
  remains Unknown. Binary-reported versions never prove official source.
- Bound activation to exact binary/profile/config evidence and added retarget/reparse/rebase behavior.
  Zero-acceptance share retargets retain old Snapshot/Applied/LKG and block the new candidate.

- 索引全部官方正式版/预发布版及已审查配置字段事件：Xray 131 版本/873 字段、Mihomo 104/1038、
  sing-box 611/1199。
- 分离 fingerprint、probe report、preference、target、profile 与 candidate validation evidence。
  已编入目录的 Release commit 继承对应 Release 功能事件；任意开发 SHA 保持 Unknown；二进制报告
  版本绝不证明官方来源。
- 启动绑定精确 binary/profile/config evidence，并实现 retarget/reparse/rebase；新目标零项可接受时保留
  旧 Snapshot/Applied/LKG 并阻止新 candidate。

For later updates, append the date, channel, old exact ref, new exact ref, compare URL, reviewed
change categories, catalog/behavior revision decision, test evidence, and known limitations.

后续更新必须追加：日期、通道、旧精确 ref、新精确 ref、compare URL、已审查变化分类、目录/行为修订
决策、测试证据和已知限制。

## Verification commands / 校验命令

```bash
python3 scripts/check-core-upstreams.py
python3 scripts/check-core-upstreams.py --live
cargo test -p camellia-nexus-core --offline
cargo test -p camellia-nexus --no-default-features --offline --lib
cargo clippy -p camellia-nexus-core -p camellia-nexus --all-targets --no-default-features --offline -- -D warnings
```

The offline check is deterministic and suitable for normal CI. The live check is an explicit
maintenance signal: upstream movement should fail it until the review workflow above updates the
baseline. It must not silently rewrite source-controlled files.

离线检查是确定性的，适合常规 CI。在线检查是显式维护信号：上游移动后应持续失败，直到完成上述审查
流程并更新基线；它绝不能静默改写受版本控制的文件。
