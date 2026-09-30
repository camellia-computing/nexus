export const intentGroups: Record<string, string> = {
  local: 'Local access', dns: 'DNS', routing: 'Traffic and connections',
  network: 'Traffic and connections', tun: 'Virtual network adapter', logging: 'Logging',
};

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
