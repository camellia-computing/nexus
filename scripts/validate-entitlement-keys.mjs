#!/usr/bin/env node

import { createPublicKey } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { isIP } from 'node:net';
import { resolve } from 'node:path';

const argumentsList = process.argv.slice(2);
const production = argumentsList.includes('--production');
const positional = argumentsList.filter((argument) => argument !== '--production');
if (positional.length > 1) {
  throw new Error('usage: validate-entitlement-keys.mjs [--production] [path]');
}

const path = resolve(positional[0] ?? 'src-tauri/entitlement-keys.json');
const raw = await readFile(path, 'utf8');
if (raw.includes('PRIVATE KEY')) {
  throw new Error('entitlement trust configuration must never contain private key material');
}

let document;
try {
  document = JSON.parse(raw);
} catch (error) {
  throw new Error(`entitlement trust configuration is invalid JSON: ${error.message}`);
}

function exactKeys(value, expected, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(`${label} must contain exactly: ${wanted.join(', ')}`);
  }
}

exactKeys(document, ['audience', 'issuer', 'keys', 'minimumLicenseEpoch'], 'authority');
let issuer;
try {
  issuer = new URL(document.issuer);
} catch {
  throw new Error('authority issuer must be an absolute URL');
}
if (
  issuer.protocol !== 'https:' ||
  issuer.username ||
  issuer.password ||
  issuer.search ||
  issuer.hash
) {
  throw new Error('authority issuer must be a credential-free HTTPS URL without query or fragment');
}

const host = issuer.hostname.toLowerCase();
const reservedSuffixes = ['.example', '.invalid', '.localhost', '.test'];
if (
  production &&
  (host === 'localhost' || isIP(host) !== 0 || reservedSuffixes.some((suffix) => host.endsWith(suffix)))
) {
  throw new Error('production authority issuer must use a non-reserved DNS hostname');
}
if (typeof document.audience !== 'string' || !/^[a-z0-9][a-z0-9.-]{0,126}$/.test(document.audience)) {
  throw new Error('authority audience must be a bounded lowercase identifier');
}
if (!Number.isSafeInteger(document.minimumLicenseEpoch) || document.minimumLicenseEpoch < 0) {
  throw new Error('minimumLicenseEpoch must be a non-negative safe integer');
}
if (!Array.isArray(document.keys) || document.keys.length < 1 || document.keys.length > 8) {
  throw new Error('authority must contain between one and eight public keys');
}

const keyIds = new Set();
for (const [index, key] of document.keys.entries()) {
  exactKeys(key, ['keyId', 'publicKeyPem'], `authority key ${index}`);
  if (typeof key.keyId !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(key.keyId)) {
    throw new Error(`authority key ${index} has an invalid keyId`);
  }
  if (keyIds.has(key.keyId)) {
    throw new Error(`authority keyId is duplicated: ${key.keyId}`);
  }
  keyIds.add(key.keyId);
  if (typeof key.publicKeyPem !== 'string' || key.publicKeyPem.length > 4096) {
    throw new Error(`authority key ${key.keyId} has invalid public key material`);
  }
  let parsed;
  try {
    parsed = createPublicKey(key.publicKeyPem);
  } catch {
    throw new Error(`authority key ${key.keyId} is not a valid public key`);
  }
  if (
    parsed.type !== 'public' ||
    parsed.asymmetricKeyType !== 'ec' ||
    parsed.asymmetricKeyDetails?.namedCurve !== 'prime256v1'
  ) {
    throw new Error(`authority key ${key.keyId} must be an ES256 P-256 public key`);
  }
}

process.stdout.write(
  `Validated ${production ? 'production' : 'baseline'} entitlement trust configuration (${document.keys.length} key${document.keys.length === 1 ? '' : 's'}).\n`,
);
