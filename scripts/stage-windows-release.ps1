#requires -Version 7.6

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9.-]{0,127}$')]
  [string]$BuildId,

  [ValidateSet('windows')]
  [string]$Platform = 'windows',

  [Parameter(Mandatory = $true)]
  [ValidateSet('x64', 'arm64')]
  [string]$Architecture,

  [Parameter(Mandatory = $true)]
  [ValidatePattern('^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$')]
  [string]$Version,

  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[0-9a-f]{40}$')]
  [string]$Commit,

  [ValidateSet('unsigned', 'signed')]
  [string]$NativeSigning = 'unsigned',

  [string]$SigningPfxPath = $env:CAMELLIA_NEXUS_SIGN_PFX,
  [Security.SecureString]$SigningPfxPassword,
  [string]$RunnerArchitecture = $env:RUNNER_ARCH,
  [string]$TargetDirectory = 'target/release',
  [string]$OutputDirectory = 'dist-artifacts',
  [string]$MetadataDirectory = 'build-metadata'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$expectedRunnerArchitecture = switch ($Architecture) {
  'x64' { 'X64' }
  'arm64' { 'ARM64' }
}
if ($RunnerArchitecture -ne $expectedRunnerArchitecture) {
  throw "Unsupported Windows release architecture: runner=$RunnerArchitecture expected=$Architecture"
}

$executable = Join-Path $TargetDirectory 'camellia-nexus.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
  throw "Windows executable was not produced: $executable"
}
$broker = Join-Path $TargetDirectory 'camellia-nexus-privilege-broker.exe'
if (-not (Test-Path -LiteralPath $broker -PathType Leaf)) {
  throw "Windows privilege broker was not produced: $broker"
}

$bundleDirectory = Join-Path $TargetDirectory 'bundle'
if (-not (Test-Path -LiteralPath $bundleDirectory -PathType Container)) {
  throw "Windows bundle directory was not produced: $bundleDirectory"
}
$msiPackages = @(Get-ChildItem -LiteralPath $bundleDirectory -File -Recurse | Where-Object { $_.Extension -ieq '.msi' })
if ($msiPackages.Count -ne 1) {
  throw "Expected exactly one Windows MSI package under $bundleDirectory; found $($msiPackages.Count)"
}

$name = "camellia-nexus-$BuildId-$Platform-$Architecture"
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stagedPortable = Join-Path $OutputDirectory "$name-portable.zip"
$stagedMsi = Join-Path $OutputDirectory "$name.msi"
Copy-Item -LiteralPath $msiPackages[0].FullName -Destination $stagedMsi -Force

if ($NativeSigning -eq 'signed' -and $env:OS -ne 'Windows_NT') {
  throw 'Signed Windows release staging requires a Windows host'
}
if ($NativeSigning -eq 'signed') {
  . (Join-Path $PSScriptRoot 'windows-authenticode.ps1')
  if ([string]::IsNullOrWhiteSpace($SigningPfxPath)) {
    throw 'The signing PFX is required to verify signed Windows release components'
  }
  $verificationPassword = if ($SigningPfxPassword) {
    $SigningPfxPassword
  } elseif ($env:CAMELLIA_NEXUS_SIGN_PASSWORD) {
    ConvertTo-SecureString -String $env:CAMELLIA_NEXUS_SIGN_PASSWORD -AsPlainText -Force
  } else {
    throw 'The signing PFX password is required to verify signed Windows release components'
  }
  $verificationContext = $null
  try {
    $verificationContext = Get-WindowsPfxVerificationContext `
      -PfxPath $SigningPfxPath `
      -Password $verificationPassword `
      -TrustEmbeddedRoot
    $signTool = Find-WindowsSignTool
    foreach ($signedFile in @($executable, $broker, $stagedMsi)) {
      Assert-WindowsSignature `
        -File $signedFile `
        -SignTool $signTool `
        -ExpectedThumbprint $verificationContext.Thumbprint `
        -PrivateRoots $verificationContext.PrivateRoots `
        -ChainCertificates $verificationContext.ChainCertificates
    }
  }
  finally {
    Close-WindowsPfxVerificationContext $verificationContext
  }
}

$portableDirectory = Join-Path ([IO.Path]::GetTempPath()) "camellia-portable-$([Guid]::NewGuid().ToString('N'))"
try {
  New-Item -ItemType Directory -Path $portableDirectory -Force | Out-Null
  Copy-Item -LiteralPath $executable -Destination (Join-Path $portableDirectory 'camellia-nexus.exe')
  Copy-Item -LiteralPath $broker -Destination (Join-Path $portableDirectory 'camellia-nexus-privilege-broker.exe')
  Compress-Archive -LiteralPath @(
    (Join-Path $portableDirectory 'camellia-nexus.exe'),
    (Join-Path $portableDirectory 'camellia-nexus-privilege-broker.exe')
  ) -DestinationPath $stagedPortable -CompressionLevel Optimal -Force
}
finally {
  Remove-Item -LiteralPath $portableDirectory -Recurse -Force -ErrorAction SilentlyContinue
}

foreach ($stagedFile in @($stagedPortable, $stagedMsi)) {
  if (-not (Test-Path -LiteralPath $stagedFile -PathType Leaf)) {
    throw "Windows release artifact was not staged: $stagedFile"
  }
}

New-Item -ItemType Directory -Path $MetadataDirectory -Force | Out-Null
$metadata = [ordered]@{
  schemaVersion = 2
  product = 'Camellia Nexus'
  version = $Version
  buildId = $BuildId
  commit = $Commit
  platform = $Platform
  architecture = $Architecture
  nativeSigning = $NativeSigning
  artifactSigning = [ordered]@{ scheme = 'none' }
}
$metadataPath = Join-Path $MetadataDirectory "$Platform-$Architecture.json"
$metadata | ConvertTo-Json | Set-Content -LiteralPath $metadataPath -Encoding utf8NoBOM
