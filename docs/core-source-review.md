# Maintained native configuration source review

The offline catalog is rebuilt from official stable Releases and immutable Git objects. Source
declaration order, decoder dispatch, field shapes and reviewed behavior are assessed separately.
The catalog and generated coverage/change reports are authoritative for the complete maintained
patch set; this review records behavior requiring explicit attention.

离线目录从官方稳定 Release 与不可变 Git 对象生成。声明顺序、解码分派、字段形状和已审查行为
分别核对。完整补丁集合以目录及覆盖／变化报告为准；本文记录需要专门核对的行为。

| Program | Stable source | Maintained families |
| --- | --- | --- |
| sing-box | [1.14.4 / c005f353](https://github.com/SagerNet/sing-box/tree/c005f35300ce76427dc285481ec5695f5b179cd2) | 1.14.x, 1.13.x |
| Mihomo | [1.19.33 / 3f27ef39](https://github.com/MetaCubeX/mihomo/tree/3f27ef39f8c96bbfbc8b0b9bb10f5c9ef60149d8) | 1.19.x, 1.18.x |
| Xray | [26.10.21 / 1b1647cd](https://github.com/XTLS/Xray-core/tree/1b1647cd9ddf0abc34b981d4611fc03bcac1ca74) | 26.10.x, 26.3.x |

Xray families are the latest two months with actual stable Releases, not adjacent calendar months.
Every retained patch keeps its own rules; adding a release does not grant its fields to other patches.

Xray 按实际存在稳定 Release 的月份分组，不补造相邻月份。每个补丁单独绑定规则，不向其他补丁
扩散新增字段或选项。

## Reviewed behavior

- sing-box: scoped lifecycle startup preserves default outbound/DNS target checks. TUN still
  requires automatic routing for automatic redirection and rejects redirection outside Linux;
  namespace selection remains Linux-only. Dependency and source evidence remain bound separately.
  The route action decoder no longer accepts the direct action in the current patch. Decoder and
  enum changes are generated from the exact patch, rather than approximated by the family number.
- Mihomo: the TUN constructor validates `dns-mode` against empty, `disabled`, `native`, and `hijack`.
  These values are checked against `github.com/metacubex/sing-tun` v0.4.28 at commit
  `a128d0c3c8001e041725fa67ff3b521502298f17` (`tun.go`). The setting is assessed only when TUN is
  enabled. Auto-redirection platform and routing requirements remain unchanged by the newly
  forwarded DNS-mode and congestion-control settings.
- Xray: proxy settings on outbound objects and the root reverse configuration are explicitly
  rejected by their constructors in the current patch. Static assessment identifies the actual
  field, while missing/null pointer values remain distinct. A rejected field is not offered as a
  usable Intent control. The maintained patch retaining that configuration keeps its own reference
  rules. Socket mark/congestion/timeout/segment/window platform limits remain unchanged; the
  new OpenBSD inbound implementation handles original-destination reception, not these options.
  Same-package transport file moves do not imply field removal. New transport/root structures
  remain subject to native configuration checks and are not a blanket claim of runtime support.
  Outbound `streamSettings.sockopt.dialerProxy` references are checked against the outbound
  manager lookup in `transport/internet/dialer.go`; target selection, removal and undo use the
  same reference index across maintained patches.

- sing-box：作用域启动保留默认出口及 DNS 目标检查。自动重定向仍依赖自动路由，仅 Linux 可用；
  网络命名空间仍限 Linux。路由动作、字段和枚举按精确补丁生成，不按系列推测能力。
- Mihomo：TUN 启用时核对 DNS 模式，仅接受空值、disabled、native、hijack；同时核对所用依赖的
  精确源码。新增 DNS 模式和拥塞控制传递没有改变自动重定向的平台及路由要求。
- Xray：构造器明确拒绝的出口代理设置和根反向配置，在对应补丁中直接定位到字段报错；缺失
  与空指针不误报，意图不提供无法使用的控件。保留该配置的维护补丁仍使用自己的引用规则。
  套接字平台限制没有因文件移动而改变；OpenBSD 新增的原目标地址接收不是其他选项的支持证明。
  新协议和根结构继续经过当前精确二进制检查，不宣称已证明运行效果。
  出口的 dialerProxy 引用依据实际出口管理器查询实现，选择、删除与撤销共用同一引用索引。

Semantic recipes fail closed when a field shape, constructor, parser, import, or build condition
changes. An explicitly inapplicable rule still requires reviewed source evidence. Structure coverage,
reviewed semantic coverage and native acceptance remain distinct; unreviewed behavior is not promoted
to a capability claim. Browser fixtures and displayed supported releases are generated from Core.

字段形状、构造器、解析器、导入或构建条件变化均要求重新审查；明确不适用的规则同样核验源码依据。
结构覆盖、已审查语义覆盖和原生接受互相独立。未经确认的行为不升级为能力声明，浏览器测试基线
从 Core 同步生成。
