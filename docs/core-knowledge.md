# Core knowledge and admission / Core 能力知识与准入

## Maintenance window / 维护窗口

Only official stable releases in two release families are maintained. sing-box and Mihomo use
major/minor families. Xray uses the two most recent year/month families with stable releases.
The official latest stable release anchors the window; development branches, prereleases, and
module-only alias tags do not define baselines. Every retained patch is assessed independently.

仅维护两个系列中的官方稳定发布。sing-box 与 Mihomo 按主次版本分组，Xray 按实际存在稳定发布的
最近两个年月分组。窗口以官方最新稳定发布为锚点；开发分支、预发布和仅供模块引用的标签不作为基线。
窗口内各补丁版本独立核对能力，不将最新补丁能力扩展到整个系列。

## Evidence and rules / 证据与规则

The offline knowledge artifact pins release tags, commits, source files, and symbols. Go AST
extraction inventories declarations, embedded fields, JSON/YAML/proxy tags, custom decoders, and
build constraints. Program adapters resolve these declarations into configuration paths. Reviewed
rules describe defaults, types, units, references, identity, dependencies, exclusions, and platform
conditions. Structural coverage and semantic coverage are independent measurements.

Type shapes retain pointers, sequences, mappings, generic arguments, and imported named references.
Each release records the module identity from its exact source commit. Reference coverage distinguishes
resolved declarations from unresolved dependencies and opaque forms; an unresolved reference is not
a statement that its fields are supported.

Reviewed dependencies are resolved from each release's exact module manifest. The maintenance tool
checks the Go module identity and checksums, independently verifies a release tag against its Git
commit (or the full commit for a pseudo-version), and reads source blobs from the reviewed repository.
Dependency declarations use full import-path namespaces; each file/symbol reference records its
own module and resolves to that module's commit, not the program's commit. sing-box's JSON helpers
and option value types are included this way. Dependency updates appear in the patch-change report;
extracting a decoder body records review evidence, not automatic approval of its semantics.

Callable aliases retain their target expression, import identities, and build condition alongside
function bodies. The behavior report records import rebinding even when a body digest is unchanged.
Generation rejects a producer change while extraction is in progress, so a directory never claims
one producer identity for a mixed extraction result.

离线目录固定发布标签、提交、源码文件和符号。Go AST 提取声明、嵌入字段、JSON/YAML/proxy 标签、
自定义解码器与构建条件，再由程序适配器映射配置路径。经审查规则描述默认值、类型、单位、引用、
身份、依赖、互斥和平台条件。结构覆盖与语义覆盖分别统计。

类型结构保留指针、序列、映射、泛型参数和导入的具名引用。每个发布从精确源码提交记录模块身份。
引用覆盖报告区分已解析声明、未解析依赖与不透明结构；未解析引用不代表其中字段已获支持。

经审查依赖从每个发布的精确模块清单解析。维护工具核对 Go 模块身份与校验和，独立验证发布标签
对应的 Git 提交，或伪版本对应的完整提交，再从已审查仓库读取源码。依赖声明使用完整导入路径命名空间；
文件与符号引用明确所属模块，解析到依赖自身的提交，而非程序提交。sing-box 的 JSON 辅助解码器与
配置值类型按此方式提取。补丁差异报告记录依赖更新；提取解码函数仅提供审查证据，不自动认可其语义。

函数转接入口与函数体一同保留目标表达式、导入身份和构建条件。即使函数体摘要未变，导入来源变化
也进入行为审查报告。提取过程中若生成工具发生变化，本次生成失败，不发布混合来源的目录。

`core-knowledge-changes.json` separates structural changes from function behavior requiring review.
File movement within a package and comment/layout changes do not become removed capabilities.
Reviewed rules pin normalized AST body digests; changed behavior requires renewed source review.
Schema annotations are recorded independently and do not assert equivalent native enforcement.
The producer and review report digests are part of the knowledge identity. Probe and validator
implementation identities derive from the compiled Core source digest, not a calendar or version label.

`core-knowledge-changes.json` 区分结构变化和需要审查的函数行为。同包文件移动以及注释、排版变化
不会被记成能力删除。经审查规则固定规范化 AST 函数体摘要，行为变化必须重新审查。Schema 注解
独立记录，不宣称原生检查具备完全相同的约束。提取器和报告摘要共同参与目录身份；探测与校验实现
身份从参与构建的 Core 源码摘要生成，不依赖日期或版本标签。

Decoder bindings record the object path, discriminator, options declaration, wire encoding,
constructor, build branches, and exact source behavior digests. Mihomo proxy dispatch follows its
`proxy` decoder; Xray dispatch records protocol aliases and the nested `settings` object. sing-box
inbound/outbound dispatch follows the registration call graph and separates rejecting constructors
from usable registration branches. Constructor bodies are evidence for review, not a claim of
installed functionality. Other registries and dependency-defined configuration types require their
own extraction adapters. Reviewed field rules also bind the field type, tags, and annotations so a
renamed field cannot silently retain a rule for a different path.

解析入口记录对象路径、类型判别字段、配置声明、编码标签、构造器、构建分支与精确函数行为摘要。
Mihomo 代理跟随 `proxy` 解码；Xray 记录协议别名和嵌套 `settings`；sing-box 入站/出站沿注册调用链
追踪，区分直接拒绝的构造器和实际注册分支。构造器源码用于审查，不代表当前安装文件具备该能力。
其他注册表及依赖库配置类型需要各自的提取适配器。字段规则同时绑定类型、标签和注解，字段改名后
不能静默沿用指向其他路径的规则。

Outbound collections and share-protocol aliases live in the same program descriptor. Source
profiles resolve each feature from that release's decoder and constructor bindings, retaining its
build conditions and immutable evidence. Their conclusions are source-declared, source-unavailable,
or unconfirmed; none of these claims that the installed executable implements the feature. Program
identity and feature resolution both bind the knowledge digest and exact stable source commit.

出站集合和分享协议别名由同一程序描述符登记。源码档案从对应发布的解码器和构造器记录解析功能，
保留构建条件与不可变证据。结论分为“源码已声明”“源码未提供”“尚未确认”，均不代表当前安装文件
已实现该功能。程序身份与功能解析同时绑定知识库摘要和精确稳定提交。

An unreviewed condition is unknown, not supported. Binary-generated schemas and dedicated probes
can establish additional field evidence for a maintained self-compiled baseline. An open-ended
schema or a zero validator exit cannot prove that an unknown field is consumed. Candidate validation
never certifies every capability or the safety of arbitrary executable code.

未审查的条件不能标记为支持。维护范围内的自编译程序可通过自身生成的结构或专门探测提供新增字段
证据。允许任意字段的结构或校验退出码为零不能证明未知字段生效。候选校验不证明全部能力，也不
保证任意可执行代码安全。

Build expressions are evaluated only for constructors used by the candidate. The evaluator retains
unknown observations, follows Go boolean precedence and implicit platform aliases, and limits input
size and nesting. An absent compiler or architecture-feature observation is not a false value.
sing-box's explicit tag line reports user build tags; Mihomo reports selected feature names. For each stable patch, extraction binds the exact version
output call, list builder, and both compiler branches for a reported flag. A flag absent from an
explicit report can be excluded only when that release proves it is reported. Other missing flags
remain unknown. Named constructors that directly return no instance
are recorded as rejecting branches as well as inline rejecting constructors. Neither a registered
name nor a successful native check overrides a configured feature's rejecting build branch.

仅检查候选实际使用的构造器条件。计算保留未知证据，遵循 Go 布尔优先级和隐式平台别名，限制输入长度与
嵌套。缺少编译器或架构特性信息不等于该条件为假。sing-box 明确输出的 Tags 行报告编译标签；
Mihomo 输出选定的功能名称；逐补丁固定版本输出调用、列表生成函数和标签开关两条编译分支。
只有该发布明确会报告的标签，才能从明确输出的列表中判断其未启用；其他缺失标签仍为未知。
直接返回空实例的具名构造器与
内联拒绝构造器都记录为拒绝分支。注册了名称或原生检查成功，均不能覆盖当前配置功能的构建拒绝条件。

## Configuration field evidence / 配置字段证据

The extractor binds each input to its exact module identity. For packages present in that source
inventory, implicit imports use the declared Go package name, not the final directory name.
Explicit aliases remain authoritative. Types, callable aliases, decoder registries, and build-tag
reports share the same import resolver. Executable packages are not importable libraries;
inconsistent library package names or duplicate local import names require review.

提取输入绑定精确模块身份。已收录源码包的隐式导入采用 Go 声明的包名，不以目录末段代替；
显式别名保持其声明语义。类型、函数转接入口、解析注册表和构建标签报告共用同一导入解析逻辑。
可执行包不作为可导入库；库包名称不一致或局部导入重名时要求重新审查。

The backend follows each maintained patch's AST types from the native root through known objects,
pointers, sequences, and embedded fields. Independent protocol options use their source decoder binding.
JSON and YAML encoding rules remain distinct. An undeclared field requires an explicit typed property
from the current binary's schema at the complete document path, including declarations
for the extension's actual object fields and array elements. Local static references and conjunctive
declarations can provide this evidence. Permissive maps, conditional alternatives, and undeclared
descendants cannot establish consumption. Native validation still checks candidate semantics.

后端从各维护补丁的 AST 根声明沿已知对象、指针、序列和嵌入字段查找；独立协议选项由源码解析入口
选择。JSON 与 YAML 编码规则保持区分。未声明字段需要当前二进制在完整文档路径上提供明确的类型属性声明，
且覆盖该扩展实际使用的对象字段与
数组元素。静态本地引用和组合声明可提供证据；开放映射、条件分支与未声明后代不能证明字段被使用。
候选语义仍须经过原生校验。

Schema loading is deduplicated by executable identity and rejects external references, nested
resource rebasing, oversized output, and excessive nesting. Evidence traversal has a shared work
budget. Literal examples/defaults are data rather than schema locations. Diagnostics contain paths,
codes, and declaration digests, not configuration values. Editor locations use document indexes for
arrays without copying user labels into technical diagnostics.

结构读取按可执行文件身份去重，拒绝外部引用、嵌套资源重新定基、超限输出和过深结构；证据遍历使用
共同工作预算。示例和默认值按数据处理，不误识别为结构引用。诊断只含路径、代码和声明摘要，不含配置值。
编辑器定位在数组中使用文档下标，不将用户标签复制到技术诊断。

Xray protocol comparison is bound to its source loader's lowercasing and dispatch behavior. JSON key
case variants retain their actual document locations and cannot skip field, semantic, or build checks.
Mihomo YAML keys are case-sensitive. Object traversal does not infer the shape of dynamic maps or
custom decoding branches. Flat protocol unions require the envelope, shared decoded options, and
their program-specific decoding rules; option struct membership alone is insufficient.

Flat object layouts distinguish envelope key matching from protocol key matching. When the decoder
excludes a matching envelope key, a same-named protocol member cannot consume it. Nested protocol
objects retain their own JSON matching behavior. Custom decoders require separate evidence and are
not treated as ordinary structures merely because an underlying declaration exists.

Xray 协议名比较绑定源码加载器的大小写规范化与分发行为。JSON 键的大小写变体保留实际文档位置，
不能绕过字段、语义或构建检查；Mihomo 的 YAML 键区分大小写。对象遍历不推断动态映射或自定义
解码分支的结构。平铺协议联合结构需要公共字段、共享解码选项及对应的解析规则，仅凭协议选项结构
不能确定整个对象的能力范围。

平铺对象分别记录公共键与协议键的匹配方式。解析器排除匹配的公共键后，同名协议字段不能再次消费它；
协议内部嵌套对象保留自身 JSON 匹配规则。自定义解码器必须有独立证据，不因存在底层声明就按普通结构处理。

sing-box inbound and outbound layouts bind the public envelope, protocol registry, excluded-key
helper, JSON field selection, and ordered-map removal to reviewed source behavior. Function bodies,
import identities, and build branches must match together. The JSON decoder's build condition is
separate from the protocol constructor's availability: an unconfirmed decoder branch requires
whole-object evidence, even when the constructor is registered. Public envelope keys are removed
exactly; protocol fields and ordinary nested objects follow their own matching rules. Unregistered
types and private fields do not bypass assessment. Native validation remains required.

sing-box 入站与出站的平铺结构将公共字段、协议注册表、排除键辅助函数、JSON 字段选择及有序映射删除
共同绑定到已审查的源码行为。函数体、导入身份和构建分支必须同时匹配。JSON 解码器的构建条件独立于
协议构造器是否可用：即使协议已登记，无法确认的解码分支仍需要整个对象的能力证据。公共键按精确键名
排除，协议字段和普通嵌套对象分别遵循自身匹配规则；未登记类型和内部字段不能绕过检查。候选仍须通过
当前二进制的原生校验。

Reviewed value decoders separately describe scalar shorthand and object forms. sing-box's
UDP-over-TCP entry binds boolean/null shorthand and strict object decoding to its own method,
dependency helpers, field matching, and build branches for each patch. An unknown object property
requires explicit evidence from the current binary; the underlying Go struct or a successful native
exit is insufficient. Coverage counts reviewed value decoders separately from extracted methods.

自定义值解码规则分别描述标量简写和对象形式。sing-box 的 UDP-over-TCP 入口逐补丁绑定自身方法、
依赖辅助函数、字段匹配及构建分支，区分布尔/null 简写与严格对象解析。对象内未知字段需要当前二进制的
明确证据，底层 Go 结构或原生成功退出均不足以证明支持。覆盖报告将经审查的值解码规则与已提取方法分开统计。

Compiler release conditions use numeric Go versions from the adapter's explicit build-output field.
Missing, prerelease, or annotated toolchain reports remain unconfirmed; arbitrary text and user tag
lists cannot supply compiler-release evidence. This observation does not identify the program's
stable baseline or establish official origin.

编译器 release 条件采用适配器明确构建输出字段中的 Go 版本数字。缺失、预发布或带附注的工具链信息
保留为未确认，不从任意文字或用户标签列表推断。该观察不用于识别程序稳定基线，也不构成官方来源证明。

Mihomo's flat proxy object combines the selected protocol options with embedded common fields.
The independent `smux` mapping uses its own options declaration and the exact parser key. Protocol
field comparison follows the source decoder's case and underscore rules, not the enclosing YAML
mapping rules. Entry and nested reflection decoding retain separate embedded-field and untagged-field
behavior for each patch. Internal fields tagged `-` and catch-all `remain` mappings do not declare
user options. Missing, malformed, and unregistered discriminators require evidence for the whole
object rather than bypassing the field assessment.

Mihomo 的平铺代理对象同时检查协议选项和嵌入公共字段。独立的 `smux` 映射使用自己的选项声明，
入口键严格按解析器匹配；协议字段的大小写和下划线规则来自对应源码解码器，不套用外层 YAML 规则。
各补丁分别记录入口与嵌套反射解析对嵌入结构和未标记字段的处理。标记为 `-` 的内部字段和 `remain`
剩余映射不构成用户选项声明。缺失、格式错误或未登记的类型判别字段需要整个对象的能力证据，不能
跳过字段检查。

## Admission and activation / 准入与应用

```text
Executable fingerprint → program identity → maintained stable baseline → CLI/build capabilities
  → staged registration → latest candidate merge → structural/semantic checks → native check
  → final identity/authority check → atomic apply
```

Registration and replacement reject unsupported or ambiguous binaries before committing a program.
Apply, start, autostart, restart, and executable updates use the same backend admission policy.
Changed binaries invalidate evidence. A running process is not terminated merely because its
installed executable no longer qualifies. Stop, inspection, export, and replacement remain possible.

Identity reports retain numeric version components and separate prerelease/build-metadata flags.
Arbitrary banners and qualifier text are not stored or displayed. Prereleases remain inadmissible;
build metadata does not establish official origin or additional capabilities.

Unix tool checks own their process group and output collectors until completion. Cancellation kills
the group and cancels collection; normal exit, timeout, and output limits also release descendant
pipes. A subsequent check can acquire the released slot.

登记和更换在提交前拒绝范围外或身份不明的程序。应用、启动、自动启动、重启和程序更新共用后端
准入规则。二进制变化使证据失效，不因安装文件不再合格而自动终止已运行进程；停止、查看、导出和
更换程序保持可用。

身份报告只保留版本数字以及独立的预发布、构建元数据标记，不保存或展示任意横幅和附注原文。
预发布仍拒绝准入，构建元数据不构成官方来源或额外能力证明。

Unix 检查任务在完成前持有进程组和输出读取任务；取消时结束同组进程并取消读取。正常退出、
超时和输出超限也会回收子进程持有的管道，后续检查能够重新取得执行名额。

The user has one Apply action. Background checks bind evidence to binary fingerprint, capability
profile hash, configuration hash, and candidate generation. Failed preparation retains the current
program and workspace. Failed activation retains the candidate and recovery material; rollback
failure is an explicit recovery state.

Manual and scheduled source refresh share a preparation/commit path. Downloads do not hold the
authorization gate or write candidate content. Commit acquires the configuration lease before the
authorization gate, rechecks ProgramSpec and state revision, and returns the candidate and editor
session together. An intervening edit rejects the prepared result; a fresh refresh can retry without
overwriting the edit. Source-list changes use the same final authorization check within their
workspace rollback boundary.

用户仅需一次应用动作。后台证据绑定二进制指纹、能力档案摘要、配置摘要和候选代次。准备失败保留
当前程序与工作区；应用失败保留候选和恢复材料，回滚失败进入明确的恢复状态。

手动与定时来源刷新共用准备及提交流程。下载不占用授权保护、不写入候选；提交先取得程序配置锁，再取得
授权保护，核对 ProgramSpec 与状态 revision 后一起返回候选和编辑会话。准备期间发生编辑时拒绝过期结果，
重新刷新不会覆盖新编辑。来源列表变更在工作区回滚边界内使用相同的最终授权检查。

## Presentation and acceptance / 展示与验收

Show one conclusion and one recovery action. Baselines are detected, never manually overridden.
Source identities, hashes, and build tags belong in collapsed details. Native check diagnostics contain
only a fixed result category, exit code, and captured UTF-8 text byte counts; process output is not persisted or
returned as diagnostic text. Categories summarize reported failures, not verified field capabilities.
Syntax errors retain location metadata without echoing input values. A failure with no proven document
location stays in the summary instead of marking every editor line. The candidate remains editable.
Assess only configuration-relevant failures; unused optional capabilities do not produce warnings.

默认展示一个结论和一个恢复动作，基线自动识别且不可手工覆盖。源码身份、摘要和构建标签进入
折叠详情。原生检查诊断仅保留固定结果分类、退出码和捕获文本的 UTF-8 字节数，不持久化或回传进程原文；
分类仅概括程序报告的错误，不作为字段能力证明。语法错误保留位置，不回显输入值。没有可信
文档位置的错误只显示在摘要中，不将整个编辑器标红，候选仍可编辑。
仅展示当前配置相关问题，未使用的可选能力缺失不产生警告。

Acceptance covers release boundaries, patch-level changes, custom build evidence, latest-operation
configuration merges, continuous conflicts, stale responses, binary replacement, storage recovery,
and responsive bilingual UI. Use real coordinator/storage tests as well as frontend tests. Build the
Windows authorized client after consolidated checks; native testing must not start managed programs,
enable TUN, or alter host networking.

验收覆盖版本边界、补丁差异、自编译证据、最新操作合并、持续冲突、过期响应、文件替换、存储恢复
和双语响应式界面。真实 coordinator/storage 与前端测试均需通过。集中回归后再构建 Windows 授权
客户端；真实测试不启动受管程序、不启用 TUN、不改变宿主网络。
