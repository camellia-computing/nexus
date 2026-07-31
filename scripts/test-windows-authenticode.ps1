#requires -Version 7.6

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "Windows Authenticode tests require Windows"
}

. (Join-Path $PSScriptRoot "windows-authenticode.ps1")

function Remove-UserTestCertificate {
    param(
        [Parameter(Mandatory)][string]$Thumbprint,
        [switch]$FailOnError
    )

    foreach ($StoreName in @("My", "CA", "Root")) {
        $StorePath = "Cert:\CurrentUser\$StoreName\$Thumbprint"
        if (-not (Test-Path -LiteralPath $StorePath)) {
            continue
        }
        & certutil.exe -user -delstore $StoreName $Thumbprint | Out-Null
        if ($LASTEXITCODE -ne 0 -or (Test-Path -LiteralPath $StorePath)) {
            $Message = "Could not remove the Authenticode test certificate from CurrentUser\$StoreName"
            if ($FailOnError) {
                throw $Message
            }
            Write-Warning $Message
        }
    }
}

$TemporaryDirectory = Join-Path ([IO.Path]::GetTempPath()) (
    "camellia-nexus-authenticode-" + [Guid]::NewGuid().ToString("N")
)
$Certificate = $null
$CatalogOnlySignature = $null
$UnsignedSignature = $null
$EmbeddedSignature = $null
$TamperedSignature = $null
$PfxRootCertificate = $null
$PfxLeafCertificate = $null
$PfxVerificationContext = $null
try {
    New-Item -ItemType Directory -Path $TemporaryDirectory | Out-Null
    $FailingSignTool = Join-Path $TemporaryDirectory "signtool-failure.cmd"
    [IO.File]::WriteAllText($FailingSignTool, "@echo off`r`nexit /b 7`r`n")
    $global:LASTEXITCODE = 91
    $ProbeExitCode = Invoke-WindowsSignToolVerification `
        -SignTool $FailingSignTool `
        -File (Join-Path $TemporaryDirectory "probe.exe")
    if ($ProbeExitCode -ne 7 -or $LASTEXITCODE -ne 0) {
        throw "SignTool verification did not isolate its expected native failure status"
    }

    $DiscoveryRoot = Join-Path $TemporaryDirectory "sdk-bin"
    $OlderX64 = Join-Path $DiscoveryRoot "10.0.999.0\x64\signtool.exe"
    $LatestX64 = Join-Path $DiscoveryRoot "10.0.1000.0\x64\signtool.exe"
    $NewerArm64 = Join-Path $DiscoveryRoot "10.0.9999.0\arm64\signtool.exe"
    foreach ($Fixture in @($OlderX64, $LatestX64, $NewerArm64)) {
        New-Item -ItemType Directory -Path (Split-Path -Parent $Fixture) -Force | Out-Null
        [IO.File]::WriteAllText($Fixture, "fixture")
    }
    $ResolvedFixture = Resolve-WindowsSignToolFromBinRoots `
        -BinRoots @($DiscoveryRoot) `
        -Architectures @("x64", "arm64")
    if ($ResolvedFixture -ne (Resolve-Path -LiteralPath $LatestX64).Path) {
        throw "Windows SDK discovery did not select the newest compatible architecture"
    }
    $ResolvedVersionRoot = Resolve-WindowsSignToolFromBinRoots `
        -BinRoots @((Split-Path -Parent (Split-Path -Parent $LatestX64))) `
        -Architectures @("x64")
    if ($ResolvedVersionRoot -ne (Resolve-Path -LiteralPath $LatestX64).Path) {
        throw "Windows SDK discovery did not accept a version-specific bin root"
    }

    $SystemDirectory = Join-Path $env:SystemRoot "System32"
    $PreferredNames = @(
        "where.exe",
        "whoami.exe",
        "hostname.exe",
        "choice.exe",
        "find.exe",
        "sort.exe"
    )
    $Candidates = @($PreferredNames | ForEach-Object {
        $Path = Join-Path $SystemDirectory $_
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            Get-Item -LiteralPath $Path
        }
    })
    $Candidates += @(Get-ChildItem -LiteralPath $SystemDirectory -Filter "*.exe" -File |
        Select-Object -First 256)
    $CatalogSource = $null
    foreach ($Candidate in $Candidates | Sort-Object FullName -Unique) {
        $Signature = Get-AuthenticodeSignature -LiteralPath $Candidate.FullName
        if ($Signature.SignatureType -eq "Catalog" -and $Signature.SignerCertificate) {
            $CandidateEmbeddedSignature = Get-WindowsEmbeddedSignature -File $Candidate.FullName
            try {
                if (-not $CandidateEmbeddedSignature.SignerCertificate) {
                    $CatalogSource = $Candidate.FullName
                    break
                }
            }
            finally {
                $CandidateEmbeddedSignature.Dispose()
            }
        }
    }
    if (-not $CatalogSource) {
        throw "No catalog-only Windows executable was available for the regression fixture"
    }

    $CatalogOnlySignature = Get-WindowsEmbeddedSignature -File $CatalogSource
    if ($CatalogOnlySignature.SignerCertificate) {
        throw "The embedded-signature reader accepted a catalog-only signer"
    }

    $Target = Join-Path $TemporaryDirectory "catalog-and-embedded.exe"
    Copy-Item -LiteralPath $CatalogSource -Destination $Target
    $UnsignedSignature = Get-WindowsEmbeddedSignature -File $Target
    if (
        $UnsignedSignature.Status -ne [CamelliaNexus.Build.WinTrust]::NoSignature -or
        $UnsignedSignature.SignerCertificate -or
        $UnsignedSignature.TimestampCount -ne 0 -or
        $UnsignedSignature.TimestampCertificate
    ) {
        throw "The unsigned executable did not report TRUST_E_NOSIGNATURE"
    }
    $Certificate = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject "CN=Camellia Nexus Authenticode CI Test" `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -KeyAlgorithm RSA `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -KeyExportPolicy NonExportable `
        -NotAfter (Get-Date).AddDays(1)

    $SignTool = Find-WindowsSignTool
    & $SignTool sign `
        /fd SHA256 `
        /sha1 $Certificate.Thumbprint `
        /d "Camellia Nexus Authenticode CI Test" `
        $Target
    if ($LASTEXITCODE -ne 0) {
        throw "Could not create the embedded Authenticode regression fixture"
    }

    $CatalogPreferred = Get-AuthenticodeSignature -LiteralPath $Target
    if ($CatalogPreferred.SignatureType -ne "Catalog") {
        throw "Windows no longer preferred the catalog signature in the regression fixture"
    }
    if (-not $CatalogPreferred.SignerCertificate) {
        throw "The preferred catalog signature did not expose its signer"
    }
    if ($CatalogPreferred.SignerCertificate.Thumbprint -eq $Certificate.Thumbprint) {
        throw "The catalog lookup unexpectedly returned the embedded test signer"
    }

    $ExpectedEmbeddedThumbprint = $Certificate.Thumbprint
    Remove-UserTestCertificate -Thumbprint $ExpectedEmbeddedThumbprint -FailOnError
    foreach ($StoreName in @("My", "CA", "Root")) {
        if (Test-Path -LiteralPath "Cert:\CurrentUser\$StoreName\$ExpectedEmbeddedThumbprint") {
            throw "The embedded signer regression certificate remained in CurrentUser\$StoreName"
        }
    }

    $EmbeddedSignature = Get-WindowsEmbeddedSignature -File $Target
    if (-not $EmbeddedSignature.SignerCertificate) {
        throw "The embedded signer was not extracted"
    }
    if ($EmbeddedSignature.SignerCertificate.Thumbprint -ne $ExpectedEmbeddedThumbprint) {
        throw "The embedded signer did not match the test certificate"
    }
    if ($EmbeddedSignature.TimestampCount -ne 0 -or $EmbeddedSignature.TimestampCertificate) {
        throw "The untimestamped regression fixture reported a timestamp"
    }

    $Bytes = [IO.File]::ReadAllBytes($Target)
    if ($Bytes.Length -le 4096) {
        throw "The catalog-signed fixture is unexpectedly small"
    }
    $Bytes[4096] = $Bytes[4096] -bxor 1
    [IO.File]::WriteAllBytes($Target, $Bytes)
    $TamperedSignature = Get-WindowsEmbeddedSignature -File $Target
    if ($TamperedSignature.Status -ne [CamelliaNexus.Build.WinTrust]::BadDigest) {
        $Status = [CamelliaNexus.Build.WinTrust]::FormatStatus($TamperedSignature.Status)
        throw "Tampered embedded signature returned $Status instead of TRUST_E_BAD_DIGEST"
    }

    $PfxFixtureId = [Guid]::NewGuid().ToString("N")
    $PfxRootCertificate = New-SelfSignedCertificate `
        -Type Custom `
        -Subject "CN=Camellia Nexus PFX Context Root $PfxFixtureId" `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -KeyAlgorithm RSA `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -KeyExportPolicy Exportable `
        -KeyUsage CertSign, CRLSign, DigitalSignature `
        -TextExtension @("2.5.29.19={critical}{text}ca=true&pathlength=0") `
        -NotAfter (Get-Date).AddDays(2)
    $PfxRootPublicPath = Join-Path $TemporaryDirectory "pfx-context-root.cer"
    Export-Certificate -Cert $PfxRootCertificate -FilePath $PfxRootPublicPath | Out-Null
    $PfxLeafCertificate = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject "CN=Camellia Nexus PFX Context Leaf $PfxFixtureId" `
        -Signer $PfxRootCertificate `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -KeyAlgorithm RSA `
        -KeyLength 2048 `
        -HashAlgorithm SHA256 `
        -KeyExportPolicy Exportable `
        -NotAfter (Get-Date).AddDays(1)
    $PfxPasswordText = [Guid]::NewGuid().ToString("N")
    $PfxPassword = ConvertTo-SecureString $PfxPasswordText -AsPlainText -Force
    $PfxLeafOnlyPath = Join-Path $TemporaryDirectory "pfx-context-leaf.pfx"
    Export-PfxCertificate `
        -Cert $PfxLeafCertificate `
        -FilePath $PfxLeafOnlyPath `
        -Password $PfxPassword `
        -ChainOption EndEntityCertOnly | Out-Null
    $PfxFixturePath = Join-Path $TemporaryDirectory "pfx-context.pfx"
    $PfxLeafWithKey = $null
    $PfxPublicRoot = $null
    try {
        $KeyStorageFlags = `
            [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable -bor `
            [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
        $PfxLeafWithKey = [System.Security.Cryptography.X509Certificates.X509CertificateLoader]::LoadPkcs12FromFile(
            $PfxLeafOnlyPath,
            $PfxPasswordText,
            $KeyStorageFlags,
            [System.Security.Cryptography.X509Certificates.Pkcs12LoaderLimits]::Defaults
        )
        $PfxPublicRoot = [System.Security.Cryptography.X509Certificates.X509CertificateLoader]::LoadCertificateFromFile(
            $PfxRootPublicPath
        )
        $PfxCollection = [System.Security.Cryptography.X509Certificates.X509Certificate2Collection]::new()
        $PfxCollection.Add($PfxLeafWithKey) | Out-Null
        $PfxCollection.Add($PfxPublicRoot) | Out-Null
        [IO.File]::WriteAllBytes(
            $PfxFixturePath,
            $PfxCollection.Export(
                [System.Security.Cryptography.X509Certificates.X509ContentType]::Pkcs12,
                $PfxPasswordText
            )
        )
    }
    finally {
        if ($PfxLeafWithKey) {
            $PfxLeafWithKey.Dispose()
        }
        if ($PfxPublicRoot) {
            $PfxPublicRoot.Dispose()
        }
    }

    Remove-UserTestCertificate -Thumbprint $PfxLeafCertificate.Thumbprint -FailOnError
    Remove-UserTestCertificate -Thumbprint $PfxRootCertificate.Thumbprint -FailOnError
    $PfxVerificationContext = Get-WindowsPfxVerificationContext `
        -PfxPath $PfxFixturePath `
        -Password $PfxPassword `
        -TrustEmbeddedRoot
    if ($PfxVerificationContext.Thumbprint -ne $PfxLeafCertificate.Thumbprint) {
        throw "The PFX verification context selected the wrong end-entity certificate"
    }
    if (
        $PfxVerificationContext.PrivateRoots.Count -ne 1 -or
        $PfxVerificationContext.PrivateRoots[0].Thumbprint -ne $PfxRootCertificate.Thumbprint
    ) {
        throw "The PFX verification context did not isolate the embedded private root"
    }

    Write-Host "Windows embedded Authenticode regression tests passed."
}
finally {
    if ($CatalogOnlySignature) {
        $CatalogOnlySignature.Dispose()
    }
    if ($UnsignedSignature) {
        $UnsignedSignature.Dispose()
    }
    if ($EmbeddedSignature) {
        $EmbeddedSignature.Dispose()
    }
    if ($TamperedSignature) {
        $TamperedSignature.Dispose()
    }
    Close-WindowsPfxVerificationContext $PfxVerificationContext
    if ($PfxLeafCertificate) {
        Remove-UserTestCertificate -Thumbprint $PfxLeafCertificate.Thumbprint
        $PfxLeafCertificate.Dispose()
    }
    if ($PfxRootCertificate) {
        Remove-UserTestCertificate -Thumbprint $PfxRootCertificate.Thumbprint
        $PfxRootCertificate.Dispose()
    }
    if ($Certificate) {
        Remove-UserTestCertificate -Thumbprint $Certificate.Thumbprint
        $Certificate.Dispose()
    }
    Remove-Item -LiteralPath $TemporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
