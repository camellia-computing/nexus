#!/usr/bin/env node

import { readdir, readFile } from 'node:fs/promises';
import { extname, join, relative, resolve } from 'node:path';
import process from 'node:process';

const root = resolve(import.meta.dirname, '..');
const scanRoots = ['crates', 'src-tauri/src', 'ui/src', '.github/workflows', 'scripts'];
const sourceExtensions = new Set([
  '.rs', '.ts', '.svelte', '.yml', '.yaml', '.json', '.mjs', '.sh', '.ps1',
]);
const forbidden = [
  ['production license bypass switch', /(?:disable[-_]?license|skip[-_]?license)/i],
  ['authentication bypass environment variable', /CAMELLIA_NEXUS_(?:NEXUS_)?SKIP_AUTH/i],
  ['permanent local paid-state flag', /\bis[_-]?pro\b/i],
  ['embedded private key', /-----BEGIN (?:EC |RSA )?PRIVATE KEY-----/],
];

async function files(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const nested = await Promise.all(entries.map(async (entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return files(path);
    return sourceExtensions.has(extname(entry.name)) ? [path] : [];
  }));
  return nested.flat();
}

const violations = [];
for (const scanRoot of scanRoots) {
  for (const file of await files(join(root, scanRoot))) {
    if (file === import.meta.filename) continue;
    const content = await readFile(file, 'utf8');
    for (const [description, pattern] of forbidden) {
      if (pattern.test(content)) {
        violations.push(`${relative(root, file)}: ${description}`);
      }
    }
  }
}

const releaseBoundaryDocuments = {
  manifest: await readFile(join(root, 'src-tauri/windows-app-manifest.xml'), 'utf8'),
  readme: await readFile(join(root, 'README.md'), 'utf8'),
  security: await readFile(join(root, 'SECURITY.md'), 'utf8'),
};
const privilegeBoundarySources = {
  broker: await readFile(join(root, 'src-tauri/src/privilege_broker.rs'), 'utf8'),
  identity: await readFile(join(root, 'src-tauri/privilege_broker_identity.rs'), 'utf8'),
  executable: await readFile(join(root, 'crates/camellia-nexus-privilege-broker/src/main.rs'), 'utf8'),
  bundle: await readFile(join(root, 'src-tauri/tauri.privilege-broker.conf.json'), 'utf8'),
  windowsStage: await readFile(join(root, 'scripts/stage-windows-release.ps1'), 'utf8'),
};
const requiredReleaseBoundaryText = [
  ['Windows manifest uses the caller token', releaseBoundaryDocuments.manifest, /requestedExecutionLevel\s+level="asInvoker"/],
  ['Chinese README documents the normal-user boundary', releaseBoundaryDocuments.readme, /普通用户令牌/],
  ['English README documents the normal-user boundary', releaseBoundaryDocuments.readme, /normal user token/],
  ['security policy documents the normal-token boundary', releaseBoundaryDocuments.security, /normal token/],
];
for (const [description, content, pattern] of requiredReleaseBoundaryText) {
  if (!pattern.test(content)) violations.push(`release boundary: missing ${description}`);
}
const requiredPrivilegeBoundaryText = [
  ['desktop locates only the dedicated broker', privilegeBoundarySources.broker, /camellia-nexus-privilege-broker/],
  ['broker identity is pinned independently from native signing', privilegeBoundarySources.broker, /CAMELLIA_NEXUS_PRIVILEGE_BROKER_SHA256/],
  ['native signature blobs are excluded from broker identity', privilegeBoundarySources.identity, /normalize_pe_authenticode[\s\S]*normalize_macho_code_signature/],
  ['Unix broker cannot be group or world writable', privilegeBoundarySources.broker, /permissions\(\)\.mode\(\) & 0o022/],
  ['background startup fails closed without an existing session', privilegeBoundarySources.broker, /!plan\.interactive && !has_active_session\(\)/],
  ['one broker session multiplexes privileged programs with bounded senders', privilegeBoundarySources.broker, /HashMap<ProgramId,\s*mpsc::Sender<PrivilegeBrokerEvent>>/],
  ['broker process event queues have capacity two', privilegeBoundarySources.broker, /const BROKER_EVENT_QUEUE_CAPACITY:\s*usize\s*=\s*2;/],
  ['broker registrations use the bounded event queue capacity', privilegeBoundarySources.broker, /mpsc::channel\(BROKER_EVENT_QUEUE_CAPACITY\)/],
  ['broker event dispatch detects a full queue without waiting', privilegeBoundarySources.broker, /sender\.try_send\(event\)[\s\S]*TrySendError::Full\(_\)[\s\S]*overflowed a process lifecycle event queue/],
  ['broker event overflow fails the session closed', privilegeBoundarySources.broker, /if let Err\(error\) = broker\.dispatch\([\s\S]*broker\.fail_all\(error\);\s*return;/],
  ['broker command ingress has a bounded queue', privilegeBoundarySources.executable, /const BROKER_COMMAND_QUEUE_CAPACITY:\s*usize\s*=\s*64;[\s\S]*mpsc::sync_channel\(BROKER_COMMAND_QUEUE_CAPACITY\)/],
  ['broker command overflow fails the session before backlog execution', privilegeBoundarySources.executable, /TrySendError::Full\(_\)[\s\S]*store\(READER_OVERFLOWED, Ordering::Release\)[\s\S]*receive_broker_command\(&command_rx, &reader_state\)/],
  ['broker stop grace is shared across privileged children', privilegeBoundarySources.executable, /let mut stopping = HashMap::<ProgramId, PendingStop>::new\(\);[\s\S]*begin_managed_stop\(child, request_id\.clone\(\)\)[\s\S]*stopping\.insert\(program_id, pending\)/],
  ['Windows child uses suspended creation', privilegeBoundarySources.executable, /SUSPENDED/],
  ['Windows child enters its Job before resume', privilegeBoundarySources.executable, /WindowsJob::attach\(&child\)[\s\S]*resume_managed\(&child\)/],
  ['Windows broker handle terminates on abandonment', privilegeBoundarySources.broker, /TerminateProcess\(self\.0, 1\)/],
  ['broker disconnect terminates the managed tree', privilegeBoundarySources.executable, /impl Drop for ManagedChildren[\s\S]*terminate_managed\(child\)/],
  ['broker is bundled only as a sidecar', privilegeBoundarySources.bundle, /"externalBin"\s*:\s*\["binaries\/camellia-nexus-privilege-broker"\]/],
  ['Windows portable package contains the application/broker pair', privilegeBoundarySources.windowsStage, /camellia-nexus\.exe[\s\S]*camellia-nexus-privilege-broker\.exe[\s\S]*Compress-Archive/],
  ['Chinese README documents automatic privilege detection', releaseBoundaryDocuments.readme, /自动检测 \+ 启动时授权/],
  ['English README documents automatic privilege detection', releaseBoundaryDocuments.readme, /automatic detection with authorization at start/],
];
for (const [description, content, pattern] of requiredPrivilegeBoundaryText) {
  if (!pattern.test(content)) violations.push(`privilege boundary: missing ${description}`);
}
const obsoleteReleaseBoundaryText = [
  'Windows 版本在初始化核心组件前请求管理员权限',
  'Windows processes run with administrative privileges',
];
for (const text of obsoleteReleaseBoundaryText) {
  if (releaseBoundaryDocuments.readme.includes(text) || releaseBoundaryDocuments.security.includes(text)) {
    violations.push(`release boundary: obsolete privilege claim: ${text}`);
  }
}

const packageWorkflow = await readFile(
  join(root, '.github/workflows/client-packages.yml'),
  'utf8',
);
const windowsLocalCi = await readFile(join(root, 'scripts/ci-local.ps1'), 'utf8');
const publishWorkflow = await readFile(
  join(root, '.github/workflows/publish-release.yml'),
  'utf8',
);
if (
  !/if \(\$SigningContext\) \{\s*Complete-WindowsExecutableSignature\s*`\s*-File \$Executable/.test(windowsLocalCi)
) {
  violations.push(
    'Windows signing: the standalone executable is not completed at its final byte boundary',
  );
}
if (
  !/function Complete-WindowsExecutableSignature[\s\S]*Get-WindowsEmbeddedSignature[\s\S]*NoSignature[\s\S]*Invoke-WindowsSignature/.test(windowsLocalCi)
) {
  violations.push(
    'Windows signing: final executable completion does not reject duplicate or invalid signatures',
  );
}
if (
  !/Get-WindowsPfxVerificationContext[\s\S]*Assert-WindowsSignature[\s\S]*ExpectedThumbprint \$verificationContext\.Thumbprint/.test(
    privilegeBoundarySources.windowsStage,
  )
) {
  violations.push(
    'Windows signing: release staging is not bound to the exact PFX signer and isolated trust chain',
  );
}
if (
  !/- name: Stage Windows[\s\S]*?env:[\s\S]*?CAMELLIA_NEXUS_SIGN_PASSWORD:[\s\S]*?shell: pwsh/.test(
    packageWorkflow,
  )
) {
  violations.push(
    'Windows signing: the staging step cannot reopen the ephemeral PFX for identity verification',
  );
}
const requiredRecoveryControlText = [
  [
    'manual publication passes the verified workflow commit to packaging',
    publishWorkflow,
    'control-commit: ${{ needs.metadata.outputs.control-sha }}',
  ],
  [
    'recovery control is restricted to a mismatched Windows package commit',
    packageWorkflow,
    "if: ${{ runner.os == 'Windows' && inputs.control-commit != '' && inputs.control-commit != inputs.commit }}",
  ],
  [
    'recovery uses an isolated checkout for the trusted full commit',
    packageWorkflow,
    'ref: ${{ inputs.control-commit }}',
  ],
  [
    'recovery uses a separate checkout path',
    packageWorkflow,
    'path: .release-control',
  ],
  [
    'recovery preserves the package commit identity',
    packageWorkflow,
    'Recovery control changed the package commit identity',
  ],
];
for (const [description, content, required] of requiredRecoveryControlText) {
  if (!content.includes(required)) violations.push(`release recovery: missing ${description}`);
}

const recoveryPaths = packageWorkflow.match(/control_paths=\(\n([\s\S]*?)\n\s*\)/);
const expectedRecoveryPaths = [
  'scripts/ci-local.ps1',
  'scripts/prepare-privilege-broker.mjs',
  'scripts/test-windows-authenticode.ps1',
  'scripts/windows-authenticode.ps1',
];
const actualRecoveryPaths = recoveryPaths
  ? recoveryPaths[1].trim().split(/\s+/)
  : [];
if (JSON.stringify(actualRecoveryPaths) !== JSON.stringify(expectedRecoveryPaths)) {
  violations.push('release recovery: Windows control allowlist is not the exact reviewed set');
}
const recoveryCheckout = packageWorkflow.match(
  /- name: Checkout trusted Windows recovery control\n([\s\S]*?)\n\s*- name: Install trusted Windows recovery control/,
);
if (
  !recoveryCheckout ||
  !recoveryCheckout[1].includes('persist-credentials: false') ||
  !recoveryCheckout[1].includes('ref: ${{ inputs.control-commit }}') ||
  !recoveryCheckout[1].includes('path: .release-control') ||
  !recoveryCheckout[1].includes('sparse-checkout-cone-mode: false')
) {
  violations.push('release recovery: isolated control checkout boundary is incomplete');
}
const recoverySparsePaths = recoveryCheckout?.[1].match(
  /sparse-checkout: \|\n([\s\S]*?)\n\s*sparse-checkout-cone-mode:/,
);
const actualRecoverySparsePaths = recoverySparsePaths
  ? recoverySparsePaths[1].trim().split(/\s+/)
  : [];
if (JSON.stringify(actualRecoverySparsePaths) !== JSON.stringify(expectedRecoveryPaths)) {
  violations.push('release recovery: sparse checkout is not the exact reviewed set');
}
if (/git\s+(?:checkout|reset|switch)[^\n]*\$CONTROL_COMMIT/.test(packageWorkflow)) {
  violations.push('release recovery: control commit can replace the package checkout');
}
if (/git\s+fetch[^\n]*\$CONTROL_COMMIT/.test(packageWorkflow)) {
  violations.push('release recovery: control commit bypasses the isolated checkout');
}

if (violations.length) {
  console.error(`Release security audit failed:\n${violations.join('\n')}`);
  process.exit(1);
}

console.log('Release security audit passed.');
