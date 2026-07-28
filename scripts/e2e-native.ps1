#requires -Version 7.6

[CmdletBinding()]
param(
  [ValidateSet('Doctor', 'Run')]
  [string]$Action = 'Run',

  [ValidateSet('WslBundle', 'Wsl2Compose', 'SshCompose', 'Existing')]
  [string]$Provider = 'WslBundle',

  [ValidateSet('smoke', 'full')]
  [string]$Suite = 'smoke',

  [string]$BundlePath,
  [string]$ServerRepository,
  [string]$WslDistribution,
  [string]$SshTarget,
  [string]$SshRepository,
  [ValidateRange(0, 65535)]
  [int]$SshPort = 0,
  [string]$SshIdentityFile,
  [string]$ServerBaseUrl,
  [string]$EntitlementAuthorityPath,
  [string]$ProCode,
  [string]$TeamCode,
  [string]$OutputDirectory,
  [switch]$SkipBuild,
  [switch]$KeepEnvironment
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$UiDirectory = Join-Path $RepositoryRoot 'ui'
$ExpectedNode = (Get-Content -Raw (Join-Path $RepositoryRoot '.node-version')).Trim()
$ExpectedPnpm = ((Get-Content -Raw (Join-Path $UiDirectory 'package.json') | ConvertFrom-Json).packageManager -split '@')[-1]
$ExpectedRust = ((Get-Content -Raw (Join-Path $RepositoryRoot 'rust-toolchain.toml') | Select-String -Pattern 'channel\s*=\s*"([^"]+)"').Matches.Groups[1].Value)
$ExpectedRustToolchain = "$ExpectedRust-x86_64-pc-windows-msvc"
$SshBootstrapAttempts = 6
Set-Location -LiteralPath $RepositoryRoot
[Environment]::CurrentDirectory = $RepositoryRoot

function Invoke-Checked {
  param(
    [Parameter(Mandatory)] [string]$FilePath,
    [Parameter()] [string[]]$Arguments = @()
  )
  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "$FilePath exited with code $LASTEXITCODE"
  }
}

function Get-CommandVersion {
  param(
    [Parameter(Mandatory)] [string]$FilePath,
    [Parameter(Mandatory)] [string[]]$Arguments
  )
  $value = (& $FilePath @Arguments 2>&1 | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) { throw "$FilePath version check failed" }
  return $value
}

function Initialize-MsvcEnvironment {
  if ((Get-Command link.exe -ErrorAction SilentlyContinue) -and
      (Get-Command rc.exe -ErrorAction SilentlyContinue)) {
    return
  }
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw 'Visual Studio with the x64 C++ tools and Windows SDK is required'
  }
  $vswhereArguments = @(
    '-latest', '-products', '*',
    '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
    '-requires', 'Microsoft.VisualStudio.Component.Windows11SDK.26100',
    '-property', 'installationPath'
  )
  $installationPath = @(& $vswhere @vswhereArguments) | Select-Object -First 1
  if (-not $installationPath) {
    throw 'Visual Studio with the x64 C++ tools and Windows 11 SDK 26100 is required'
  }
  $developerShell = Join-Path $installationPath 'Common7\Tools\Launch-VsDevShell.ps1'
  if (-not (Test-Path -LiteralPath $developerShell -PathType Leaf)) {
    throw "Visual Studio developer shell is missing from $installationPath"
  }
  & $developerShell -Arch amd64 -HostArch amd64 -NoLogo -SkipAutomaticLocation
  if (-not (Get-Command link.exe -ErrorAction SilentlyContinue) -or
      -not (Get-Command rc.exe -ErrorAction SilentlyContinue)) {
    throw 'Visual Studio did not expose the x64 linker and Windows resource compiler'
  }
}

function Assert-Toolchain {
  if ($env:OS -ne 'Windows_NT') {
    throw 'Native desktop E2E requires Windows'
  }
  if ($PSVersionTable.PSVersion -lt [version]'7.6.0') {
    throw "PowerShell 7.6 or newer is required; found $($PSVersionTable.PSVersion)"
  }
  $node = (Get-CommandVersion node @('--version')).TrimStart('v')
  if ($node -ne $ExpectedNode) { throw "Node.js $ExpectedNode is required; found $node" }
  Push-Location $UiDirectory
  try { $pnpm = Get-CommandVersion pnpm @('--version') }
  finally { Pop-Location }
  if ($pnpm -ne $ExpectedPnpm) { throw "pnpm $ExpectedPnpm is required; found $pnpm" }
  $null = Get-Command rustup -ErrorAction Stop
  $rust = Get-CommandVersion rustup @('run', $ExpectedRustToolchain, 'rustc', '--version')
  if ($rust -notmatch "^rustc $([regex]::Escape($ExpectedRust))\b") {
    throw "Rust $ExpectedRust is required; found $rust"
  }
  $null = Get-CommandVersion rustup @('run', $ExpectedRustToolchain, 'cargo', '--version')
  $env:RUSTUP_TOOLCHAIN = $ExpectedRustToolchain
  Initialize-MsvcEnvironment
  if ($Provider -in @('WslBundle', 'Wsl2Compose')) {
    $null = Get-Command wsl.exe -ErrorAction Stop
    Invoke-CapturedProcess wsl.exe @('--version') | Out-Null
    Invoke-CapturedProcess wsl.exe @('--status') | Out-Null
  }
  if ($Provider -eq 'SshCompose') {
    $null = Get-Command ssh.exe -ErrorAction Stop
  }
}

function Get-FreeTcpPort {
  $listener = [System.Net.Sockets.TcpListener]::new(
    [System.Net.IPAddress]::Loopback,
    0
  )
  $listener.Start()
  try { return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port }
  finally { $listener.Stop() }
}

function Get-SafeLogTail {
  param(
    [Parameter(Mandatory)] [string]$Path,
    [int]$TailLines = 80,
    [int]$MaximumCharacters = 8000
  )
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    return "Log file was not created: $Path"
  }
  try { $content = (Get-Content -LiteralPath $Path -Tail $TailLines | Out-String).Trim() }
  catch { return "Log file could not be read: $Path" }
  if (-not $content) { return "Log file is empty: $Path" }
  $content = $content.Replace('e2e-only-password', '[REDACTED]')
  $content = [regex]::Replace($content, 'postgres://[^@\s]+@', 'postgres://[REDACTED]@')
  $content = [regex]::Replace(
    $content,
    '(?s)-----BEGIN [^-]+-----.*?-----END [^-]+-----',
    '[REDACTED PEM]'
  )
  if ($content.Length -gt $MaximumCharacters) {
    $content = '...' + $content.Substring($content.Length - $MaximumCharacters)
  }
  return "Log tail ($Path):$([Environment]::NewLine)$content"
}

function Assert-BackgroundProcess {
  param(
    [Parameter(Mandatory)] [System.Diagnostics.Process]$Process,
    [Parameter(Mandatory)] [string]$Name,
    [Parameter(Mandatory)] [string]$LogPath
  )
  if (-not $Process.HasExited) { return }
  throw "$Name exited with code $($Process.ExitCode).$([Environment]::NewLine)$(Get-SafeLogTail $LogPath)"
}

function Wait-TcpPort {
  param(
    [Parameter(Mandatory)] [int]$Port,
    [int]$TimeoutSeconds = 45,
    [System.Diagnostics.Process]$Process,
    [string]$ProcessName = 'Background process',
    [string]$LogPath
  )
  $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
  do {
    if ($Process) { Assert-BackgroundProcess $Process $ProcessName $LogPath }
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
      $task = $client.ConnectAsync('127.0.0.1', $Port)
      if ($task.Wait(500) -and $client.Connected) { return }
    } catch {
      # Retry until the bounded deadline.
    } finally {
      $client.Dispose()
    }
    Start-Sleep -Milliseconds 250
  } while ([DateTimeOffset]::UtcNow -lt $deadline)
  if ($Process) {
    Assert-BackgroundProcess $Process $ProcessName $LogPath
    throw "$ProcessName is still running, but TCP port $Port did not become ready within $TimeoutSeconds seconds.$([Environment]::NewLine)$(Get-SafeLogTail $LogPath)"
  }
  throw "TCP port $Port did not become ready within $TimeoutSeconds seconds"
}

function Wait-HttpReady {
  param(
    [Parameter(Mandatory)] [uri]$Uri,
    [int]$TimeoutSeconds = 60,
    [System.Diagnostics.Process]$Process,
    [string]$ProcessName = 'Background process',
    [string]$LogPath
  )
  $deadline = [DateTimeOffset]::UtcNow.AddSeconds($TimeoutSeconds)
  do {
    if ($Process) { Assert-BackgroundProcess $Process $ProcessName $LogPath }
    try {
      $response = Invoke-WebRequest -UseBasicParsing -Uri $Uri -TimeoutSec 3
      if ($response.StatusCode -eq 200) { return }
    } catch {
      # Retry until the bounded deadline.
    }
    Start-Sleep -Milliseconds 500
  } while ([DateTimeOffset]::UtcNow -lt $deadline)
  if ($Process) {
    Assert-BackgroundProcess $Process $ProcessName $LogPath
    throw "$ProcessName is still running, but $Uri did not become ready within $TimeoutSeconds seconds.$([Environment]::NewLine)$(Get-SafeLogTail $LogPath)"
  }
  throw "$Uri did not become ready within $TimeoutSeconds seconds"
}

function ConvertTo-ShellLiteral {
  param([Parameter(Mandatory)] [AllowEmptyString()] [string]$Value)
  $quote = [string][char]39
  $replacement = [string][char]39 + [char]34 + [char]39 + [char]34 + [char]39
  return $quote + $Value.Replace($quote, $replacement) + $quote
}

function ConvertTo-WslPath {
  param([Parameter(Mandatory)] [string]$Path)
  $full = [System.IO.Path]::GetFullPath($Path)
  if ($full -notmatch '^([A-Za-z]):\\(.*)$') {
    throw "WSL E2E paths must be on a local Windows drive: $full"
  }
  $drive = $Matches[1].ToLowerInvariant()
  $tail = $Matches[2].Replace('\', '/')
  return "/mnt/$drive/$tail"
}

function Start-BackgroundProcess {
  param(
    [Parameter(Mandatory)] [string]$FilePath,
    [Parameter(Mandatory)] [string[]]$Arguments,
    [string]$WorkingDirectory,
    [switch]$CaptureOutput
  )
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = $FilePath
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  if ($WorkingDirectory) { $start.WorkingDirectory = $WorkingDirectory }
  if ($CaptureOutput) {
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
  }
  foreach ($argument in $Arguments) { $null = $start.ArgumentList.Add($argument) }
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $start
  if (-not $process.Start()) { throw "Could not start $FilePath" }
  return $process
}

function Invoke-CapturedProcess {
  param(
    [Parameter(Mandatory)] [string]$FilePath,
    [Parameter()] [string[]]$Arguments = @()
  )
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = $FilePath
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in $Arguments) { $null = $start.ArgumentList.Add($argument) }
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $start
  try {
    if (-not $process.Start()) { throw "Could not start $FilePath" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $stdout = $stdoutTask.GetAwaiter().GetResult().Trim()
    $stderr = $stderrTask.GetAwaiter().GetResult().Trim()
    if ($process.ExitCode -ne 0) {
      $details = if ($stderr) { $stderr } else { $stdout }
      throw "$FilePath exited with code $($process.ExitCode): $details"
    }
    return $stdout
  } finally {
    $process.Dispose()
  }
}

function Invoke-Wsl {
  param(
    [Parameter(Mandatory)] [string]$Distribution,
    [Parameter(Mandatory)] [string]$Command,
    [switch]$Capture
  )
  $output = Invoke-CapturedProcess wsl.exe @('-d', $Distribution, '--', 'sh', '-lc', $Command)
  if ($Capture) { return $output }
}

function Remove-WslDistribution {
  param(
    [Parameter(Mandatory)] [string]$Distribution,
    [switch]$BestEffort
  )
  try { Invoke-CapturedProcess wsl.exe @('--terminate', $Distribution) | Out-Null }
  catch { Write-Warning "Could not terminate WSL distribution $Distribution`: $($_.Exception.Message)" }
  try { Invoke-CapturedProcess wsl.exe @('--unregister', $Distribution) | Out-Null }
  catch {
    if (-not $BestEffort) { throw }
    Write-Warning "Could not unregister WSL distribution $Distribution`: $($_.Exception.Message)"
  }
}

function Set-WslFile {
  param(
    [Parameter(Mandatory)] [string]$Distribution,
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [string]$Content,
    [string]$Mode = '0600'
  )
  if (-not $Path.StartsWith('/') -or $Path.EndsWith('/') -or $Path -match '[\r\n\x00]') {
    throw 'WSL file paths must be absolute POSIX file paths'
  }
  if ($Mode -notmatch '^[0-7]{3,4}$') { throw 'WSL file mode must be an octal permission' }
  $separator = $Path.LastIndexOf('/')
  $parent = if ($separator -eq 0) { '/' } else { $Path.Substring(0, $separator) }
  $quotedPath = ConvertTo-ShellLiteral $Path
  $command = "umask 077; mkdir -p $(ConvertTo-ShellLiteral $parent); cat > $quotedPath; chmod $Mode $quotedPath"
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = 'wsl.exe'
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardInput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in @('-d', $Distribution, '--', 'sh', '-lc', $command)) {
    $null = $start.ArgumentList.Add($argument)
  }
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $start
  if (-not $process.Start()) { throw "Could not write $Path in $Distribution" }
  $process.StandardInput.Write($Content)
  $process.StandardInput.Close()
  $errorText = $process.StandardError.ReadToEnd()
  $process.WaitForExit()
  if ($process.ExitCode -ne 0) { throw "Could not write $Path in $Distribution`: $errorText" }
}

function New-KeyringJson {
  param([Parameter(Mandatory)] [string]$Kind)
  $bytes = [byte[]]::new(32)
  [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
  $key = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
  return (@{
      activeKeyId = "$Kind-e2e"
      keys = @{ "$Kind-e2e" = $key }
    } | ConvertTo-Json -Depth 4)
}

function New-EntitlementMaterial {
  param(
    [Parameter(Mandatory)] [string]$Directory,
    [Parameter(Mandatory)] [string]$Issuer
  )
  $algorithm = [System.Security.Cryptography.ECDsa]::Create(
    [System.Security.Cryptography.ECCurve+NamedCurves]::nistP256
  )
  try {
    $privatePem = $algorithm.ExportPkcs8PrivateKeyPem()
    $publicPem = $algorithm.ExportSubjectPublicKeyInfoPem()
  } finally {
    $algorithm.Dispose()
  }
  $authorityPath = Join-Path $Directory 'entitlement-authority.json'
  $authority = @{
    issuer = $Issuer
    audience = 'camellia-nexus-desktop'
    minimumLicenseEpoch = 0
    keys = @(@{ keyId = 'entitlement-e2e'; publicKeyPem = $publicPem })
  }
  [System.IO.File]::WriteAllText(
    $authorityPath,
    ($authority | ConvertTo-Json -Depth 6),
    [System.Text.UTF8Encoding]::new($false)
  )
  return @{
    PrivatePem = $privatePem
    PublicPem = $publicPem
    AuthorityPath = $authorityPath
  }
}

function Assert-Bundle {
  param([Parameter(Mandatory)] [string]$Path)
  $resolved = (Resolve-Path $Path).Path
  $manifestPath = Join-Path $resolved 'manifest.json'
  $manifest = Get-Content -Raw $manifestPath | ConvertFrom-Json
  if ($manifest.schemaVersion -ne 2) { throw 'Unsupported server E2E bundle schema' }
  if ($manifest.purpose -ne 'Camellia Nexus Windows native E2E only') {
    throw 'The supplied directory is not a Camellia Nexus E2E bundle'
  }
  if ($manifest.runtime.wslVersion -ne 2 -or
      $manifest.runtime.architecture -ne 'x86_64' -or
      $manifest.runtime.networking -ne 'shared-loopback') {
    throw 'The server E2E bundle does not declare the required WSL2 runtime'
  }
  if ($manifest.postgres.majorVersion -ne 18) {
    throw 'The server E2E bundle does not contain the required PostgreSQL major version'
  }
  $rootFiles = @(
    @{ Entry = $manifest.server.rootfs; Name = 'server-rootfs.tar' },
    @{ Entry = $manifest.postgres.rootfs; Name = 'postgres-rootfs.tar' }
  )
  foreach ($rootFile in $rootFiles) {
    $entry = $rootFile.Entry
    if ($entry.file -ne $rootFile.Name -or $entry.sha256 -notmatch '^[0-9a-f]{64}$' -or
        [long]$entry.bytes -le 0) {
      throw "Invalid bundle root filesystem metadata: $($rootFile.Name)"
    }
    $file = Join-Path $resolved $entry.file
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Bundle file is missing: $($entry.file)" }
    $item = Get-Item -LiteralPath $file
    if ($item.Length -ne [long]$entry.bytes) { throw "Bundle size mismatch: $($entry.file)" }
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
    if ($digest -ne $entry.sha256) { throw "Bundle digest mismatch: $($entry.file)" }
  }
  return @{ Root = $resolved; Manifest = $manifest }
}

function Assert-Wsl2Distribution {
  param([Parameter(Mandatory)] [string]$Distribution)
  $runtime = Invoke-Wsl $Distribution 'uname -m; cat /proc/sys/kernel/osrelease' -Capture
  $lines = @($runtime -split '\r?\n')
  if ($lines.Count -ne 2 -or $lines[0] -ne 'x86_64' -or $lines[1] -notmatch '(?i)wsl2') {
    throw "WSL distribution $Distribution did not start as x86_64 WSL2: $runtime"
  }
}

function Write-WslDistributionDiagnostics {
  param(
    [Parameter(Mandatory)] [string]$Distribution,
    [Parameter(Mandatory)] [string]$Path,
    [switch]$Postgres
  )
  $pathCommand = if ($Postgres) {
    'ls -ld /var/lib/postgresql /var/lib/postgresql/data /usr/local/bin/postgres /usr/local/bin/docker-entrypoint.sh 2>&1 || true'
  } else {
    'ls -ld /app /run/camellia-e2e /usr/local/bin/camellia-nexus-management-server 2>&1 || true'
  }
  $command = @(
    "printf '%s\n' '[kernel]'",
    'uname -srmo',
    "printf '%s\n' '[os]'",
    'cat /etc/os-release 2>&1 || true',
    "printf '%s\n' '[processes]'",
    'for status in /proc/[0-9]*/status; do [ -r "$status" ] || continue; awk ''/^(Name|State|Pid|PPid):/ { printf "%s%s", separator, $0; separator=" " } END { print "" }'' "$status"; done',
    "printf '%s\n' '[paths]'",
    $pathCommand
  ) -join '; '
  Write-DiagnosticLog $Path { Invoke-Wsl $Distribution $command -Capture }
}

function Stop-WslBundleProcesses {
  param([AllowNull()] [object[]]$Processes)
  foreach ($process in @($Processes)) {
    if (-not $process) { continue }
    try {
      if (-not $process.HasExited) { $process.Kill($true) }
      if (-not $process.WaitForExit(5000)) {
        Write-Warning "Background WSL process $($process.Id) did not exit within five seconds"
      }
    } catch {
      Write-Warning "Could not stop a background WSL process: $($_.Exception.Message)"
    } finally {
      $process.Dispose()
    }
  }
}

function New-WslBundleEnvironment {
  param(
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [string]$RunId,
    [Parameter(Mandatory)] [string]$RunDirectory
  )
  $bundle = Assert-Bundle $Path
  $serverDistribution = "camellia-nexus-E2E-$RunId-Server"
  $postgresDistribution = "camellia-nexus-E2E-$RunId-Postgres"
  $serverInstall = Join-Path $RunDirectory 'wsl-server'
  $postgresInstall = Join-Path $RunDirectory 'wsl-postgres'
  $serverProcess = $null
  $postgresProcess = $null
  $serverImported = $false
  $postgresImported = $false
  try {
    New-Item -ItemType Directory -Path $serverInstall, $postgresInstall | Out-Null
    Invoke-Checked wsl.exe @(
      '--import', $postgresDistribution, $postgresInstall,
      (Join-Path $bundle.Root $bundle.Manifest.postgres.rootfs.file), '--version', '2'
    ) | Out-Null
    $postgresImported = $true
    Assert-Wsl2Distribution $postgresDistribution
    Invoke-Checked wsl.exe @(
      '--import', $serverDistribution, $serverInstall,
      (Join-Path $bundle.Root $bundle.Manifest.server.rootfs.file), '--version', '2'
    ) | Out-Null
    $serverImported = $true
    Assert-Wsl2Distribution $serverDistribution

  $postgresPort = Get-FreeTcpPort
  $publicPort = Get-FreeTcpPort
  $issuer = "http://127.0.0.1:$publicPort"
  $material = New-EntitlementMaterial $RunDirectory $issuer
  Set-WslFile $serverDistribution '/run/camellia-e2e/entitlement-private.pem' $material.PrivatePem
  Set-WslFile $serverDistribution '/run/camellia-e2e/admin-keyring.json' (New-KeyringJson admin)
  Set-WslFile $serverDistribution '/run/camellia-e2e/workspace-keyring.json' (New-KeyringJson workspace)
  Set-WslFile $serverDistribution '/run/camellia-e2e/webhook-keyring.json' (New-KeyringJson webhook)

  $postgresLog = Join-Path $RunDirectory 'postgres.log'
  $serverLog = Join-Path $RunDirectory 'management-server.log'
  $postgresLogWsl = ConvertTo-WslPath $postgresLog
  $serverLogWsl = ConvertTo-WslPath $serverLog
  $postgresCommand = "export POSTGRES_DB=camellia_license POSTGRES_USER=camellia POSTGRES_PASSWORD=e2e-only-password PGDATA=/var/lib/postgresql/data; exec /usr/local/bin/docker-entrypoint.sh postgres -h 127.0.0.1 -p $postgresPort > $(ConvertTo-ShellLiteral $postgresLogWsl) 2>&1"
  $postgresProcess = Start-BackgroundProcess wsl.exe @('-d', $postgresDistribution, '--', 'sh', '-lc', $postgresCommand)
  Wait-TcpPort $postgresPort -Process $postgresProcess -ProcessName 'PostgreSQL WSL process' -LogPath $postgresLog

  $variables = [ordered]@{
    CAMELLIA_NEXUS_LICENSE_ENV = 'development'
    CAMELLIA_NEXUS_LICENSE_STORAGE = 'postgres'
    DATABASE_URL = "postgres://camellia:e2e-only-password@127.0.0.1:$postgresPort/camellia_license"
    CAMELLIA_NEXUS_LICENSE_ISSUER = $issuer
    CAMELLIA_NEXUS_LICENSE_AUDIENCE = 'camellia-nexus-desktop'
    CAMELLIA_NEXUS_LICENSE_KEY_ID = 'entitlement-e2e'
    CAMELLIA_NEXUS_LICENSE_PRIVATE_KEY_PEM_PATH = '/run/camellia-e2e/entitlement-private.pem'
    CAMELLIA_NEXUS_ADMIN_KEYRING_PATH = '/run/camellia-e2e/admin-keyring.json'
    CAMELLIA_NEXUS_WORKSPACE_KEYRING_PATH = '/run/camellia-e2e/workspace-keyring.json'
    CAMELLIA_NEXUS_WEBHOOK_KEYRING_PATH = '/run/camellia-e2e/webhook-keyring.json'
    CAMELLIA_NEXUS_OAUTH_CLIENT_ID = 'camellia-nexus-desktop'
    CAMELLIA_NEXUS_OAUTH_REDIRECT_URIS = 'camellia-nexus://auth/callback'
    CAMELLIA_NEXUS_CLIENT_MINIMUM_VERSION = '1.0.0'
    CAMELLIA_NEXUS_CLIENT_RECOMMENDED_VERSION = '1.0.0'
    CAMELLIA_NEXUS_CLIENT_VERSION_ENFORCE_AFTER = '2030-01-01T00:00:00Z'
    CAMELLIA_NEXUS_CORS_ALLOWED_ORIGINS = 'camellia-nexus://localhost'
    CAMELLIA_NEXUS_DATABASE_MIGRATE = 'false'
    CAMELLIA_NEXUS_LOG_FORMAT = 'json'
    CAMELLIA_NEXUS_LOG_LEVEL = 'trace'
    CAMELLIA_NEXUS_HTTP_LOG_LEVEL = 'trace'
    CAMELLIA_NEXUS_HTTP_FAILURE_LOG_LEVEL = 'trace'
  }
  $exports = ($variables.GetEnumerator() | ForEach-Object {
      "export $($_.Key)=$(ConvertTo-ShellLiteral ([string]$_.Value))"
    }) -join '; '
  $binary = '/usr/local/bin/camellia-nexus-management-server'
  Invoke-Wsl $serverDistribution "cd /app; $exports; $binary migrate" | Out-Null

  $suffix = $RunId.ToLowerInvariant()
  $operator = 'native_e2e'
  $reason = 'native desktop end-to-end verification'
  function Invoke-AdminJson {
    param([Parameter(Mandatory)] [string[]]$Arguments)
    $argumentsText = ($Arguments | ForEach-Object { ConvertTo-ShellLiteral $_ }) -join ' '
    $json = Invoke-Wsl $serverDistribution "cd /app; $exports; $binary admin --raw $argumentsText" -Capture
    return $json | ConvertFrom-Json
  }

  $freeAccount = "acct_e2e_free_$suffix"
  $freeLicense = "lic_e2e_free_$suffix"
  $proAccount = "acct_e2e_pro_$suffix"
  $proLicense = "lic_e2e_pro_$suffix"
  $teamAccount = "acct_e2e_team_$suffix"
  $teamLicense = "lic_e2e_team_$suffix"
  $billingOffer = "offer_e2e_pro_$suffix"
  $billingMethod = "method_e2e_$suffix"
  $billingInvoice = "invoice_e2e_pro_$suffix"
  $null = Invoke-AdminJson @('account', 'create', $freeAccount, '--actor', $operator, '--reason', $reason)
  $null = Invoke-AdminJson @('license', 'create', $freeLicense, '--account', $freeAccount, '--plan', 'free', '--actor', $operator, '--reason', $reason)
  $freePrimaryDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $freeAccount, '--license', $freeLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-free-primary-code"
  )
  $freeSecondDeviceDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $freeAccount, '--license', $freeLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-free-second-device-code"
  )
  $null = Invoke-AdminJson @('account', 'create', $proAccount, '--actor', $operator, '--reason', $reason)
  $null = Invoke-AdminJson @('license', 'create', $proLicense, '--account', $proAccount, '--plan', 'pro', '--actor', $operator, '--reason', $reason)
  $proPrimaryDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $proAccount, '--license', $proLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-pro-primary-code"
  )
  $proRecoveryDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $proAccount, '--license', $proLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-pro-recovery-code"
  )
  $null = Invoke-AdminJson @(
    'billing', 'offer', 'create', $billingOffer, '--plan', 'pro',
    '--duration-days', '365', '--currency', 'USD', '--amount', '99.00',
    '--name-en', 'Native E2E Pro annual', '--name-zh', 'Native E2E Pro annual',
    '--actor', $operator, '--reason', $reason
  )
  $null = Invoke-AdminJson @(
    'billing', 'method', 'create', $billingMethod,
    '--name-en', 'Native E2E bank transfer', '--name-zh', 'Native E2E bank transfer',
    '--instructions-en', 'Use the synthetic E2E reference.',
    '--instructions-zh', 'Use the synthetic E2E reference.', '--asset', 'USD',
    '--destination-hint', 'E2E-ONLY', '--actor', $operator, '--reason', $reason
  )
  $null = Invoke-AdminJson @(
    'billing', 'invoice', 'create', $billingInvoice, '--account', $proAccount,
    '--license', $proLicense, '--offer', $billingOffer, '--due-at', '30d',
    '--actor', $operator, '--reason', $reason
  )
  $null = Invoke-AdminJson @('account', 'create', $teamAccount, '--actor', $operator, '--reason', $reason)
  $null = Invoke-AdminJson @('license', 'create', $teamLicense, '--account', $teamAccount, '--plan', 'team', '--seats', '3', '--actor', $operator, '--reason', $reason)
  $teamOwnerDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $teamAccount, '--license', $teamLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-team-owner-code"
  )
  $teamMemberDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $teamAccount, '--license', $teamLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-team-member-code"
  )
  $teamAdditionalDeviceDelivery = Invoke-AdminJson @(
    'code', 'issue', '--account', $teamAccount, '--license', $teamLicense,
    '--issued-by', $operator, '--reason', $reason,
    '--operation-id', "native-e2e-$suffix-team-additional-device-code"
  )

  $serveVariables = [ordered]@{}
  foreach ($entry in $variables.GetEnumerator()) {
    $serveVariables[$entry.Key] = $entry.Value
  }
  $serveVariables.CAMELLIA_NEXUS_LICENSE_BIND = "0.0.0.0:$publicPort"
  $serveExports = ($serveVariables.GetEnumerator() | ForEach-Object {
      "export $($_.Key)=$(ConvertTo-ShellLiteral ([string]$_.Value))"
    }) -join '; '
  $serverCommand = "cd /app; $serveExports; echo `$`$ > /run/camellia-e2e/server.pid; exec $binary serve > $(ConvertTo-ShellLiteral $serverLogWsl) 2>&1"
  $serverProcess = Start-BackgroundProcess wsl.exe @('-d', $serverDistribution, '--', 'sh', '-lc', $serverCommand)
  Wait-HttpReady "$issuer/readyz" -Process $serverProcess -ProcessName 'Management server WSL process' -LogPath $serverLog

    return @{
      BaseUrl = $issuer
      AuthorityPath = $material.AuthorityPath
      FreeAccountId = $freeAccount
      FreeLicenseId = $freeLicense
      FreePrimaryCode = [string]$freePrimaryDelivery.code
      FreeSecondDeviceCode = [string]$freeSecondDeviceDelivery.code
      ProAccountId = $proAccount
      ProLicenseId = $proLicense
      ProPrimaryCode = [string]$proPrimaryDelivery.code
      ProRecoveryCode = [string]$proRecoveryDelivery.code
      BillingOfferId = $billingOffer
      BillingPaymentMethodId = $billingMethod
      BillingInvoiceId = $billingInvoice
      TeamAccountId = $teamAccount
      TeamLicenseId = $teamLicense
      TeamOwnerCode = [string]$teamOwnerDelivery.code
      TeamMemberCode = [string]$teamMemberDelivery.code
      TeamAdditionalDeviceCode = [string]$teamAdditionalDeviceDelivery.code
      AdminPrefix = "cd /app; $exports; $binary admin --raw"
      ServerDistribution = $serverDistribution
      PostgresDistribution = $postgresDistribution
      ServerProcess = $serverProcess
      PostgresProcess = $postgresProcess
    }
  } catch {
    $failure = $_
    if ($serverImported) {
      Write-WslDistributionDiagnostics $serverDistribution (Join-Path $RunDirectory 'server-wsl-diagnostics.log')
    }
    if ($postgresImported) {
      Write-WslDistributionDiagnostics $postgresDistribution (Join-Path $RunDirectory 'postgres-wsl-diagnostics.log') -Postgres
    }
    Stop-WslBundleProcesses -Processes @($serverProcess, $postgresProcess)
    if ($serverImported) { Remove-WslDistribution $serverDistribution -BestEffort }
    if ($postgresImported) { Remove-WslDistribution $postgresDistribution -BestEffort }
    throw $failure
  }
}

function Resolve-ServerRepository {
  $candidate = if ($ServerRepository) {
    $ServerRepository
  } else {
    Join-Path (Split-Path -Parent $RepositoryRoot) 'nexus-management-server'
  }
  $resolved = (Resolve-Path -LiteralPath $candidate -ErrorAction Stop).Path
  if (-not (Test-Path -LiteralPath (Join-Path $resolved 'scripts/provision-e2e-compose.sh') -PathType Leaf)) {
    throw "The management-server E2E provisioner is missing from $resolved"
  }
  return $resolved
}

function Resolve-WslDistribution {
  if ($WslDistribution) {
    $candidates = @($WslDistribution)
  } else {
    $raw = Invoke-CapturedProcess wsl.exe @('--list', '--quiet')
    $candidates = @(
      $raw.Replace([string][char]0, '').Split([Environment]::NewLine) |
        ForEach-Object { $_.Trim() } |
        Where-Object { $_ -and $_ -notmatch '^(docker-desktop|camellia-nexus-E2E-)' }
    )
  }
  $available = @()
  foreach ($candidate in $candidates) {
    try {
      Invoke-Wsl $candidate "grep -qi 'wsl2' /proc/sys/kernel/osrelease && docker compose version >/dev/null" | Out-Null
      $available += $candidate
    } catch {
      if ($WslDistribution) { throw "WSL distribution '$candidate' must use WSL2 and provide Docker Compose: $($_.Exception.Message)" }
    }
  }
  if ($available.Count -ne 1) {
    throw 'Specify WslDistribution when exactly one WSL2 distribution with Docker Compose cannot be selected automatically'
  }
  return $available[0]
}

function Convert-ComposeDescriptor {
  param(
    [Parameter(Mandatory)] [string]$Json,
    [Parameter(Mandatory)] [string]$RunDirectory,
    [string]$BaseUrlOverride
  )
  try { $descriptor = $Json | ConvertFrom-Json }
  catch { throw "The E2E provider returned an invalid JSON descriptor: $($_.Exception.Message)" }
  if ($descriptor.schemaVersion -ne 3 -or $descriptor.environmentId -notmatch '^[a-f0-9]{12}$') {
    throw 'The E2E provider returned an unsupported descriptor'
  }
  foreach ($property in @('baseUrl', 'issuer', 'publicKeyPemBase64')) {
    if (-not [string]$descriptor.$property) { throw "The E2E descriptor is missing $property" }
  }
  $free = $descriptor.fixtures.free
  $pro = $descriptor.fixtures.pro
  $team = $descriptor.fixtures.team
  foreach ($value in @(
      $free.accountId,
      $free.licenseId,
      $free.activationCodes.primary,
      $free.activationCodes.secondDevice,
      $pro.accountId,
      $pro.licenseId,
      $pro.activationCodes.primary,
      $pro.activationCodes.recovery,
      $pro.billing.offerId,
      $pro.billing.paymentMethodId,
      $pro.billing.invoiceId,
      $team.accountId,
      $team.licenseId,
      $team.activationCodes.owner,
      $team.activationCodes.member,
      $team.activationCodes.additionalDevice
    )) {
    if (-not [string]$value) { throw 'The E2E descriptor contains an incomplete fixture set' }
  }
  try {
    $publicKey = [System.Text.Encoding]::UTF8.GetString(
      [Convert]::FromBase64String([string]$descriptor.publicKeyPemBase64)
    )
  } catch {
    throw 'The E2E descriptor contains an invalid entitlement public key'
  }
  if ($publicKey -notmatch '^-----BEGIN PUBLIC KEY-----') {
    throw 'The E2E descriptor entitlement key is not a PEM public key'
  }
  $baseUrl = if ($BaseUrlOverride) { $BaseUrlOverride } else { [string]$descriptor.baseUrl }
  try {
    $baseUri = [uri]$baseUrl
    $issuerUri = [uri][string]$descriptor.issuer
  } catch {
    throw 'The E2E descriptor contains an invalid service URL or issuer'
  }
  if (-not $baseUri.IsAbsoluteUri -or $baseUri.Scheme -notin @('http', 'https') -or
      -not $issuerUri.IsAbsoluteUri -or $issuerUri.Scheme -notin @('http', 'https')) {
    throw 'The E2E descriptor service URL and issuer must be absolute HTTP(S) URLs'
  }
  $authorityPath = Join-Path $RunDirectory 'entitlement-authority.json'
  $authority = @{
    issuer = $issuerUri.AbsoluteUri.TrimEnd('/')
    audience = 'camellia-nexus-desktop'
    minimumLicenseEpoch = 0
    keys = @(@{ keyId = 'entitlement-e2e'; publicKeyPem = $publicKey })
  }
  [System.IO.File]::WriteAllText(
    $authorityPath,
    ($authority | ConvertTo-Json -Depth 6),
    [System.Text.UTF8Encoding]::new($false)
  )
  return @{
    BaseUrl = $baseUri.AbsoluteUri.TrimEnd('/')
    AuthorityPath = $authorityPath
    FreeAccountId = [string]$free.accountId
    FreeLicenseId = [string]$free.licenseId
    FreePrimaryCode = [string]$free.activationCodes.primary
    FreeSecondDeviceCode = [string]$free.activationCodes.secondDevice
    ProAccountId = [string]$pro.accountId
    ProLicenseId = [string]$pro.licenseId
    ProPrimaryCode = [string]$pro.activationCodes.primary
    ProRecoveryCode = [string]$pro.activationCodes.recovery
    BillingOfferId = [string]$pro.billing.offerId
    BillingPaymentMethodId = [string]$pro.billing.paymentMethodId
    BillingInvoiceId = [string]$pro.billing.invoiceId
    TeamAccountId = [string]$team.accountId
    TeamLicenseId = [string]$team.licenseId
    TeamOwnerCode = [string]$team.activationCodes.owner
    TeamMemberCode = [string]$team.activationCodes.member
    TeamAdditionalDeviceCode = [string]$team.activationCodes.additionalDevice
    EnvironmentId = [string]$descriptor.environmentId
  }
}

function New-WslComposeEnvironment {
  param([Parameter(Mandatory)] [string]$RunDirectory)
  $repository = Resolve-ServerRepository
  $distribution = Resolve-WslDistribution
  $wslRepository = ConvertTo-WslPath $repository
  $wslStateRoot = ConvertTo-WslPath (Join-Path $RunDirectory 'compose-state')
  $hostPort = Get-FreeTcpPort
  $provisioner = "$wslRepository/scripts/provision-e2e-compose.sh"
  $stateAssignment = "CAMELLIA_NEXUS_E2E_STATE_ROOT=$(ConvertTo-ShellLiteral $wslStateRoot)"
  $command = "$stateAssignment $(ConvertTo-ShellLiteral $provisioner) up --source $(ConvertTo-ShellLiteral $wslRepository) --host-port $hostPort --bind 0.0.0.0"
  $descriptor = Invoke-Wsl $distribution $command -Capture
  $environmentId = $null
  try {
    $environment = Convert-ComposeDescriptor $descriptor $RunDirectory
    $environmentId = $environment.EnvironmentId
    $environment.ProviderDistribution = $distribution
    $environment.ProviderProvisioner = $provisioner
    $environment.ProviderStateRoot = $wslStateRoot
    Wait-HttpReady "$($environment.BaseUrl)/readyz" -TimeoutSeconds 180
    return $environment
  } catch {
    if (-not $environmentId) {
      try { $environmentId = [string](($descriptor | ConvertFrom-Json).environmentId) } catch { }
    }
    if ($environmentId -match '^[a-f0-9]{12}$') {
      try {
        Invoke-Wsl $distribution "$stateAssignment $(ConvertTo-ShellLiteral $provisioner) down $(ConvertTo-ShellLiteral $environmentId)" | Out-Null
      } catch { Write-Warning "Could not clean the failed WSL2 E2E environment: $($_.Exception.Message)" }
    }
    throw
  }
}

function Get-SshBaseArguments {
  $arguments = @('-o', 'BatchMode=yes')
  if ($SshPort -gt 0) {
    $arguments += @('-p', [string]$SshPort)
  }
  if ($SshIdentityFile) {
    $arguments += @('-i', (Resolve-Path -LiteralPath $SshIdentityFile -ErrorAction Stop).Path)
  }
  return $arguments
}

function Assert-SshSettings {
  if (-not $SshTarget -or $SshTarget.StartsWith('-') -or $SshTarget -match '[\r\n\x00]') {
    throw 'SshTarget must be a non-option SSH host or user@host value'
  }
  if (-not $SshRepository -or -not $SshRepository.StartsWith('/') -or $SshRepository -match '[\r\n\x00]') {
    throw 'SshRepository must be an absolute POSIX path to the management-server checkout'
  }
}

function New-SshComposeEnvironmentAttempt {
  param([Parameter(Mandatory)] [string]$RunDirectory)
  Assert-SshSettings
  if ($KeepEnvironment) {
    throw 'KeepEnvironment is not supported by SshCompose because its loopback tunnel is process-scoped'
  }
  $provisioner = "$SshRepository/scripts/provision-e2e-compose.sh"
  $localPort = Get-FreeTcpPort
  $remotePort = [System.Security.Cryptography.RandomNumberGenerator]::GetInt32(20000, 60001)
  $remoteCommand = @'
set -eu
provisioner=__PROVISIONER__
source_root=__SOURCE_ROOT__
environment_id=''
cleanup() {
  if [ -n "$environment_id" ]; then
    "$provisioner" logs "$environment_id" || true
    "$provisioner" down "$environment_id"
  fi
}
trap cleanup EXIT HUP INT TERM
descriptor="$("$provisioner" up --source "$source_root" --host-port __REMOTE_PORT__ --bind 127.0.0.1)"
environment_id="$(printf '%s' "$descriptor" | jq -er '.environmentId')"
printf 'CAMELLIA_NEXUS_DESCRIPTOR:%s\n' "$(printf '%s' "$descriptor" | base64 | tr -d '\n')"
while IFS= read -r control; do
  case "$control" in
    pause|resume)
      "$provisioner" "$control" "$environment_id"
      ;;
    account-state:*)
      old_ifs="$IFS"
      IFS=:
      set -- $control
      IFS="$old_ifs"
      [ "$#" -eq 3 ]
      "$provisioner" account-state "$environment_id" "$2" "$3"
      ;;
    cleanup)
      break
      ;;
    *)
      echo "unsupported E2E control command" >&2
      exit 2
      ;;
  esac
  printf 'CAMELLIA_NEXUS_CONTROL:%s\n' "$control"
done
'@
  $remoteCommand = $remoteCommand.Replace(
    '__PROVISIONER__',
    (ConvertTo-ShellLiteral $provisioner)
  ).Replace(
    '__SOURCE_ROOT__',
    (ConvertTo-ShellLiteral $SshRepository)
  ).Replace('__REMOTE_PORT__', [string]$remotePort)
  $arguments = @(Get-SshBaseArguments) + @(
    '-o', 'ExitOnForwardFailure=yes',
    '-o', 'ServerAliveInterval=15',
    '-o', 'ServerAliveCountMax=3',
    '-L', "$localPort`:127.0.0.1:$remotePort",
    $SshTarget,
    $remoteCommand
  )
  $start = [System.Diagnostics.ProcessStartInfo]::new()
  $start.FileName = 'ssh.exe'
  $start.UseShellExecute = $false
  $start.CreateNoWindow = $true
  $start.RedirectStandardInput = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in $arguments) { $null = $start.ArgumentList.Add($argument) }
  $process = [System.Diagnostics.Process]::new()
  $process.StartInfo = $start
  $errorTask = $null
  try {
    if (-not $process.Start()) { throw 'Could not start the SSH E2E provider' }
    $errorTask = $process.StandardError.ReadToEndAsync()
    $descriptorLine = $process.StandardOutput.ReadLine()
    if (-not $descriptorLine -or -not $descriptorLine.StartsWith('CAMELLIA_NEXUS_DESCRIPTOR:')) {
      $process.WaitForExit()
      $details = $errorTask.GetAwaiter().GetResult().Trim()
      if (-not $details) { $details = 'the provider closed before returning its descriptor' }
      $diagnosticPath = Join-Path $RunDirectory 'ssh-provider-bootstrap.log'
      [System.IO.File]::WriteAllText(
        $diagnosticPath,
        $details,
        [System.Text.UTF8Encoding]::new($false)
      )
      $summary = if ($details.Length -gt 8000) {
        '…' + $details.Substring($details.Length - 8000)
      } else {
        $details
      }
      throw "The SSH E2E provider failed. Full diagnostics: $diagnosticPath$([Environment]::NewLine)$summary"
    }
    try {
      $descriptorJson = [System.Text.Encoding]::UTF8.GetString(
        [Convert]::FromBase64String($descriptorLine.Substring('CAMELLIA_NEXUS_DESCRIPTOR:'.Length))
      )
      $descriptor = $descriptorJson | ConvertFrom-Json
    } catch {
      throw 'The SSH E2E provider returned an invalid encoded JSON descriptor'
    }
    try { $remoteUri = [uri][string]$descriptor.baseUrl }
    catch { throw 'The SSH E2E descriptor contains an invalid service URL' }
    if ($remoteUri.Scheme -ne 'http' -or -not $remoteUri.IsLoopback -or
        $remoteUri.Port -ne $remotePort) {
      throw 'The SSH E2E service did not bind the requested remote loopback port'
    }
    Wait-TcpPort $localPort
    $environment = Convert-ComposeDescriptor $descriptorJson $RunDirectory "http://127.0.0.1:$localPort"
    $environment.ProviderTarget = $SshTarget
    $environment.ProviderProvisioner = $provisioner
    $environment.SshProcess = $process
    $environment.SshErrorTask = $errorTask
    Wait-HttpReady "$($environment.BaseUrl)/readyz" -TimeoutSeconds 180
    return $environment
  } catch {
    if ($process -and -not $process.HasExited) {
      try {
        $process.StandardInput.WriteLine('cleanup')
        $process.StandardInput.Close()
      } catch { }
      if (-not $process.WaitForExit(180000)) { $process.Kill($true) }
    }
    if ($process) { $process.Dispose() }
    throw
  }
}

function New-SshComposeEnvironment {
  param([Parameter(Mandatory)] [string]$RunDirectory)
  for ($attempt = 1; $attempt -le $SshBootstrapAttempts; $attempt++) {
    try {
      return (New-SshComposeEnvironmentAttempt $RunDirectory)
    } catch {
      $transportFailure = $_.Exception.Message -match
        '(?i)kex_exchange_identification|banner exchange|connection (?:closed|refused|reset|timed out)|ssh_exchange_identification'
      if (-not $transportFailure -or $attempt -eq $SshBootstrapAttempts) { throw }
      Write-Warning "SSH bootstrap attempt $attempt failed before provisioning; retrying in 60 seconds"
      Start-Sleep -Seconds 60
    }
  }
}

function Write-DiagnosticLog {
  param(
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [scriptblock]$ReadLog
  )
  try {
    $content = & $ReadLog
    [System.IO.File]::WriteAllText($Path, $content, [System.Text.UTF8Encoding]::new($false))
  } catch {
    Write-Warning "Could not collect provider diagnostics: $($_.Exception.Message)"
  }
}

function Invoke-ComposeControl {
  param(
    [Parameter(Mandatory)] [hashtable]$Environment,
    [Parameter(Mandatory)] [string]$Command
  )
  if ($Command -notmatch '^(pause|resume|account-state:[A-Za-z0-9_-]{1,128}:(active|suspended|denylisted))$') {
    throw 'Invalid disposable-provider control command'
  }
  if ($Provider -eq 'Wsl2Compose') {
    $parts = $Command.Split(':')
    $arguments = if ($parts[0] -eq 'account-state') {
      "account-state $(ConvertTo-ShellLiteral $Environment.EnvironmentId) $(ConvertTo-ShellLiteral $parts[1]) $(ConvertTo-ShellLiteral $parts[2])"
    } else {
      "$($parts[0]) $(ConvertTo-ShellLiteral $Environment.EnvironmentId)"
    }
    $stateAssignment = "CAMELLIA_NEXUS_E2E_STATE_ROOT=$(ConvertTo-ShellLiteral $Environment.ProviderStateRoot)"
    Invoke-Wsl $Environment.ProviderDistribution "$stateAssignment $(ConvertTo-ShellLiteral $Environment.ProviderProvisioner) $arguments" | Out-Null
    return
  }
  if ($Provider -eq 'SshCompose') {
    $Environment.SshProcess.StandardInput.WriteLine($Command)
    $acknowledgement = $Environment.SshProcess.StandardOutput.ReadLine()
    if ($acknowledgement -ne "CAMELLIA_NEXUS_CONTROL:$Command") {
      throw "The SSH E2E provider did not acknowledge $Command"
    }
    return
  }
  throw "Compose controls are unavailable for $Provider"
}

function Set-E2eServerAvailability {
  param(
    [Parameter(Mandatory)] [hashtable]$Environment,
    [Parameter(Mandatory)] [bool]$Available
  )
  if ($Provider -eq 'WslBundle') {
    $signal = if ($Available) { 'CONT' } else { 'STOP' }
    Invoke-Wsl $Environment.ServerDistribution "kill -$signal `"`$(cat /run/camellia-e2e/server.pid)`"" | Out-Null
  } else {
    Invoke-ComposeControl $Environment $(if ($Available) { 'resume' } else { 'pause' })
  }
  if ($Available) {
    Wait-HttpReady "$($Environment.BaseUrl)/readyz" -TimeoutSeconds 180
  }
}

function Set-E2eAccountState {
  param(
    [Parameter(Mandatory)] [hashtable]$Environment,
    [Parameter(Mandatory)] [string]$AccountId,
    [ValidateSet('active', 'suspended', 'denylisted')]
    [string]$State
  )
  if ($AccountId -notmatch '^[A-Za-z0-9_-]{1,128}$') {
    throw 'Invalid controlled E2E account identifier'
  }
  if ($Provider -eq 'WslBundle') {
    $arguments = @(
      'account', 'set-state', $AccountId, $State,
      '--actor', 'native_e2e_control',
      '--reason', 'native E2E controlled account transition'
    ) | ForEach-Object { ConvertTo-ShellLiteral $_ }
    Invoke-Wsl $Environment.ServerDistribution "$($Environment.AdminPrefix) $($arguments -join ' ')" | Out-Null
  } else {
    Invoke-ComposeControl $Environment "account-state:$AccountId`:$State"
  }
}

function Remove-ComposeEnvironment {
  param(
    [Parameter(Mandatory)] [hashtable]$Environment,
    [Parameter(Mandatory)] [string]$RunDirectory
  )
  $environmentId = ConvertTo-ShellLiteral $Environment.EnvironmentId
  if ($Provider -eq 'Wsl2Compose') {
    $stateAssignment = "CAMELLIA_NEXUS_E2E_STATE_ROOT=$(ConvertTo-ShellLiteral $Environment.ProviderStateRoot)"
    Write-DiagnosticLog (Join-Path $RunDirectory 'management-server-compose.log') {
      Invoke-Wsl $Environment.ProviderDistribution "$stateAssignment $(ConvertTo-ShellLiteral $Environment.ProviderProvisioner) logs $environmentId" -Capture
    }
    Invoke-Wsl $Environment.ProviderDistribution "$stateAssignment $(ConvertTo-ShellLiteral $Environment.ProviderProvisioner) down $environmentId" | Out-Null
  } elseif ($Provider -eq 'SshCompose') {
    $process = $Environment.SshProcess
    try {
      $process.StandardInput.WriteLine('cleanup')
      $process.StandardInput.Close()
      $outputTask = $process.StandardOutput.ReadToEndAsync()
      if (-not $process.WaitForExit(180000)) {
        $process.Kill($true)
        throw 'The SSH E2E provider did not clean up within 180 seconds'
      }
      $content = $outputTask.GetAwaiter().GetResult()
      $errorContent = $Environment.SshErrorTask.GetAwaiter().GetResult()
      if ($process.ExitCode -ne 0) {
        throw "The SSH E2E provider cleanup failed with code $($process.ExitCode): $errorContent"
      }
      [System.IO.File]::WriteAllText(
        (Join-Path $RunDirectory 'management-server-compose.log'),
        "$errorContent$([Environment]::NewLine)$content",
        [System.Text.UTF8Encoding]::new($false)
      )
    } finally {
      if (-not $process.HasExited) { $process.Kill($true) }
      $process.Dispose()
    }
  }
}

function Remove-WslBundleEnvironment {
  param(
    [Parameter(Mandatory)] [hashtable]$Environment,
    [Parameter(Mandatory)] [string]$RunDirectory
  )
  Write-WslDistributionDiagnostics $Environment.ServerDistribution (Join-Path $RunDirectory 'server-wsl-diagnostics.log')
  Write-WslDistributionDiagnostics $Environment.PostgresDistribution (Join-Path $RunDirectory 'postgres-wsl-diagnostics.log') -Postgres
  Stop-WslBundleProcesses -Processes @($Environment.ServerProcess, $Environment.PostgresProcess)
  foreach ($distribution in @($Environment.ServerDistribution, $Environment.PostgresDistribution)) {
    Remove-WslDistribution $distribution
  }
}

function Set-ProcessEnvironment {
  param([Parameter(Mandatory)] [hashtable]$Values)
  $previous = @{}
  foreach ($entry in $Values.GetEnumerator()) {
    $previous[$entry.Key] = [Environment]::GetEnvironmentVariable($entry.Key, 'Process')
    [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, 'Process')
  }
  return $previous
}

function Restore-ProcessEnvironment {
  param([Parameter(Mandatory)] [hashtable]$Previous)
  foreach ($entry in $Previous.GetEnumerator()) {
    [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
  }
}

function Initialize-E2eIdentity {
  param(
    [Parameter(Mandatory)] [string]$Identity,
    [Parameter(Mandatory)] [string]$DataRoot
  )
  if ($Identity -notmatch '^[a-z][a-z0-9-]{0,31}$') {
    throw 'Invalid native E2E identity name'
  }
  $directory = Join-Path $DataRoot $Identity
  if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
    New-Item -ItemType Directory -Path $directory | Out-Null
    $settings = @{
      version = 1
      logRetention = 'preserve'
      logLevel = 'trace'
      programStartupDelayMs = 0
      language = 'en'
    } | ConvertTo-Json
    [System.IO.File]::WriteAllText(
      (Join-Path $directory 'settings.json'),
      $settings,
      [System.Text.UTF8Encoding]::new($false)
    )
  }
  return $directory
}

function Publish-E2eHandoff {
  param([Parameter(Mandatory)] [string]$Path)
  $temporaryPath = "$Path.$([guid]::NewGuid().ToString('N')).tmp"
  try {
    [System.IO.File]::WriteAllText(
      $temporaryPath,
      '{}',
      [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::Move($temporaryPath, $Path)
  } finally {
    if (Test-Path -LiteralPath $temporaryPath -PathType Leaf) {
      Remove-Item -LiteralPath $temporaryPath -Force
    }
  }
}

function Write-CapturedProcessTranscript {
  param(
    [Parameter(Mandatory)] [System.Threading.Tasks.Task]$StandardOutput,
    [Parameter(Mandatory)] [System.Threading.Tasks.Task]$StandardError,
    [Parameter(Mandatory)] [string]$Path
  )
  $stdout = [string]$StandardOutput.GetAwaiter().GetResult()
  $stderr = [string]$StandardError.GetAwaiter().GetResult()
  $separator = [Environment]::NewLine
  $content = @('STDOUT', $stdout, 'STDERR', $stderr) -join $separator
  [System.IO.File]::WriteAllText($Path, $content, [System.Text.UTF8Encoding]::new($false))
  if ($stdout) { [Console]::Out.Write($stdout) }
  if ($stderr) { [Console]::Error.Write($stderr) }
}

function Invoke-CoordinatedNativePhase {
  param(
    [Parameter(Mandatory)] [string]$ReadySignal,
    [Parameter(Mandatory)] [string]$AppliedSignal,
    [Parameter(Mandatory)] [scriptblock]$OnReady
  )
  $handoffRoot = $env:CAMELLIA_NEXUS_E2E_HANDOFF_DIR
  if (-not $handoffRoot -or -not (Test-Path -LiteralPath $handoffRoot -PathType Container)) {
    throw 'The native E2E handoff directory is unavailable'
  }
  foreach ($signal in @($ReadySignal, $AppliedSignal)) {
    if ($signal -notmatch '^[a-z][a-z0-9-]{0,63}\.(?:json|token)$') {
      throw 'Invalid native E2E handoff signal name'
    }
  }
  $readyPath = Join-Path $handoffRoot $ReadySignal
  $appliedPath = Join-Path $handoffRoot $AppliedSignal
  $outputDirectory = $env:CAMELLIA_NEXUS_E2E_OUTPUT_DIR
  if (-not $outputDirectory) { throw 'The native E2E output directory is unavailable' }
  [System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
  $phaseLogPath = Join-Path $outputDirectory 'native-phase-process.log'
  $phaseProcess = Start-BackgroundProcess `
    -FilePath $env:ComSpec `
    -Arguments @('/d', '/s', '/c', 'pnpm.CMD test:native') `
    -WorkingDirectory $UiDirectory `
    -CaptureOutput
  $stdoutTask = $phaseProcess.StandardOutput.ReadToEndAsync()
  $stderrTask = $phaseProcess.StandardError.ReadToEndAsync()
  $phaseFailure = $null
  $transcriptFailure = $null
  try {
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds(60)
    while (-not (Test-Path -LiteralPath $readyPath -PathType Leaf)) {
      if ($phaseProcess.HasExited) {
        throw "Native phase exited with code $($phaseProcess.ExitCode) before publishing $ReadySignal"
      }
      if ([DateTimeOffset]::UtcNow -ge $deadline) {
        throw "Native phase did not publish $ReadySignal within 60 seconds"
      }
      Start-Sleep -Milliseconds 100
    }
    & $OnReady
    Publish-E2eHandoff $appliedPath
    if (-not $phaseProcess.WaitForExit(360000)) {
      throw 'Native phase did not finish within 360 seconds after the coordinated transition'
    }
    if ($phaseProcess.ExitCode -ne 0) {
      throw "pnpm test:native exited with code $($phaseProcess.ExitCode)"
    }
  } catch {
    $phaseFailure = $_
  } finally {
    if (-not $phaseProcess.HasExited) {
      try { $phaseProcess.Kill($true) }
      catch { Write-Warning "Could not stop the coordinated native phase: $($_.Exception.Message)" }
    }
    if (-not $phaseProcess.HasExited) { $null = $phaseProcess.WaitForExit(10000) }
    if ($phaseProcess.HasExited) {
      try { Write-CapturedProcessTranscript $stdoutTask $stderrTask $phaseLogPath }
      catch { $transcriptFailure = $_.Exception.Message }
    } else {
      $transcriptFailure = 'The coordinated native phase did not exit, so its output could not be collected'
    }
    $phaseProcess.Dispose()
  }
  if ($transcriptFailure) {
    if (-not $phaseFailure) { throw $transcriptFailure }
    Write-Warning "Could not collect coordinated native phase output: $transcriptFailure"
  }
  if ($phaseFailure) {
    throw "$($phaseFailure.Exception.Message)$([Environment]::NewLine)$(Get-SafeLogTail $phaseLogPath)"
  }
}

function Remove-E2eCredentialNamespace {
  param([Parameter(Mandatory)] [string]$Namespace)
  if ($Namespace -notmatch '^[a-f0-9]{12}-[a-z][a-z0-9-]{0,31}$') {
    throw 'Invalid native E2E credential namespace'
  }
  $prefix = "com.camellia.nexus.licensing.E2E.$Namespace/"
  $pattern = [regex]::Escape($prefix) + '[A-Za-z0-9-]+'
  $listing = Invoke-CapturedProcess cmdkey.exe @('/list')
  $targets = @(
    [regex]::Matches($listing, $pattern) |
      ForEach-Object { $_.Value } |
      Sort-Object -Unique
  )
  foreach ($target in $targets) {
    try { Invoke-CapturedProcess cmdkey.exe @("/delete:$target") | Out-Null }
    catch {
      Write-Warning "Could not remove native E2E credential $target`: $($_.Exception.Message)"
    }
  }
  $remaining = Invoke-CapturedProcess cmdkey.exe @('/list')
  if ($remaining -match $pattern) {
    throw "Native E2E credentials remain in isolated namespace $Namespace"
  }
}

function Invoke-NativePhase {
  param(
    [Parameter(Mandatory)] [string]$Phase,
    [Parameter(Mandatory)] [string]$Identity,
    [Parameter(Mandatory)] [string]$DataRoot,
    [Parameter(Mandatory)] [string]$RunDirectory,
    [Parameter(Mandatory)] [string]$RunId,
    [string]$ReadySignal,
    [string]$AppliedSignal,
    [scriptblock]$OnReady,
    [switch]$ResetIdentity
  )
  $coordinated = [bool]$OnReady
  if ($coordinated -ne [bool]($ReadySignal -and $AppliedSignal)) {
    throw 'A coordinated native phase requires ready/applied signals and an action'
  }
  $identityDirectory = Initialize-E2eIdentity $Identity $DataRoot
  $outputPhase = if ($Phase -eq 'cleanup') { "cleanup-$Identity" } else { $Phase }
  $phaseEnvironment = @{
    CAMELLIA_NEXUS_E2E_NAMESPACE = "$RunId-$Identity"
    CAMELLIA_NEXUS_E2E_DATA_DIR = $identityDirectory
    CAMELLIA_NEXUS_E2E_PHASE = $Phase
    CAMELLIA_NEXUS_E2E_OUTPUT_DIR = Join-Path $RunDirectory "wdio/$outputPhase"
    CAMELLIA_NEXUS_E2E_WEBDRIVER_PORT = $(Get-FreeTcpPort)
    CAMELLIA_NEXUS_E2E_RESET_IDENTITY = $(if ($ResetIdentity) { 'true' } else { 'false' })
  }
  $previous = Set-ProcessEnvironment $phaseEnvironment
  $phaseFailure = $null
  try {
    Write-Host "==> Native phase $Phase ($Identity)"
    if ($coordinated) {
      Invoke-CoordinatedNativePhase $ReadySignal $AppliedSignal $OnReady
    } else {
      Push-Location $UiDirectory
      try { Invoke-Checked pnpm @('test:native') }
      finally { Pop-Location }
    }
  } catch {
    $phaseFailure = $_
  } finally {
    Restore-ProcessEnvironment $previous
    if ($ResetIdentity) {
      try { Remove-E2eCredentialNamespace "$RunId-$Identity" }
      catch {
        if ($phaseFailure) {
          Write-Warning "Could not scrub native E2E credentials after phase failure: $($_.Exception.Message)"
        } else {
          throw
        }
      }
    }
  }
  if ($phaseFailure) { throw $phaseFailure }
}

Assert-Toolchain
if ($Action -eq 'Doctor') {
  Write-Host "Native E2E prerequisites are ready (PowerShell $($PSVersionTable.PSVersion), Node $ExpectedNode, pnpm $ExpectedPnpm, Rust $ExpectedRust)."
  exit 0
}

$runId = [guid]::NewGuid().ToString('N').Substring(0, 12)
$outputRoot = if ($OutputDirectory) {
  [System.IO.Path]::GetFullPath($OutputDirectory)
} elseif ($env:RUNNER_TEMP) {
  Join-Path $env:RUNNER_TEMP 'camellia-native-e2e'
} else {
  Join-Path $RepositoryRoot 'e2e-output'
}
$runDirectory = Join-Path $outputRoot $runId
New-Item -ItemType Directory -Path $runDirectory -Force | Out-Null
$environment = $null
$previousEnvironment = $null
$handoffDirectory = $null

try {
  Push-Location $UiDirectory
  try {
    Invoke-Checked pnpm @('install', '--frozen-lockfile')
    Invoke-Checked pnpm @('exec', 'node', 'scripts/test-native-driver.mjs')
  } finally {
    Pop-Location
  }

  switch ($Provider) {
    WslBundle {
      if (-not $BundlePath) { throw 'BundlePath is required for the WslBundle provider' }
      $environment = New-WslBundleEnvironment $BundlePath $runId $runDirectory
    }
    Wsl2Compose {
      $environment = New-WslComposeEnvironment $runDirectory
    }
    SshCompose {
      $environment = New-SshComposeEnvironment $runDirectory
    }
    Existing {
      if ($Suite -ne 'smoke') {
        throw 'The Existing provider supports smoke only because it cannot safely control external state'
      }
      foreach ($required in @{
          ServerBaseUrl = $ServerBaseUrl
          EntitlementAuthorityPath = $EntitlementAuthorityPath
          ProCode = $ProCode
          TeamCode = $TeamCode
        }.GetEnumerator()) {
        if (-not $required.Value) { throw "$($required.Key) is required for the Existing provider" }
      }
      $environment = @{
        BaseUrl = ([uri]$ServerBaseUrl).AbsoluteUri.TrimEnd('/')
        AuthorityPath = (Resolve-Path $EntitlementAuthorityPath).Path
        FreeAccountId = 'existing-free-account'
        FreeLicenseId = 'existing-free-license'
        FreePrimaryCode = $ProCode
        FreeSecondDeviceCode = $ProCode
        ProAccountId = 'existing-pro-account'
        ProLicenseId = 'existing-pro-license'
        ProPrimaryCode = $ProCode
        ProRecoveryCode = $ProCode
        BillingOfferId = 'existing-offer'
        BillingPaymentMethodId = 'existing-method'
        BillingInvoiceId = 'existing-invoice'
        TeamAccountId = 'existing-team-account'
        TeamLicenseId = 'existing-team-license'
        TeamOwnerCode = $TeamCode
        TeamMemberCode = $TeamCode
        TeamAdditionalDeviceCode = $TeamCode
      }
      Wait-HttpReady "$($environment.BaseUrl)/readyz"
    }
  }

  if ($env:GITHUB_ACTIONS -eq 'true') {
    foreach ($code in @(
        $environment.FreePrimaryCode,
        $environment.FreeSecondDeviceCode,
        $environment.ProPrimaryCode,
        $environment.ProRecoveryCode,
        $environment.TeamOwnerCode,
        $environment.TeamMemberCode,
        $environment.TeamAdditionalDeviceCode
      )) {
      Write-Output "::add-mask::$code"
    }
  }

  $dataRoot = Join-Path $runDirectory 'client-data'
  $fixtureDirectory = Join-Path $runDirectory 'fixture'
  $handoffDirectory = Join-Path ([System.IO.Path]::GetTempPath()) "camellia-nexus-e2e-handoff-$runId"
  New-Item -ItemType Directory -Path $dataRoot, $fixtureDirectory, $handoffDirectory | Out-Null
  $handoffSecurity = [System.Security.AccessControl.DirectorySecurity]::new()
  $handoffOwner = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
  $handoffSecurity.SetOwner($handoffOwner)
  $handoffSecurity.SetAccessRuleProtection($true, $false)
  $handoffSecurity.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
      $handoffOwner,
      [System.Security.AccessControl.FileSystemRights]::FullControl,
      [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit',
      [System.Security.AccessControl.PropagationFlags]::None,
      [System.Security.AccessControl.AccessControlType]::Allow
    ))
  Set-Acl -LiteralPath $handoffDirectory -AclObject $handoffSecurity
  $fixtureScript = Join-Path $fixtureDirectory 'long-running.ps1'
  [System.IO.File]::WriteAllText(
    $fixtureScript,
    'Write-Output ''native-e2e-ready''; while ($true) { Start-Sleep -Milliseconds 200 }',
    [System.Text.UTF8Encoding]::new($false)
  )
  $fixtureExecutable = (Get-Command pwsh.exe -ErrorAction Stop).Source
  $fixtureWorkingDirectory = Split-Path -Parent $fixtureExecutable
  $appBinary = Join-Path $RepositoryRoot 'target/debug/camellia-nexus.exe'

  $values = @{
    CAMELLIA_NEXUS_LICENSE_URL = $environment.BaseUrl
    CAMELLIA_NEXUS_AUTHORIZATION_ENDPOINT = 'https://native-e2e.invalid/oauth/authorize'
    CAMELLIA_NEXUS_OAUTH_CLIENT_ID = 'camellia-nexus-desktop'
    CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH = $environment.AuthorityPath
    CAMELLIA_NEXUS_E2E_SERVER_BASE_URL = $environment.BaseUrl
    CAMELLIA_NEXUS_E2E_APP_BINARY = $appBinary
    CAMELLIA_NEXUS_E2E_SUITE = $Suite
    CAMELLIA_NEXUS_E2E_HANDOFF_DIR = $handoffDirectory
    CAMELLIA_NEXUS_E2E_FREE_ACCOUNT_ID = $environment.FreeAccountId
    CAMELLIA_NEXUS_E2E_FREE_LICENSE_ID = $environment.FreeLicenseId
    CAMELLIA_NEXUS_E2E_FREE_PRIMARY_CODE = $environment.FreePrimaryCode
    CAMELLIA_NEXUS_E2E_FREE_SECOND_DEVICE_CODE = $environment.FreeSecondDeviceCode
    CAMELLIA_NEXUS_E2E_PRO_ACCOUNT_ID = $environment.ProAccountId
    CAMELLIA_NEXUS_E2E_PRO_LICENSE_ID = $environment.ProLicenseId
    CAMELLIA_NEXUS_E2E_PRO_PRIMARY_CODE = $environment.ProPrimaryCode
    CAMELLIA_NEXUS_E2E_PRO_RECOVERY_CODE = $environment.ProRecoveryCode
    CAMELLIA_NEXUS_E2E_BILLING_OFFER_ID = $environment.BillingOfferId
    CAMELLIA_NEXUS_E2E_BILLING_PAYMENT_METHOD_ID = $environment.BillingPaymentMethodId
    CAMELLIA_NEXUS_E2E_BILLING_INVOICE_ID = $environment.BillingInvoiceId
    CAMELLIA_NEXUS_E2E_TEAM_ACCOUNT_ID = $environment.TeamAccountId
    CAMELLIA_NEXUS_E2E_TEAM_LICENSE_ID = $environment.TeamLicenseId
    CAMELLIA_NEXUS_E2E_TEAM_OWNER_CODE = $environment.TeamOwnerCode
    CAMELLIA_NEXUS_E2E_TEAM_MEMBER_CODE = $environment.TeamMemberCode
    CAMELLIA_NEXUS_E2E_TEAM_ADDITIONAL_DEVICE_CODE = $environment.TeamAdditionalDeviceCode
    CAMELLIA_NEXUS_E2E_FIXTURE_EXECUTABLE = $fixtureExecutable
    CAMELLIA_NEXUS_E2E_FIXTURE_SCRIPT = $fixtureScript
    CAMELLIA_NEXUS_E2E_FIXTURE_WORKING_DIRECTORY = $fixtureWorkingDirectory
    RUST_LOG = 'camellia_nexus=trace,camellia_nexus_licensing=trace,camellia_nexus_core=trace'
  }
  $previousEnvironment = Set-ProcessEnvironment $values

  Push-Location $UiDirectory
  try {
    if (-not $SkipBuild) {
      Invoke-Checked pnpm @('desktop:build:e2e')
    }
    if (-not (Test-Path -LiteralPath $appBinary -PathType Leaf)) {
      throw "Native E2E binary is missing: $appBinary"
    }
    $nativeSucceeded = $false
    try {
      if ($Suite -eq 'full') {
        Invoke-NativePhase 'free-activation-limits' 'free-primary' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'free-device-limit' 'free-secondary' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'free-primary-release' 'free-primary' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'free-device-recovery' 'free-secondary' $dataRoot $runDirectory $runId
      }
      Invoke-NativePhase 'smoke-activation' 'pro' $dataRoot $runDirectory $runId
      Invoke-NativePhase 'smoke-persistence' 'pro' $dataRoot $runDirectory $runId
      if ($Suite -eq 'full') {
        try {
          Set-E2eServerAvailability $environment $false
          Invoke-NativePhase 'full-offline' 'pro' $dataRoot $runDirectory $runId
        } finally {
          Set-E2eServerAvailability $environment $true
        }
        Invoke-NativePhase 'full-recovery-billing' 'pro' $dataRoot $runDirectory $runId
        try {
          Invoke-NativePhase 'full-terminal-denial' 'pro' $dataRoot $runDirectory $runId `
            -ReadySignal 'terminal-denial-ready.json' `
            -AppliedSignal 'terminal-denial-applied.json' `
            -OnReady { Set-E2eAccountState $environment $environment.ProAccountId suspended }
        } finally {
          Set-E2eAccountState $environment $environment.ProAccountId active
        }
        Invoke-NativePhase 'full-restoration' 'pro' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-owner-activation' 'team-owner' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-member-join' 'team-member' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-owner-workspace' 'team-owner' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-additional-device' 'team-additional' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-former-owner-leave' 'team-owner' $dataRoot $runDirectory $runId
        Invoke-NativePhase 'team-new-owner' 'team-member' $dataRoot $runDirectory $runId
      }
      $nativeSucceeded = $true
    } finally {
      $cleanupFailures = [System.Collections.Generic.List[string]]::new()
      foreach ($identity in @(
          'free-primary',
          'free-secondary',
          'pro',
          'team-owner',
          'team-member',
          'team-additional'
        )) {
        if (Test-Path -LiteralPath (Join-Path $dataRoot $identity) -PathType Container) {
          try {
            Invoke-NativePhase 'cleanup' $identity $dataRoot $runDirectory $runId -ResetIdentity
          } catch {
            $message = "Could not clean native E2E identity $identity`: $($_.Exception.Message)"
            if ($nativeSucceeded) {
              $cleanupFailures.Add($message)
            } else {
              Write-Warning $message
            }
          }
        }
      }
      if ($nativeSucceeded -and $cleanupFailures.Count -gt 0) {
        throw ($cleanupFailures -join [Environment]::NewLine)
      }
    }
  } finally {
    Pop-Location
  }
  Write-Host "Native $Suite E2E passed. Diagnostics: $runDirectory"
} finally {
  if ($previousEnvironment) { Restore-ProcessEnvironment $previousEnvironment }
  if ($handoffDirectory -and (Test-Path -LiteralPath $handoffDirectory -PathType Container)) {
    Remove-Item -LiteralPath $handoffDirectory -Recurse -Force
  }
  if ($environment -and -not $KeepEnvironment) {
    if ($Provider -eq 'WslBundle') {
      Remove-WslBundleEnvironment $environment $runDirectory
    } elseif ($Provider -in @('Wsl2Compose', 'SshCompose')) {
      Remove-ComposeEnvironment $environment $runDirectory
    }
  } elseif ($environment -and $KeepEnvironment -and $Provider -eq 'Wsl2Compose') {
    Write-Host "WSL2 Compose environment retained: $($environment.EnvironmentId)"
  }
}
