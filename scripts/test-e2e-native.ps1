#requires -Version 7.6

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = Split-Path -Parent $PSScriptRoot

function Assert-Contract {
  param(
    [Parameter(Mandatory)] [bool]$Condition,
    [Parameter(Mandatory)] [string]$Message
  )
  if (-not $Condition) { throw "Native E2E contract: $Message" }
}

$harnessPath = Join-Path $PSScriptRoot 'e2e-native.ps1'
$tokens = $null
$parseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
  $harnessPath,
  [ref]$tokens,
  [ref]$parseErrors
) | Out-Null
$parseMessage = if ($parseErrors.Count -gt 0) {
  ($parseErrors | ForEach-Object { $_.Message }) -join '; '
} else {
  'PowerShell parsing failed'
}
Assert-Contract ($parseErrors.Count -eq 0) $parseMessage

$productionConfig = Get-Content -Raw (Join-Path $repositoryRoot 'src-tauri/tauri.conf.json') |
  ConvertFrom-Json
$e2eConfig = Get-Content -Raw (Join-Path $repositoryRoot 'src-tauri/tauri.e2e.conf.json') |
  ConvertFrom-Json
$productionCapabilities = @($productionConfig.app.security.capabilities)
$e2eCapabilities = @($e2eConfig.app.security.capabilities)
Assert-Contract ($productionCapabilities -notcontains 'e2e') 'production configuration must exclude the WebDriver capability'
Assert-Contract (
  $e2eCapabilities.Count -eq 1 -and
  $e2eCapabilities[0].identifier -eq 'e2e' -and
  @($e2eCapabilities[0].permissions) -contains 'wdio:default' -and
  @($e2eCapabilities[0].permissions) -contains 'wdio-webdriver:default'
) 'the E2E configuration must inline only its isolated WebDriver capability'
Assert-Contract (
  $e2eConfig.identifier -ne $productionConfig.identifier
) 'the E2E application must use a distinct platform identity'
Assert-Contract (
  -not (Test-Path -LiteralPath (Join-Path $repositoryRoot 'src-tauri/capabilities/e2e.json'))
) 'plugin permissions must not live in the production capability discovery directory'

$windowsApplicationManifest = Get-Content -Raw (
  Join-Path $repositoryRoot 'src-tauri/windows-app-manifest.xml'
)
$privilegeBrokerBundleConfig = Get-Content -Raw (
  Join-Path $repositoryRoot 'src-tauri/tauri.privilege-broker.conf.json'
)
$privilegeBroker = Get-Content -Raw (
  Join-Path $repositoryRoot 'src-tauri/src/privilege_broker.rs'
)
$privilegeBrokerIdentity = Get-Content -Raw (
  Join-Path $repositoryRoot 'src-tauri/privilege_broker_identity.rs'
)
$privilegeBrokerExecutable = Get-Content -Raw (
  Join-Path $repositoryRoot 'crates/camellia-nexus-privilege-broker/src/main.rs'
)
$jobAssignment = $privilegeBrokerExecutable.IndexOf('WindowsJob::attach(&child)')
$processResume = $privilegeBrokerExecutable.IndexOf('resume_managed(&child)')
Assert-Contract (
  $windowsApplicationManifest -match 'requestedExecutionLevel\s+level="asInvoker"' -and
  $privilegeBrokerBundleConfig -match 'camellia-nexus-privilege-broker' -and
  $privilegeBroker -match 'ShellExecuteExW' -and
  $privilegeBroker -match 'CAMELLIA_NEXUS_PRIVILEGE_BROKER_SHA256' -and
  $privilegeBroker -match 'BROKER_SESSION' -and
  $privilegeBrokerIdentity -match 'normalize_pe_authenticode' -and
  $privilegeBrokerExecutable -match 'CREATE_SUSPENDED' -and
  $privilegeBrokerExecutable -notmatch 'CREATE_BREAKAWAY_FROM_JOB' -and
  $jobAssignment -ge 0 -and
  $processResume -gt $jobAssignment
) 'Windows elevation must remain isolated to a build-pinned session sidecar and a nested-Job-compatible suspended child'

$manifest = Get-Content -Raw (Join-Path $repositoryRoot 'src-tauri/Cargo.toml')
$composition = Get-Content -Raw (Join-Path $repositoryRoot 'src-tauri/src/lib.rs')
$harness = Get-Content -Raw $harnessPath
Assert-Contract (
  $harness -match '\[System\.Security\.Cryptography\.ECCurve\+NamedCurves\]::nistP256' -and
  $harness -notmatch '\[System\.Security\.Cryptography\.ECCurve\]::NamedCurves\.'
) 'the entitlement fixture must select P-256 through the PowerShell nested-type syntax'
Assert-Contract (
  $harness.Contains('$separator = $Path.LastIndexOf(''/'')') -and
  -not $harness.Contains('Split-Path -Parent $Path')
) 'WSL file parents must be resolved as POSIX paths independently of the host'
Assert-Contract (
  $harness -notmatch '\$privatePath\s*=' -and
  $harness -notmatch 'WriteAllText\(\$privatePath'
) 'ephemeral entitlement private keys must not enter the diagnostics directory'
Assert-Contract (
  $harness.Contains("Invoke-CapturedProcess wsl.exe @('--version')") -and
  $harness.Contains('$manifest.schemaVersion -ne 2') -and
  $harness.Contains('$manifest.runtime.wslVersion -ne 2') -and
  $harness.Contains('$manifest.runtime.architecture -ne ''x86_64''') -and
  $harness.Contains('$manifest.runtime.networking -ne ''shared-loopback''')
) 'the bundle contract must require modern x86_64 WSL2 with shared loopback networking'
Assert-Contract (
  ([regex]::Matches($harness, "'--version', '2'")).Count -eq 2 -and
  $harness -notmatch "'--version', '1'" -and
  $harness -match 'function\s+Assert-Wsl2Distribution\b'
) 'both bundle root filesystems must be imported and verified as WSL2 without a WSL1 path'
Assert-Contract (
  $harness.Contains('Wait-TcpPort $postgresPort -Process $postgresProcess') -and
  $harness.Contains('Wait-HttpReady "$issuer/readyz" -Process $serverProcess') -and
  $harness -match 'function\s+Assert-BackgroundProcess\b' -and
  $harness -match 'is still running, but TCP port'
) 'bundle readiness must distinguish process exit from a live process timeout'
$diagnosticsStart = $harness.IndexOf('function Write-WslDistributionDiagnostics')
$diagnosticsEnd = $harness.IndexOf('function Stop-WslBundleProcesses', $diagnosticsStart)
Assert-Contract (
  $diagnosticsStart -ge 0 -and $diagnosticsEnd -gt $diagnosticsStart
) 'bounded WSL diagnostics functions must be present'
$diagnostics = $harness.Substring($diagnosticsStart, $diagnosticsEnd - $diagnosticsStart)
Assert-Contract (
  $diagnostics.Contains('/proc/[0-9]*/status') -and
  $diagnostics.Contains('/etc/os-release') -and
  $diagnostics.Contains('/var/lib/postgresql/data') -and
  $diagnostics.Contains("-join '; '") -and
  $diagnostics -notmatch "@'" -and
  $diagnostics -notmatch '/proc/[^\s''"]*/(?:cmdline|environ)'
) 'WSL diagnostics must use CRLF-safe commands without process arguments or environment'
Assert-Contract (
  $harness -match "\.Replace\('e2e-only-password', '\[REDACTED\]'\)" -and
  $harness -match '\[int\]\$MaximumCharacters\s*=\s*8000' -and
  $harness -match '\$process\.WaitForExit\(5000\)' -and
  $harness -match '\[AllowNull\(\)\]\s*\[object\[\]\]\$Processes' -and
  $harness -match '\$failure\s*=\s*\$_' -and
  $harness -match 'throw\s+\$failure'
) 'background failures must remain primary while cleanup redacts, bounds, and flushes diagnostics'
foreach ($scenario in @(
    'free-primary',
    'free-second-device',
    'pro-primary',
    'pro-recovery',
    'team-owner',
    'team-member',
    'team-additional-device'
  )) {
  Assert-Contract (
    $harness.Contains("'--operation-id', `"native-e2e-`$suffix-$scenario-code`"")
  ) "the WSL bundle $scenario activation-code issue must use a stable operation identifier"
}
Assert-Contract (
  $harness.Contains('$descriptor.schemaVersion -ne 3') -and
  $harness.Contains('$free.activationCodes.primary') -and
  $harness.Contains('$free.activationCodes.secondDevice') -and
  $harness.Contains('$pro.activationCodes.primary') -and
  $harness.Contains('$pro.activationCodes.recovery') -and
  $harness.Contains('$pro.billing.offerId') -and
  $harness.Contains('$pro.billing.paymentMethodId') -and
  $harness.Contains('$pro.billing.invoiceId') -and
  $harness.Contains('$team.activationCodes.owner') -and
  $harness.Contains('$team.activationCodes.member') -and
  $harness.Contains('$team.activationCodes.additionalDevice')
) 'owned providers must require the complete schema-3 multi-plan, multi-identity, and billing fixture set'
Assert-Contract (
  $harness.Contains("ConvertTo-WslPath (Join-Path `$RunDirectory 'compose-state')") -and
  $harness.Contains('$environment.ProviderStateRoot = $wslStateRoot') -and
  $harness.Contains('--host-port $hostPort --bind 0.0.0.0') -and
  ([regex]::Matches($harness, 'CAMELLIA_NEXUS_E2E_STATE_ROOT=')).Count -ge 3
) 'the local WSL2 provider must keep disposable Compose state and a stable port across all controls'
Assert-Contract (
  $harness -match 'function\s+Set-E2eServerAvailability\b' -and
  $harness -match 'function\s+Set-E2eAccountState\b' -and
  $harness.Contains('kill -$signal') -and
  $harness.Contains('/run/camellia-e2e/server.pid') -and
  $harness.Contains("'account-state'") -and
  $harness.Contains("'pause'") -and
  $harness.Contains("'resume'")
) 'owned providers must expose bounded availability and account-state controls'
Assert-Contract (
  $harness.Contains("if (`$Suite -ne 'smoke')") -and
  $harness.Contains('The Existing provider supports smoke only')
) 'an externally owned provider must not run destructive full scenarios'

$nativeConfig = Get-Content -Raw (Join-Path $repositoryRoot 'ui/wdio.native.conf.mjs')
$handoff = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/handoff.mjs')
$app = Get-Content -Raw (Join-Path $repositoryRoot 'ui/src/App.svelte')
$licenseSettings = Get-Content -Raw (Join-Path $repositoryRoot 'ui/src/LicenseSettingsPanel.svelte')
$nativeSupport = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/support.mjs')
$restorationScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/06-full-restoration.e2e.mjs')
$terminalDenialScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/05-full-terminal-denial.e2e.mjs')
$freeScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/00-free-activation-limits.e2e.mjs')
$freeLimitScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/07-free-device-limit.e2e.mjs')
$freeReleaseScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/08-free-primary-release.e2e.mjs')
$freeRecoveryScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/09-free-device-recovery.e2e.mjs')
$memberScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/11-team-member-join.e2e.mjs')
$additionalDeviceScenario = Get-Content -Raw (Join-Path $repositoryRoot 'ui/tests/native/13-team-additional-device.e2e.mjs')
$phases = @(
  'free-activation-limits',
  'free-device-limit',
  'free-primary-release',
  'free-device-recovery',
  'smoke-activation',
  'smoke-persistence',
  'full-offline',
  'full-recovery-billing',
  'full-terminal-denial',
  'full-restoration',
  'team-owner-activation',
  'team-member-join',
  'team-owner-workspace',
  'team-additional-device',
  'team-former-owner-leave',
  'team-new-owner',
  'cleanup'
)
foreach ($phase in $phases) {
  Assert-Contract (
    $nativeConfig.Contains("'$phase':") -or $nativeConfig.Contains("$phase`:")
  ) "the Webdriver configuration must map the $phase phase"
  Assert-Contract (
    $harness.Contains("Invoke-NativePhase '$phase'") -or $phase -eq 'cleanup'
  ) "the native harness must orchestrate the $phase phase"
}
foreach ($spec in @(
    '00-free-activation-limits.e2e.mjs',
    '01-smoke-activation.e2e.mjs',
    '02-smoke-persistence.e2e.mjs',
    '03-full-offline.e2e.mjs',
    '04-full-recovery-billing.e2e.mjs',
    '05-full-terminal-denial.e2e.mjs',
    '06-full-restoration.e2e.mjs',
    '07-free-device-limit.e2e.mjs',
    '08-free-primary-release.e2e.mjs',
    '09-free-device-recovery.e2e.mjs',
    '10-team-owner-activation.e2e.mjs',
    '11-team-member-join.e2e.mjs',
    '12-team-owner-workspace.e2e.mjs',
    '13-team-additional-device.e2e.mjs',
    '14-team-former-owner-leave.e2e.mjs',
    '15-team-new-owner.e2e.mjs',
    '99-cleanup.e2e.mjs'
  )) {
  Assert-Contract (
    Test-Path -LiteralPath (Join-Path $repositoryRoot "ui/tests/native/$spec") -PathType Leaf
  ) "the native phase spec $spec is missing"
}
Assert-Contract (
  ([regex]::Matches($harness, "pnpm @\('desktop:build:e2e'\)")).Count -eq 1 -and
  $harness.IndexOf("pnpm @('desktop:build:e2e')") -lt
    $harness.IndexOf("Invoke-NativePhase 'smoke-activation'")
) 'the native application must be built once before all serialized phases'
Assert-Contract (
  $harness.Contains('CAMELLIA_NEXUS_E2E_HANDOFF_DIR = $handoffDirectory') -and
  $harness.Contains('SetAccessRuleProtection($true, $false)') -and
  $harness.Contains('Set-Acl -LiteralPath $handoffDirectory') -and
  $harness.Contains('Remove-Item -LiteralPath $handoffDirectory -Recurse -Force') -and
  $handoff.Contains("flag: 'wx'") -and
  $handoff.Contains('if (consume) await unlink(target)') -and
  $handoff.Contains('\.(?:json|token)')
) 'cross-identity secrets must use a private, exclusive, one-time handoff directory'
Assert-Contract (
  $harness.Contains("-ReadySignal 'terminal-denial-ready.json'") -and
  $harness.Contains("-AppliedSignal 'terminal-denial-applied.json'") -and
  $harness.Contains('[System.IO.File]::Move($temporaryPath, $Path)') -and
  $harness.Contains('Publish-E2eHandoff $appliedPath') -and
  $harness.Contains("'native-phase-process.log'") -and
  $harness.Contains('-CaptureOutput') -and
  $harness.Contains('WaitForExit(360000)') -and
  $handoff.Contains('export async function waitForHandoff') -and
  $handoff.Contains("['EACCES', 'EBUSY', 'ENOENT', 'EPERM']") -and
  $terminalDenialScenario.Contains("writeHandoff('terminal-denial-ready.json'") -and
  $terminalDenialScenario.Contains("waitForHandoff('terminal-denial-applied.json', true)") -and
  $harness.IndexOf("Invoke-NativePhase 'full-terminal-denial'") -lt
    $harness.IndexOf('Set-E2eAccountState $environment $environment.ProAccountId suspended')
) 'terminal denial must coordinate atomically after the process starts and preserve worker diagnostics'
Assert-Contract (
  $nativeConfig.Contains("'.team-secret code'") -and
  $nativeConfig.Contains("'.webhook-secret code'") -and
  $nativeConfig.Contains("'[data-e2e-sensitive]'") -and
  $nativeConfig.Contains("input.value = '[REDACTED]'")
) 'failure screenshots must redact Team, enrollment, and webhook secrets'
Assert-Contract (
  $app.Contains("api.beginLicenseAuthorization(import.meta.env.MODE !== 'e2e')") -and
  $licenseSettings.Contains('data-e2e-authorization-url={import.meta.env.MODE === ''e2e''') -and
  $nativeSupport.Contains("getAttribute('data-e2e-authorization-url')") -and
  $nativeSupport.Contains('export async function completeUiAuthorization(activationCode)') -and
  $nativeSupport -notmatch '__TAURI_INTERNALS__|__CAMELLIA_NEXUS_E2E_AUTHORIZATION_REQUESTS__|installAuthorizationInterceptor|requestIndex'
) 'native activation must use the real application-domain request without opening a browser or patching IPC'
Assert-Contract (
  $freeScenario.Contains("requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_PRIMARY_CODE')") -and
  $freeScenario.Contains("expect(claims.plan).toBe('free')") -and
  $freeScenario.Contains('max_programs: 5') -and
  $freeScenario.Contains('max_config_sources_per_program: 0') -and
  $freeScenario.Contains('async function expectCommandRejected(command, args)') -and
  ([regex]::Matches($freeScenario, 'await expectCommandRejected\(')).Count -eq 6 -and
  $freeScenario.Contains("expect(await invoke('list_programs')).toHaveLength(5)") -and
  $freeScenario.Contains("await invoke('get_program', { programId: programIds[0] })") -and
  $freeLimitScenario.Contains("requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_SECOND_DEVICE_CODE')") -and
  $freeLimitScenario.Contains("waitForText('.license-panel', 'License limit reached')") -and
  $freeReleaseScenario.Contains("clickButtonInTextContainer('.license-device-list article', 'This device', 'Remove')") -and
  $freeRecoveryScenario.Contains("requiredEnvironment('CAMELLIA_NEXUS_E2E_FREE_SECOND_DEVICE_CODE')") -and
  $freeRecoveryScenario.Contains("expect(snapshot.entitlementState.entitlement.claims.plan).toBe('free')")
) 'the full native suite must exercise Free activation, lifecycle, quotas, capability denials, device release, and retry recovery'
Assert-Contract (
  $nativeConfig.Contains("import { randomUUID } from 'node:crypto'") -and
  $nativeConfig -match "core\.invoke\('reset_license_device_identity',\s*\{\s*operationId:\s*resetOperationId\s*\}\)" -and
  $harness -match 'function\s+Remove-E2eCredentialNamespace\b' -and
  $harness.Contains('com.camellia.nexus.licensing.E2E.$Namespace/') -and
  $harness.Contains('Native E2E credentials remain in isolated namespace') -and
  $harness.Contains("Invoke-NativePhase 'cleanup' `$identity `$dataRoot `$runDirectory `$runId -ResetIdentity") -and
  $harness.Contains('$cleanupFailures = [System.Collections.Generic.List[string]]::new()') -and
  -not ($harness -match '(?s)if\s*\(\s*-not\s+\$nativeSucceeded\s*\).*?Invoke-NativePhase\s+''cleanup''')
) 'every successful or failed native run must reset with a UUID operation and verify its exact Credential Manager namespace is empty'
Assert-Contract (
  $nativeConfig -match 'timeout:\s*300_000' -and
  $restorationScenario.Contains("requiredEnvironment('CAMELLIA_NEXUS_E2E_PRO_RECOVERY_CODE')") -and
  $restorationScenario.Contains("removed.entitlementState.status).toBe('unauthenticated')") -and
  $restorationScenario.Contains('reactivated.entitlementState.entitlement.claims.deviceId).toBe(expected.deviceId)') -and
  $memberScenario.Contains("invoke('accept_license_team_invitation'") -and
  $additionalDeviceScenario.Contains("'.team-accept:not(.team-device-accept)")
) 'long real-service phases must cover removed-device recovery and idempotent Team token routing'
Assert-Contract (
  $memberScenario.Contains("import { randomUUID } from 'node:crypto'") -and
  $memberScenario -match 'request:\s*\{\s*invitationToken,\s*operationId:\s*randomUUID\(\)\s*\}' -and
  $additionalDeviceScenario.Contains("import { randomUUID } from 'node:crypto'") -and
  $additionalDeviceScenario -match 'request:\s*\{\s*enrollmentToken,\s*operationId:\s*randomUUID\(\)\s*\}'
) 'direct native Team replay probes must send canonical random operation identities'
$algorithm = $null
$signer = $null
$verifier = $null
try {
  $algorithm = [System.Security.Cryptography.ECDsa]::Create(
    [System.Security.Cryptography.ECCurve+NamedCurves]::nistP256
  )
  $privatePem = $algorithm.ExportPkcs8PrivateKeyPem()
  $publicPem = $algorithm.ExportSubjectPublicKeyInfoPem()
  $signer = [System.Security.Cryptography.ECDsa]::Create()
  $verifier = [System.Security.Cryptography.ECDsa]::Create()
  $signer.ImportFromPem($privatePem)
  $verifier.ImportFromPem($publicPem)
  $payload = [System.Text.Encoding]::UTF8.GetBytes('native-e2e-key-contract')
  $signature = $signer.SignData(
    $payload,
    [System.Security.Cryptography.HashAlgorithmName]::SHA256
  )
  Assert-Contract (
    $algorithm.KeySize -eq 256 -and
    $signer.KeySize -eq 256 -and
    $verifier.VerifyData(
      $payload,
      $signature,
      [System.Security.Cryptography.HashAlgorithmName]::SHA256
    )
  ) 'the native E2E entitlement fixture must export a usable P-256 key pair'
} finally {
  foreach ($instance in @($algorithm, $signer, $verifier)) {
    if ($instance) { $instance.Dispose() }
  }
}
Assert-Contract (
  $manifest -match 'tauri-plugin-wdio[^\r\n]+optional\s*=\s*true' -and
  $manifest -match 'tauri-plugin-wdio-webdriver[^\r\n]+optional\s*=\s*true'
) 'WebDriver crates must remain optional dependencies'
Assert-Contract (
  $composition -match 'cfg\(all\(feature = "desktop-e2e", not\(debug_assertions\)\)\)' -and
  $composition -match 'compile_error!'
) 'release builds must reject the desktop-e2e feature'
Assert-Contract (
  $harness -match '\[int\]\$SshPort\s*=\s*0' -and
  $harness -match 'if\s*\(\$SshPort\s+-gt\s+0\)'
) 'SSH config must remain authoritative unless a port is explicitly supplied'
Assert-Contract (
  $harness -match 'ServerAliveInterval=15' -and
  $harness -match '\$environment\.SshProcess\s*=\s*\$process' -and
  $harness -notmatch 'function\s+Invoke-Ssh\b'
) 'SSH provisioning, tunnel, diagnostics, and cleanup must share one bounded session'
Assert-Contract (
  $harness -match "ssh-provider-bootstrap\.log" -and
  $harness -match '\$details\.Length\s+-gt\s+8000'
) 'SSH bootstrap failures must retain complete diagnostics and bound console output'
Assert-Contract (
  $harness -match '\$SshBootstrapAttempts\s*=\s*6' -and
  $harness -match 'Start-Sleep\s+-Seconds\s+60' -and
  $harness -match 'kex_exchange_identification'
) 'SSH bootstrap retries must be bounded and limited to transport failures'
Assert-Contract (
  $harness.IndexOf("pnpm @('install', '--frozen-lockfile')") -lt
  $harness.IndexOf('switch ($Provider)') -and
  $harness -match 'scripts/test-native-driver\.mjs'
) 'native driver dependencies must be validated before any provider is provisioned'
Assert-Contract (
  $harness -match 'Microsoft\.VisualStudio\.Component\.VC\.Tools\.x86\.x64' -and
  $harness -match 'Microsoft\.VisualStudio\.Component\.Windows11SDK\.26100'
) 'native builds must discover the required MSVC and Windows SDK environment'
foreach ($provider in @('WslBundle', 'Wsl2Compose', 'SshCompose', 'Existing')) {
  Assert-Contract ($harness.Contains("'$provider'")) "the $provider provider is missing"
}
Assert-Contract (
  $harness -notmatch '(?im)ssh\s+us\b|/usr/local/etc/docker/camellia-nexus'
) 'the harness must not embed a machine-specific SSH target or deployment path'

Write-Host 'Native E2E isolation and provider contracts passed.'
