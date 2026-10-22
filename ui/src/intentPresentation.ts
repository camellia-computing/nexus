import type { IntentNode } from './types';

export type IntentSettingChange = { kind: 'set'; value: unknown } | { kind: 'follow' };

const nativeTitles: Record<string, string> = {
  '$schema': 'Configuration schema', log: 'Logging', 'log-level': 'Logging',
  dns: 'DNS', ntp: 'Time synchronization', certificate: 'Certificates',
  domain_resolver: 'DNS used for address lookup', server: 'DNS server', inbound: 'Incoming connections', inboundTag: 'Incoming connections',
  certificate_providers: 'Certificate providers', http_clients: 'HTTP clients',
  network_namespaces: 'Network namespaces', endpoints: 'Endpoints',
  inbounds: 'Incoming connections', outbounds: 'Outgoing connections',
  route: 'Routing', routing: 'Routing', services: 'Services', experimental: 'Advanced features',
  transport: 'Transport', policy: 'Connection policies', api: 'Program API', metrics: 'Metrics',
  stats: 'Statistics', reverse: 'Reverse connections', fakeDns: 'Synthetic DNS',
  observatory: 'Connection monitoring', burstObservatory: 'Connection monitoring',
  port: 'HTTP proxy', 'socks-port': 'SOCKS proxy', 'mixed-port': 'HTTP and SOCKS proxy',
  'proxy-providers': 'Connection providers', 'rule-providers': 'Rule providers',
  proxies: 'Outgoing connections', 'proxy-groups': 'Connection groups',
  rules: 'Routing rules', 'sub-rules': 'Rule groups', listeners: 'Incoming connections',
  hosts: 'Host overrides', tun: 'Virtual network adapter', 'tuic-server': 'TUIC server',
  iptables: 'Firewall integration', profile: 'Program preferences', 'geox-url': 'Geodata sources',
  sniffer: 'Traffic detection', tls: 'TLS', version: 'Configuration version',
  auto_route: 'Automatic routing', 'auto-route': 'Automatic routing',
  auto_redirect: 'Automatic traffic redirection', 'auto-redirect': 'Automatic traffic redirection',
  password: 'Password', private_key: 'Private key', 'private-key': 'Private key',
  uuid: 'User identifier', secret: 'Access secret',
};

export function intentNodeLabel(node: IntentNode): string {
  return nativeTitles[node.nativeName] ?? node.nativeName;
}

export function intentEffectLabel(path: string): string {
  const name = path.split('/').at(-1)?.replace(/~1/g, '/').replace(/~0/g, '~') ?? '';
  return nativeTitles[name] ?? name;
}

export function intentNodeQualifier(node: IntentNode): string {
  return node.typeSelections.map((item) => item.value).filter((value, index, values) => values.indexOf(value) === index).join(' · ');
}

export function intentContextSummary(value: { present: boolean; protected: boolean; value?: unknown; summary?: { kind: string; size: number } }, localize: (message: string) => string): string {
  if (value.protected) return localize(value.present ? 'Already set' : 'Not set');
  if (!value.present) return localize('Use program default');
  if (value.summary) return `${value.summary.size} ${localize(value.summary.kind === 'list' ? 'entries' : value.summary.kind === 'object' ? 'fields' : 'characters')}`;
  if (value.value === null) return 'null';
  if (typeof value.value === 'boolean') return localize(value.value ? 'On' : 'Off');
  return String(value.value ?? '');
}

export function dnsServerNeedsResolver(server: unknown): boolean {
  if (typeof server !== 'string' || !server.trim()) return false;
  let host = server.trim();
  if (host.includes('://')) {
    try { host = new URL(host).hostname; }
    catch { return false; }
  }
  host = host.replace(/^\[|\]$/g, '');
  const ipLiteral = /^(?:\d{1,3}\.){3}\d{1,3}$/.test(host) || (host.includes(':') && /^[\da-f:.]+$/i.test(host));
  return !ipLiteral;
}

export const intentSettingOptionMessages: Record<string, Record<string, string>> = {
    'logging.level': { trace: 'Detailed tracing', debug: 'Debugging', info: 'Normal logging', warn: 'Warnings and errors', warning: 'Warnings and errors', error: 'Errors only', fatal: 'Fatal errors only', panic: 'Critical failures only', none: 'No logging', silent: 'No logging' },
    'logging.maskAddress': { '': 'Do not hide addresses', quarter: 'Partly hide addresses', half: 'Hide more of each address', full: 'Hide the whole address' },
    'dns.strategy': { prefer_ipv4: 'Prefer IPv4', prefer_ipv6: 'Prefer IPv6', ipv4_only: 'IPv4 only', ipv6_only: 'IPv6 only' },
    'dns.queryStrategy': { UseIP: 'IPv4 and IPv6', UseIPv4: 'IPv4 only', UseIPv6: 'IPv6 only' },
    'routing.domainStrategy': { AsIs: 'Match domain names', IPIfNonMatch: 'Try IP rules if no domain matches', IPOnDemand: 'Resolve addresses when a rule needs them' },
    'dns.mode': { normal: 'Normal lookup', 'fake-ip': 'Placeholder IP addresses', 'redir-host': 'Real IP addresses' },
    'routing.mode': { rule: 'Use routing rules', global: 'One connection for all traffic', direct: 'Direct connections' },
};
const stackMessages: Record<string, string> = { system: 'System network stack', gvisor: 'Userspace network stack', mixed: 'Combined network stacks' };
const optionMessages: Record<string, string> = {
    domain: 'Exact domain', subdomain: 'Domain and subdomains', ip: 'IP address or network',
    local: 'This computer only', lan: 'Devices on my network', system: 'System default',
    udp: 'Standard DNS', tls: 'Encrypted DNS (TLS)', https: 'Encrypted DNS (HTTPS)',
    mixed: 'HTTP and SOCKS', http: 'HTTP', socks: 'SOCKS',
};

export function intentOptionMessage(value: string, settingId = ''): string {
  if (intentSettingOptionMessages[settingId]?.[value]) return intentSettingOptionMessages[settingId][value];
  if (settingId === 'stack' || settingId.endsWith('.stack')) return stackMessages[value] ?? value;
  return optionMessages[value] ?? value;
}
