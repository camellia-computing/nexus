#requires -Version 7.6

param(
    [ValidateSet("Quality", "DesktopCheck", "DesktopBuild", "DesktopPackage")]
    [string]$Mode = "Quality",
    [switch]$SkipQuality,
    [switch]$Sign,
    [switch]$TrustEmbeddedRoot,
    [string]$PfxPath = "",
    [Security.SecureString]$PfxPassword,
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$RootDirectory = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$DefaultPfxPath = Join-Path $RootDirectory "Camellia-Computing-Software-CodeSigning.pfx"
. (Join-Path $PSScriptRoot "windows-authenticode.ps1")
if ($env:CAMELLIA_NEXUS_TIMESTAMP_URL -and -not $PSBoundParameters.ContainsKey("TimestampUrl")) {
    $TimestampUrl = $env:CAMELLIA_NEXUS_TIMESTAMP_URL
}

function Assert-Command {
    param([string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command is unavailable: $Name"
    }
}

function Invoke-Step {
    param(
        [string]$Label,
        [scriptblock]$Action
    )
    Write-Host "==> $Label"
    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

function Prepare-PrivilegeBroker {
    param([bool]$DebugBuild)
    $BrokerTarget = (& rustc --print host-tuple).Trim()
    if (-not $BrokerTarget) {
        throw "Rust did not report a host target for the privilege broker"
    }
    $env:TAURI_ENV_TARGET_TRIPLE = $BrokerTarget
    $env:TAURI_ENV_DEBUG = if ($DebugBuild) { "true" } else { "false" }
    Write-Host "==> Prepare the $BrokerTarget privilege broker"
    & node scripts/prepare-privilege-broker.mjs | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "Privilege broker preparation failed with exit code $LASTEXITCODE"
    }
    $env:CAMELLIA_NEXUS_PRIVILEGE_BROKER_PREPARED = "1"
    return $BrokerTarget
}

function Resolve-CodeSigningPassword {
    if ($PfxPassword) {
        return $PfxPassword
    }
    if ($env:CAMELLIA_NEXUS_SIGN_PASSWORD) {
        return ConvertTo-SecureString -String $env:CAMELLIA_NEXUS_SIGN_PASSWORD -AsPlainText -Force
    }
    if ($env:CI) {
        throw "CAMELLIA_NEXUS_SIGN_PASSWORD is required for CI signing"
    }
    return Read-Host "PFX password" -AsSecureString
}

function Initialize-CodeSigning {
    param([switch]$CreateTauriConfig)
    if ($env:OS -ne "Windows_NT") {
        throw "PFX Authenticode signing is only supported by this script on Windows"
    }
    $ResolvedPfx = if ($PfxPath) {
        $PfxPath
    } elseif ($env:CAMELLIA_NEXUS_SIGN_PFX) {
        $env:CAMELLIA_NEXUS_SIGN_PFX
    } else {
        $DefaultPfxPath
    }
    if (-not (Test-Path -LiteralPath $ResolvedPfx -PathType Leaf)) {
        throw "Code-signing PFX was not found: $ResolvedPfx"
    }
    $SignTool = Find-WindowsSignTool
    $ResolvedPfx = (Resolve-Path -LiteralPath $ResolvedPfx).Path
    $Password = Resolve-CodeSigningPassword
    $PfxVerificationContext = $null
    $ImportedStorePaths = @()
    $ConfigPath = $null
    try {
        Write-Host "Loading Authenticode certificate"
        $PfxVerificationContext = Get-WindowsPfxVerificationContext `
            -PfxPath $ResolvedPfx `
            -Password $Password `
            -TrustEmbeddedRoot:$TrustEmbeddedRoot
        $Certificate = $PfxVerificationContext.SignerCertificate
        $StorePath = "Cert:\CurrentUser\My\$($Certificate.Thumbprint)"
        $Existing = Get-Item -LiteralPath $StorePath -ErrorAction SilentlyContinue
        if ($Existing -and -not $Existing.HasPrivateKey) {
            throw "A certificate with the same thumbprint exists without its private key"
        }
        $RemoveAfterBuild = -not [bool]$Existing
        if ($RemoveAfterBuild) {
            $ImportedStorePaths = @(
                @($PfxVerificationContext.Certificates) |
                    Select-Object -ExpandProperty Thumbprint -Unique |
                    ForEach-Object { "Cert:\CurrentUser\My\$_" } |
                    Where-Object { -not (Test-Path -LiteralPath $_) }
            )
            Write-Host "Importing Authenticode certificate into the current-user store"
            $Imported = Import-PfxCertificate `
                -FilePath $ResolvedPfx `
                -CertStoreLocation "Cert:\CurrentUser\My" `
                -Password $Password `
                -Exportable:$false
            if (-not $Imported) {
                throw "The code-signing certificate could not be imported"
            }
        }
        $Installed = Get-Item -LiteralPath $StorePath -ErrorAction Stop
        if (-not $Installed.HasPrivateKey) {
            throw "The imported code-signing certificate has no accessible private key"
        }
        Write-Host "Preparing isolated Authenticode trust chain"

        if ($CreateTauriConfig) {
            $ConfigDirectory = Join-Path $RootDirectory "target\codesign"
            New-Item -ItemType Directory -Path $ConfigDirectory -Force | Out-Null
            $ConfigPath = Join-Path $ConfigDirectory "tauri-signing.json"
            $SigningConfig = @{
                bundle = @{
                    externalBin = @("binaries/camellia-nexus-privilege-broker")
                    windows = @{
                        certificateThumbprint = $Certificate.Thumbprint
                        digestAlgorithm = "sha256"
                        timestampUrl = $TimestampUrl
                        tsp = $true
                    }
                }
            }
            $ConfigJson = $SigningConfig | ConvertTo-Json -Depth 4
            [IO.File]::WriteAllText($ConfigPath, $ConfigJson, [Text.UTF8Encoding]::new($false))
        }

        return [PSCustomObject]@{
            Thumbprint = $Certificate.Thumbprint
            StorePath = $StorePath
            ImportedStorePaths = $ImportedStorePaths
            ConfigPath = $ConfigPath
            SignTool = $SignTool
            TimestampUrl = $TimestampUrl
            PrivateRoots = @($PfxVerificationContext.PrivateRoots)
            ChainCertificates = @($PfxVerificationContext.ChainCertificates)
            PfxVerificationContext = $PfxVerificationContext
        }
    }
    catch {
        if ($ConfigPath) {
            Remove-Item -LiteralPath $ConfigPath -Force -ErrorAction SilentlyContinue
        }
        foreach ($Path in $ImportedStorePaths) {
            Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
        }
        Close-WindowsPfxVerificationContext $PfxVerificationContext
        throw
    }
}

function Remove-CodeSigningContext {
    param($Context)
    if (-not $Context) {
        return
    }
    if ($Context.ConfigPath) {
        Remove-Item -LiteralPath $Context.ConfigPath -Force -ErrorAction SilentlyContinue
    }
    foreach ($Path in $Context.ImportedStorePaths) {
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
    Close-WindowsPfxVerificationContext $Context.PfxVerificationContext
}

function Invoke-WindowsSignature {
    param(
        [string]$File,
        [string]$SignTool,
        [string]$Thumbprint,
        [string]$TimestampUrl
    )
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) {
        throw "Windows executable was not produced: $File"
    }
    $Attempts = 3
    for ($Attempt = 1; $Attempt -le $Attempts; $Attempt++) {
        & $SignTool sign `
            /fd SHA256 `
            /sha1 $Thumbprint `
            /tr $TimestampUrl `
            /td SHA256 `
            /d "Camellia Nexus" `
            $File
        if ($LASTEXITCODE -eq 0) {
            Write-Host "Signed Windows executable: $File"
            return
        }
        if ($Attempt -lt $Attempts) {
            Write-Warning "Authenticode signing attempt $Attempt failed; retrying"
            Start-Sleep -Seconds ([Math]::Pow(2, $Attempt))
        }
    }
    throw "Authenticode signing failed after $Attempts attempts: $File"
}

function Complete-WindowsExecutableSignature {
    param(
        [string]$File,
        [string]$SignTool,
        [string]$Thumbprint,
        [string]$TimestampUrl
    )
    $EmbeddedSignature = Get-WindowsEmbeddedSignature -File $File
    try {
        if ($EmbeddedSignature.SignerCertificate) {
            $ActualThumbprint = $EmbeddedSignature.SignerCertificate.Thumbprint
            if ($ActualThumbprint -ne $Thumbprint) {
                throw "The final Windows executable has an unexpected embedded signer: $File"
            }
            if (
                $EmbeddedSignature.TimestampCount -ne 1 -or
                -not $EmbeddedSignature.TimestampCertificate
            ) {
                throw "The final Windows executable does not have exactly one RFC 3161 timestamp: $File"
            }
            Write-Host "Final Windows executable is already signed at its byte boundary: $File"
            return
        }
        if ($EmbeddedSignature.Status -ne [CamelliaNexus.Build.WinTrust]::NoSignature) {
            $Status = [CamelliaNexus.Build.WinTrust]::FormatStatus($EmbeddedSignature.Status)
            throw "The final Windows executable has an invalid embedded-signature state ${Status}: $File"
        }
    }
    finally {
        $EmbeddedSignature.Dispose()
    }
    Invoke-WindowsSignature `
        -File $File `
        -SignTool $SignTool `
        -Thumbprint $Thumbprint `
        -TimestampUrl $TimestampUrl
}

function Assert-WindowsStartupLinkage {
    param([string]$Executable)
    if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
        throw "Windows executable was not produced: $Executable"
    }
    $Bytes = [System.IO.File]::ReadAllBytes($Executable)
    if ($Bytes.Length -lt 2 -or $Bytes[0] -ne 0x4D -or $Bytes[1] -ne 0x5A) {
        throw "Build output is not a Windows PE executable: $Executable"
    }
    $Printable = [System.Text.Encoding]::ASCII.GetString($Bytes)
    $Markers = @(
        "asInvoker",
        "WinVerifyTrust",
        "Camellia Computing",
        "ShellExecuteW"
    )
    foreach ($Marker in $Markers) {
        if (-not $Printable.Contains($Marker)) {
            throw "Windows executable is missing required startup linkage marker: $Marker"
        }
    }
    $ForbiddenMarkers = @(
        "--startup-bridge",
        "Failed to create the elevated startup task",
        "schtasks.exe",
        "runas"
    )
    foreach ($Marker in $ForbiddenMarkers) {
        if ($Printable.Contains($Marker)) {
            throw "Windows executable contains forbidden whole-application elevation marker: $Marker"
        }
    }
    Write-Host "Verified normal-user Windows startup linkage: $Executable"
    $Hash = Get-FileHash -Algorithm SHA256 -LiteralPath $Executable
    Write-Host "SHA256 $($Hash.Hash.ToLowerInvariant())"
}

Assert-Command "cargo"
Assert-Command "node"
Assert-Command "pnpm"

Push-Location $RootDirectory
$SigningContext = $null
try {
    Invoke-Step "Install locked frontend dependencies" {
        pnpm --dir ui install --frozen-lockfile
    }

    if (-not $SkipQuality) {
        Invoke-Step "Audit release authorization boundaries" {
            node scripts/audit-release-security.mjs
        }
        Invoke-Step "Validate embedded entitlement trust" {
            node scripts/validate-entitlement-keys.mjs
        }
        Invoke-Step "Validate native E2E isolation" {
            ./scripts/test-e2e-native.ps1
        }
        Invoke-Step "Check Rust formatting" {
            cargo fmt --all -- --check
        }
        Invoke-Step "Lint platform-independent Rust targets" {
            cargo clippy --workspace --locked --no-default-features --all-targets -- -D warnings
        }
        Invoke-Step "Run platform-independent and native process tests" {
            cargo test --workspace --locked --no-default-features
        }
        Invoke-Step "Check Svelte and TypeScript" {
            pnpm --dir ui check
        }
        Invoke-Step "Test frontend utilities" {
            pnpm --dir ui test
        }
        Invoke-Step "Build frontend" {
            pnpm --dir ui build
        }
    }

    if ($Mode -eq "DesktopCheck") {
        Prepare-PrivilegeBroker -DebugBuild $true | Out-Null
        Invoke-Step "Check the native desktop target" {
            cargo check --locked -p camellia-nexus
        }
    }
    elseif ($Mode -in @("DesktopBuild", "DesktopPackage")) {
        $BrokerTarget = Prepare-PrivilegeBroker -DebugBuild $false
        if ($Sign) {
            Write-Host "==> Prepare Authenticode signing"
            $SigningContext = Initialize-CodeSigning -CreateTauriConfig:($Mode -eq "DesktopPackage")
            $BrokerFiles = @(
                (Join-Path $RootDirectory "src-tauri\binaries\camellia-nexus-privilege-broker-$BrokerTarget.exe"),
                (Join-Path $RootDirectory "target\release\camellia-nexus-privilege-broker.exe")
            )
            foreach ($BrokerFile in $BrokerFiles) {
                Invoke-WindowsSignature `
                    -File $BrokerFile `
                    -SignTool $SigningContext.SignTool `
                    -Thumbprint $SigningContext.Thumbprint `
                    -TimestampUrl $SigningContext.TimestampUrl
            }
        }
        $Arguments = @("ui/node_modules/@tauri-apps/cli/tauri.js", "build", "--ci")
        if ($Mode -eq "DesktopBuild") {
            $Arguments += "--no-bundle"
        }
        elseif ($env:CAMELLIA_NEXUS_TAURI_BUNDLES) {
            $Arguments += @("--bundles", $env:CAMELLIA_NEXUS_TAURI_BUNDLES)
        }
        if ($Mode -eq "DesktopPackage" -and -not $SigningContext) {
            $Arguments += @("--config", "src-tauri/tauri.privilege-broker.conf.json")
        }
        if ($SigningContext -and $Mode -eq "DesktopPackage") {
            $Arguments += @("--config", $SigningContext.ConfigPath)
        }
        $Arguments += @("--", "--locked")
        if ($env:OS -eq "Windows_NT") {
            $ExpectedExecutable = Join-Path $RootDirectory "target\release\camellia-nexus.exe"
            Remove-Item -LiteralPath $ExpectedExecutable -Force -ErrorAction SilentlyContinue
        }
        Invoke-Step $(if ($Mode -eq "DesktopPackage") { "Build desktop packages" } else { "Build the native release executable" }) {
            node @Arguments
        }

        if ($env:OS -eq "Windows_NT") {
            $Executable = Join-Path $RootDirectory "target\release\camellia-nexus.exe"
            # Tauri signs the patched application embedded in each Windows bundle, then restores
            # the original standalone executable. Complete that final portable byte boundary only
            # when it is explicitly unsigned. If a future Tauri version preserves the correct
            # signature, the inspection above prevents appending a second signature or timestamp.
            if ($SigningContext) {
                Complete-WindowsExecutableSignature `
                    -File $Executable `
                    -SignTool $SigningContext.SignTool `
                    -Thumbprint $SigningContext.Thumbprint `
                    -TimestampUrl $SigningContext.TimestampUrl
            }
            Assert-WindowsStartupLinkage $Executable
            if ($SigningContext) {
                $SignedFiles = @(
                    $Executable,
                    (Join-Path $RootDirectory "target\release\camellia-nexus-privilege-broker.exe"),
                    (Join-Path $RootDirectory "src-tauri\binaries\camellia-nexus-privilege-broker-$BrokerTarget.exe")
                )
                if ($Mode -eq "DesktopPackage") {
                    $BundleDirectory = Join-Path $RootDirectory "target\release\bundle"
                    if (Test-Path -LiteralPath $BundleDirectory) {
                        $SignedFiles += Get-ChildItem -LiteralPath $BundleDirectory -File -Recurse |
                            Where-Object { $_.Extension -eq ".msi" } |
                            Select-Object -ExpandProperty FullName
                    }
                }
                foreach ($File in $SignedFiles | Select-Object -Unique) {
                    Assert-WindowsSignature `
                        -File $File `
                        -SignTool $SigningContext.SignTool `
                        -ExpectedThumbprint $SigningContext.Thumbprint `
                        -PrivateRoots $SigningContext.PrivateRoots `
                        -ChainCertificates $SigningContext.ChainCertificates
                }
            }
        }
    }

    Write-Host "Camellia Nexus local CI completed successfully."
}
finally {
    Remove-Item Env:CAMELLIA_NEXUS_PRIVILEGE_BROKER_PREPARED -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_ENV_TARGET_TRIPLE -ErrorAction SilentlyContinue
    Remove-Item Env:TAURI_ENV_DEBUG -ErrorAction SilentlyContinue
    Remove-CodeSigningContext $SigningContext
    Pop-Location
}
