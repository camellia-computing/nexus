# Windows code signing

This document defines the supported local and GitHub Actions signing modes for Camellia Nexus. Authenticode is optional; published assets still receive SHA-256 checksums and keyless Sigstore/Cosign bundles.

## Trust model

| Mode | Configuration | Result | Suitable use |
| --- | --- | --- | --- |
| Unsigned | No Windows signing secrets | Unsigned `.exe` and `.msi` | Internal testing or users who accept SmartScreen warnings |
| Authenticode | Both Windows signing secrets | SHA-256 Authenticode plus RFC 3161 timestamp | Private-CA testing or public CA distribution |

The workflow requires both secrets or neither:

- `WINDOWS_CODESIGN_PFX_BASE64`: one-line base64 of a password-protected PFX;
- `WINDOWS_CODESIGN_PFX_PASSWORD`: its export password.

Optional variable `WINDOWS_TIMESTAMP_URL` overrides the default RFC 3161 endpoint. A partial configuration fails before the build. `RELEASE-METADATA.json` records `unsigned` or `signed`.

A self-signed/private-CA signature proves integrity only to machines that trust that private root. It does not create public SmartScreen reputation. Commercial public distribution should use a current trusted code-signing service or certificate and follow the issuer's hardware-key/cloud-signing requirements.

## Create a private development CA

Run PowerShell on a controlled Windows machine. These commands create an exportable development root and a leaf restricted to code signing.

```powershell
$root = New-SelfSignedCertificate `
  -Type Custom `
  -Subject 'CN=Camellia Nexus Development Root CA' `
  -FriendlyName 'Camellia Nexus Development Root CA' `
  -KeyAlgorithm RSA `
  -KeyLength 4096 `
  -HashAlgorithm SHA256 `
  -KeyExportPolicy Exportable `
  -KeyUsage CertSign, CRLSign, DigitalSignature `
  -TextExtension @('2.5.29.19={critical}{text}ca=true&pathlength=0') `
  -NotAfter (Get-Date).AddYears(10) `
  -CertStoreLocation 'Cert:\CurrentUser\My'

$rootCer = Join-Path $PWD 'camellia-nexus-Development-Root.cer'
Export-Certificate -Cert $root -FilePath $rootCer | Out-Null
Import-Certificate -FilePath $rootCer -CertStoreLocation 'Cert:\CurrentUser\Root' | Out-Null

$leaf = New-SelfSignedCertificate `
  -Type CodeSigningCert `
  -Subject 'CN=Camellia Nexus Development Code Signing' `
  -FriendlyName 'Camellia Nexus Development Code Signing' `
  -Signer $root `
  -KeyAlgorithm RSA `
  -KeyLength 3072 `
  -HashAlgorithm SHA256 `
  -KeyExportPolicy Exportable `
  -NotAfter (Get-Date).AddMonths(24) `
  -CertStoreLocation 'Cert:\CurrentUser\My'

$password = Read-Host 'PFX export password' -AsSecureString
$pfx = Join-Path $PWD 'camellia-nexus-development-codesign.pfx'
Export-PfxCertificate `
  -Cert $leaf `
  -FilePath $pfx `
  -Password $password `
  -ChainOption BuildChain | Out-Null
```

Keep the root private key offline after issuing the leaf. Install only the public root certificate on managed test machines. Never commit the `.pfx`, password, root key or generated base64 file, and never ask public customers to trust this development root.

## Local build and verification

```powershell
$password = Read-Host 'PFX password' -AsSecureString
./scripts/ci-local.ps1 `
  -Mode DesktopPackage `
  -SkipQuality `
  -Sign `
  -TrustEmbeddedRoot `
  -PfxPath '.\camellia-nexus-development-codesign.pfx' `
  -PfxPassword $password
```

The build script imports the PFX into the current-user store, signs the executable and MSI with SHA-256, requests an RFC 3161 timestamp, verifies the exact signer, validates the embedded private chain in an isolated trust store, and removes temporary certificate-store entries.

Release staging reopens the same ephemeral PFX without importing its private key, reconstructs the
verification-only certificate context, and checks the exact leaf thumbprint, embedded file
signature, single timestamp, and isolated private root for the portable application, broker, and
copied MSI. This remains valid after the build removes its temporary certificate-store entries and
does not weaken private-CA verification to accepting an arbitrary untrusted signer. The workflow
exposes the PFX password only to the signed build and staging steps and deletes the temporary PFX in
an always-run cleanup step.

For an MSI build, Tauri signs the bundle-patched application that is embedded in the installer and
then restores the original standalone executable in `target/release`. The local build wrapper
inspects that restored file and signs it at its final portable-package byte boundary only when
WinTrust reports `TRUST_E_NOSIGNATURE`. An existing signer, unexpected trust state, or timestamp
count other than one fails closed instead of appending another Authenticode signature.

The signing helper resolves `signtool.exe` from `PATH`, the active Windows SDK environment
variables, the `KitsRoot10` registry installation root, and common Windows Kits directories under
both Program Files locations and the system drive. When multiple SDK versions are installed, it
selects the newest version that supports the current process architecture.

Verify a public-CA staged file independently with the same embedded-signature helper used by the
build. Supply the expected leaf-certificate thumbprint from the reviewed signing identity:

```powershell
. .\scripts\windows-authenticode.ps1
Assert-WindowsSignature `
  -File '.\target\release\camellia-nexus.exe' `
  -SignTool (Find-WindowsSignTool) `
  -ExpectedThumbprint '<reviewed-leaf-thumbprint>'
```

The helper selects the embedded file signature through WinTrust `WTD_CHOICE_FILE`, checks the exact
signer, requires exactly one timestamp countersignature, and then applies normal Windows trust
policy. `Get-AuthenticodeSignature` is useful for observing Windows policy, but it may prefer a
catalog signature and therefore is not proof of the embedded signer. The regression fixture in
`scripts/test-windows-authenticode.ps1` deliberately creates conflicting catalog and embedded
signatures and verifies that the release check selects the embedded one.

`signtool verify` succeeds under normal policy only when Windows trusts the chain. For a private-CA
build, use the complete `ci-local.ps1` command above so the helper also receives the isolated private
root and intermediate set. An unmanaged machine may otherwise report an untrusted root even when
the signature bytes and timestamp are intact.

## GitHub Actions configuration

Create a one-line base64 value without printing the PFX password:

```powershell
$pfx = Resolve-Path '.\camellia-nexus-development-codesign.pfx'
[Convert]::ToBase64String([IO.File]::ReadAllBytes($pfx)) |
  Set-Content -NoNewline '.\certificate-base64.txt'
```

Configure the current repository without embedding an owner or account:

```powershell
Get-Content -Raw '.\certificate-base64.txt' | gh secret set WINDOWS_CODESIGN_PFX_BASE64
gh secret set WINDOWS_CODESIGN_PFX_PASSWORD
gh variable set WINDOWS_TIMESTAMP_URL --body 'http://timestamp.digicert.com'
```

Delete `certificate-base64.txt` after configuration. The release workflow decodes the PFX only into the ephemeral runner directory and removes it in an always-run cleanup step.

## Rotation and incident handling

- Replace the PFX and password together; a half-rotated pair intentionally blocks release.
- Revoke or distrust a compromised certificate before uploading a replacement.
- Preserve timestamp evidence: a valid RFC 3161 timestamp allows verification of a signature made while the certificate was valid.
- A release whose native signing status is unexpected must remain a draft. Do not rewrite metadata or sign already-published bytes.

## References

- [Tauri Windows code-signing guide](https://v2.tauri.app/distribute/sign/windows/)
- [Microsoft `New-SelfSignedCertificate`](https://learn.microsoft.com/powershell/module/pki/new-selfsignedcertificate)
- [Microsoft SignTool](https://learn.microsoft.com/windows/win32/seccrypto/signtool)
