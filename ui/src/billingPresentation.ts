const DECIMAL_PATTERN = /^(-?)(\d+)(?:\.(\d+))?$/;

export function formatBillingAmount(value: string) {
  const match = DECIMAL_PATTERN.exec(value);
  if (!match) return value;
  const [, sign, integer, fraction = ''] = match;
  if (!fraction) return `${sign}${integer}`;
  const significant = fraction.replace(/0+$/, '');
  const displayed = significant.padEnd(2, '0');
  return displayed ? `${sign}${integer}.${displayed}` : `${sign}${integer}`;
}

export function compactPaymentReference(value: string) {
  if (value.length <= 24) return value;
  return `${value.slice(0, 11)}…${value.slice(-8)}`;
}
