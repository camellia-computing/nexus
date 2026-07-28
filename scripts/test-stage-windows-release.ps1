#requires -Version 7.6

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$scriptPath = Join-Path $PSScriptRoot 'stage-windows-release.ps1'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "camellia-stage-$([Guid]::NewGuid().ToString('N'))"
$target = Join-Path $testRoot 'target/release'
$bundle = Join-Path $target 'bundle/msi'
$output = Join-Path $testRoot 'output'

try {
  New-Item -ItemType Directory -Path $bundle -Force | Out-Null
  [IO.File]::WriteAllText((Join-Path $target 'camellia-nexus.exe'), 'exe')
  [IO.File]::WriteAllText((Join-Path $target 'camellia-nexus-privilege-broker.exe'), 'broker')
  [IO.File]::WriteAllText((Join-Path $bundle 'Camellia Nexus.msi'), 'msi')

  & $scriptPath -BuildId '1.2.3' -Architecture x64 -RunnerArchitecture X64 `
    -Version '1.2.3' -Commit ('a' * 40) `
    -TargetDirectory $target -OutputDirectory $output -MetadataDirectory (Join-Path $testRoot 'metadata')
  foreach ($name in @('camellia-nexus-1.2.3-windows-x64-portable.zip', 'camellia-nexus-1.2.3-windows-x64.msi')) {
    if (-not (Test-Path -LiteralPath (Join-Path $output $name) -PathType Leaf)) {
      throw "Expected staged fixture is missing: $name"
    }
  }
  if (@(Get-ChildItem -LiteralPath $output -File).Count -ne 2) {
    throw 'Windows staging produced an unexpected number of artifacts'
  }
  $portableExtract = Join-Path $testRoot 'portable'
  Expand-Archive -LiteralPath (Join-Path $output 'camellia-nexus-1.2.3-windows-x64-portable.zip') `
    -DestinationPath $portableExtract
  $portableFiles = @(Get-ChildItem -LiteralPath $portableExtract -File | Select-Object -ExpandProperty Name | Sort-Object)
  if (($portableFiles -join ',') -ne 'camellia-nexus-privilege-broker.exe,camellia-nexus.exe') {
    throw "Portable package does not contain the exact application/broker pair: $($portableFiles -join ',')"
  }
  $metadata = Get-Content -LiteralPath (Join-Path $testRoot 'metadata/windows-x64.json') -Raw | ConvertFrom-Json
  if ($metadata.schemaVersion -ne 3 -or
      $metadata.version -ne '1.2.3' -or
      $metadata.nativeSigning -ne 'unsigned' -or
      $metadata.distributionTrust -ne 'none' -or
      $null -ne $metadata.identity -or
      $metadata.artifactSigning.scheme -ne 'none' -or
      $metadata.artifactSigning.trust -ne 'none' -or
      $metadata.delivery -ne 'installable' -or
      $metadata.commit -ne ('a' * 40)) {
    throw 'Windows build metadata does not match the staged fixture'
  }

  $originalOs = $env:OS
  $signedWithoutIdentityRejected = $false
  try {
    $env:OS = 'Windows_NT'
    try {
      & $scriptPath -BuildId '1.2.3' -Architecture x64 -RunnerArchitecture X64 `
        -Version '1.2.3' -Commit ('a' * 40) -NativeSigning signed `
        -DistributionTrust private-trust -ExpectedSigningThumbprint ('B' * 40) `
        -ExpectedSigningSha256 ('C' * 64) `
        -SigningPfxPath (Join-Path $testRoot 'missing-signing-identity.pfx') `
        -SigningPfxPassword (ConvertTo-SecureString 'test-only' -AsPlainText -Force) `
        -TargetDirectory $target -OutputDirectory $output -MetadataDirectory (Join-Path $testRoot 'metadata')
    }
    catch {
      $signedWithoutIdentityRejected = $_.Exception.Message -like 'Code-signing PFX was not found:*'
    }
  }
  finally {
    $env:OS = $originalOs
  }
  if (-not $signedWithoutIdentityRejected) {
    throw 'Signed Windows staging did not require the exact signing identity'
  }

  $architectureRejected = $false
  try {
    & $scriptPath -BuildId '1.2.3' -Architecture x64 -RunnerArchitecture ARM64 `
      -Version '1.2.3' -Commit ('a' * 40) `
      -TargetDirectory $target -OutputDirectory $output -MetadataDirectory (Join-Path $testRoot 'metadata')
  }
  catch {
    $architectureRejected = $true
  }
  if (-not $architectureRejected) {
    throw 'Mismatched runner architecture was accepted'
  }

  [IO.File]::WriteAllText((Join-Path $bundle 'duplicate.msi'), 'msi')
  $duplicateRejected = $false
  try {
    & $scriptPath -BuildId '1.2.3' -Architecture x64 -RunnerArchitecture X64 `
      -Version '1.2.3' -Commit ('a' * 40) `
      -TargetDirectory $target -OutputDirectory $output -MetadataDirectory (Join-Path $testRoot 'metadata')
  }
  catch {
    $duplicateRejected = $true
  }
  if (-not $duplicateRejected) {
    throw 'Multiple MSI packages were accepted'
  }

  Write-Host 'Windows release staging tests passed'
}
finally {
  Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
