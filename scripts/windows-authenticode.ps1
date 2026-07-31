#requires -Version 7.6

function Resolve-WindowsSignToolFromBinRoots {
    param(
        [Parameter(Mandatory)][string[]]$BinRoots,
        [Parameter(Mandatory)][string[]]$Architectures
    )

    $UniqueRoots = @($BinRoots |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Unique)
    foreach ($Architecture in $Architectures) {
        foreach ($BinRoot in $UniqueRoots) {
            if (-not (Test-Path -LiteralPath $BinRoot -PathType Container)) {
                continue
            }

            $VersionDirectories = @(Get-ChildItem -LiteralPath $BinRoot -Directory `
                -ErrorAction SilentlyContinue | ForEach-Object {
                    $ParsedVersion = $null
                    if ([version]::TryParse($_.Name, [ref]$ParsedVersion)) {
                        [PSCustomObject]@{
                            Path = $_.FullName
                            Version = $ParsedVersion
                        }
                    }
                } | Sort-Object Version -Descending)
            foreach ($VersionDirectory in $VersionDirectories) {
                $Candidate = Join-Path $VersionDirectory.Path "$Architecture\signtool.exe"
                if (Test-Path -LiteralPath $Candidate -PathType Leaf) {
                    return (Resolve-Path -LiteralPath $Candidate).Path
                }
            }

            $DirectCandidate = Join-Path $BinRoot "$Architecture\signtool.exe"
            if (Test-Path -LiteralPath $DirectCandidate -PathType Leaf) {
                return (Resolve-Path -LiteralPath $DirectCandidate).Path
            }
        }
    }
    return $null
}

function Find-WindowsSignTool {
    $Command = Get-Command "signtool.exe" -CommandType Application -ErrorAction SilentlyContinue
    if ($Command) {
        return $Command.Source
    }

    $BinRoots = [Collections.Generic.List[string]]::new()
    $AddBinRoot = {
        param([string]$Path)
        if (-not [string]::IsNullOrWhiteSpace($Path)) {
            $BinRoots.Add($Path) | Out-Null
        }
    }

    & $AddBinRoot ([Environment]::GetEnvironmentVariable("WindowsSdkVerBinPath"))
    & $AddBinRoot ([Environment]::GetEnvironmentVariable("WindowsSdkBinPath"))
    $WindowsSdkDirectory = [Environment]::GetEnvironmentVariable("WindowsSdkDir")
    if ($WindowsSdkDirectory) {
        & $AddBinRoot (Join-Path $WindowsSdkDirectory "bin")
    }

    foreach ($RegistryPath in @(
        "HKLM:\SOFTWARE\Microsoft\Windows Kits\Installed Roots",
        "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"
    )) {
        $InstalledRoots = Get-ItemProperty -LiteralPath $RegistryPath -ErrorAction SilentlyContinue
        $KitsRootProperty = if ($InstalledRoots) {
            $InstalledRoots.PSObject.Properties["KitsRoot10"]
        } else {
            $null
        }
        if ($KitsRootProperty -and $KitsRootProperty.Value) {
            & $AddBinRoot (Join-Path ([string]$KitsRootProperty.Value) "bin")
        }
    }

    foreach ($ProgramFilesRoot in @(
        [Environment]::GetEnvironmentVariable("ProgramFiles", "Process"),
        [Environment]::GetEnvironmentVariable("ProgramFiles(x86)", "Process")
    )) {
        if ($ProgramFilesRoot) {
            & $AddBinRoot (Join-Path $ProgramFilesRoot "Windows Kits\10\bin")
        }
    }
    $SystemDrive = [Environment]::GetEnvironmentVariable("SystemDrive")
    if ($SystemDrive) {
        & $AddBinRoot (Join-Path $SystemDrive "Windows Kits\10\bin")
    }
    & $AddBinRoot "C:\Windows Kits\10\bin"

    $ProcessArchitecture = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITEW6432")
    if (-not $ProcessArchitecture) {
        $ProcessArchitecture = [Environment]::GetEnvironmentVariable("PROCESSOR_ARCHITECTURE")
    }
    $Architectures = switch ($ProcessArchitecture.ToUpperInvariant()) {
        "ARM64" { @("arm64", "x64", "x86"); break }
        "AMD64" { @("x64", "x86"); break }
        "X86" { @("x86"); break }
        default { @("x64", "arm64", "x86") }
    }
    $Candidate = Resolve-WindowsSignToolFromBinRoots `
        -BinRoots $BinRoots.ToArray() `
        -Architectures $Architectures
    if ($Candidate) {
        return $Candidate
    }

    $SearchedRoots = @($BinRoots | Select-Object -Unique) -join ", "
    throw "signtool.exe is unavailable. Install the latest Windows SDK or add it to PATH. Searched Windows SDK bin roots: $SearchedRoots"
}

function Get-WindowsPfxVerificationContext {
    param(
        [Parameter(Mandatory)][string]$PfxPath,
        [Parameter(Mandatory)][Security.SecureString]$Password,
        [switch]$TrustEmbeddedRoot
    )

    if ($env:OS -ne "Windows_NT") {
        throw "PFX Authenticode verification is only supported on Windows"
    }
    if (-not (Test-Path -LiteralPath $PfxPath -PathType Leaf)) {
        throw "Code-signing PFX was not found: $PfxPath"
    }
    $ResolvedPfx = (Resolve-Path -LiteralPath $PfxPath).Path
    $Pfx = Get-PfxData -FilePath $ResolvedPfx -Password $Password
    $EndEntityCertificates = @($Pfx.EndEntityCertificates | Where-Object { $_ })
    $Certificates = @(
        (@($Pfx.EndEntityCertificates) + @($Pfx.OtherCertificates)) |
            Where-Object { $_ } |
            Sort-Object Thumbprint -Unique
    )
    try {
        if ($EndEntityCertificates.Count -ne 1) {
            throw "The PFX must contain exactly one end-entity code-signing certificate"
        }
        $SignerCertificate = $EndEntityCertificates[0]
        $PrivateRoots = @()
        if ($TrustEmbeddedRoot) {
            $PrivateRoots = @($Certificates | Where-Object {
                $Certificate = $_
                $BasicConstraints = @($Certificate.Extensions | Where-Object {
                    $_.Oid.Value -eq "2.5.29.19"
                }) | Select-Object -First 1
                $IsCertificateAuthority = $false
                if ($BasicConstraints) {
                    $ParsedConstraints = [System.Security.Cryptography.X509Certificates.X509BasicConstraintsExtension]::new()
                    $ParsedConstraints.CopyFrom($BasicConstraints)
                    $IsCertificateAuthority = $ParsedConstraints.CertificateAuthority
                }
                $Certificate.Thumbprint -ne $SignerCertificate.Thumbprint -and
                    $Certificate.Subject -eq $Certificate.Issuer -and
                    $IsCertificateAuthority
            } | Sort-Object Thumbprint -Unique)
            if ($PrivateRoots.Count -eq 0) {
                Write-Host "No embedded private root certificate was found"
            } else {
                foreach ($Root in $PrivateRoots) {
                    Write-Host "Using isolated private trust anchor: $($Root.Subject)"
                }
            }
        }

        return [PSCustomObject]@{
            PfxPath = $ResolvedPfx
            SignerCertificate = $SignerCertificate
            Thumbprint = $SignerCertificate.Thumbprint
            Certificates = $Certificates
            PrivateRoots = $PrivateRoots
            ChainCertificates = @($Pfx.OtherCertificates)
        }
    }
    catch {
        foreach ($Certificate in $Certificates) {
            $Certificate.Dispose()
        }
        throw
    }
}

function Close-WindowsPfxVerificationContext {
    param($Context)

    if (-not $Context) {
        return
    }
    foreach ($Certificate in @($Context.Certificates)) {
        if ($Certificate) {
            $Certificate.Dispose()
        }
    }
}

function Get-WindowsEmbeddedSignature {
    param([Parameter(Mandatory)][string]$File)

    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) {
        throw "Windows signed file does not exist: $File"
    }
    if (-not ("CamelliaNexus.Build.WinTrust" -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.Cryptography.X509Certificates;

namespace CamelliaNexus.Build
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    internal struct WinTrustFileInfo
    {
        internal uint StructSize;
        [MarshalAs(UnmanagedType.LPWStr)]
        internal string FilePath;
        internal IntPtr FileHandle;
        internal IntPtr KnownSubject;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct WinTrustData
    {
        internal uint StructSize;
        internal IntPtr PolicyCallbackData;
        internal IntPtr SipClientData;
        internal uint UiChoice;
        internal uint RevocationChecks;
        internal uint UnionChoice;
        internal IntPtr FileInfo;
        internal uint StateAction;
        internal IntPtr StateData;
        internal IntPtr UrlReference;
        internal uint ProviderFlags;
        internal uint UiContext;
        internal IntPtr SignatureSettings;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct CryptProviderCertificate
    {
        internal uint StructSize;
        internal IntPtr CertificateContext;
        internal int Commercial;
        internal int TrustedRoot;
        internal int SelfSigned;
        internal int TestCertificate;
        internal uint RevokedReason;
        internal uint Confidence;
        internal uint Error;
        internal IntPtr TrustListContext;
        internal int TrustListSignerCertificate;
        internal IntPtr CtlContext;
        internal uint CtlError;
        internal int IsCyclic;
        internal IntPtr ChainElement;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct CryptProviderSigner
    {
        internal uint StructSize;
        internal System.Runtime.InteropServices.ComTypes.FILETIME VerifyAsOf;
        internal uint CertificateChainCount;
        internal IntPtr CertificateChain;
        internal uint SignerType;
        internal IntPtr SignerInfo;
        internal uint Error;
        internal uint CounterSignerCount;
        internal IntPtr CounterSigners;
        internal IntPtr ChainContext;
    }

    [StructLayout(LayoutKind.Sequential)]
    internal struct CryptDataBlob
    {
        internal uint Size;
        internal IntPtr Data;
    }

    public sealed class EmbeddedSignature : IDisposable
    {
        public int Status { get; private set; }
        public X509Certificate2 SignerCertificate { get; private set; }
        public X509Certificate2 TimestampCertificate { get; private set; }
        public uint TimestampCount { get; private set; }

        internal EmbeddedSignature(
            int status,
            X509Certificate2 signerCertificate,
            X509Certificate2 timestampCertificate,
            uint timestampCount)
        {
            Status = status;
            SignerCertificate = signerCertificate;
            TimestampCertificate = timestampCertificate;
            TimestampCount = timestampCount;
        }

        public void Dispose()
        {
            if (SignerCertificate != null)
            {
                SignerCertificate.Dispose();
                SignerCertificate = null;
            }
            if (TimestampCertificate != null)
            {
                TimestampCertificate.Dispose();
                TimestampCertificate = null;
            }
        }
    }

    public static class WinTrust
    {
        public const int Success = 0;
        public const int BadDigest = unchecked((int)0x80096010);
        public const int NoSignature = unchecked((int)0x800B0100);
        public const int UntrustedRoot = unchecked((int)0x800B0109);
        public const int PartialChain = unchecked((int)0x800B010A);
        private const uint UiNone = 2;
        private const uint ChoiceFile = 1;
        private const uint StateVerify = 1;
        private const uint StateClose = 2;
        private const uint QueryObjectFile = 1;
        private const uint QueryObjectBlob = 2;
        private const uint QueryContentPkcs7Signed = 8;
        private const uint QueryContentFlagPkcs7Signed = 0x00000100;
        private const uint QueryContentPkcs7SignedEmbed = 0x00000400;
        private const uint QueryFormatBinary = 0x00000002;
        private const uint MessageSignerCertificateInfo = 7;
        private const uint FindSubjectCertificate = 0x000B0000;
        private const ushort DosSignature = 0x5A4D;
        private const uint PeSignature = 0x00004550;
        private const ushort Pe32OptionalHeaderMagic = 0x010B;
        private const ushort Pe32PlusOptionalHeaderMagic = 0x020B;
        private const int DosPeHeaderOffset = 0x3C;
        private const int CoffHeaderSize = 20;
        private const int Pe32DirectoryCountOffset = 92;
        private const int Pe32DataDirectoriesOffset = 96;
        private const int Pe32PlusDirectoryCountOffset = 108;
        private const int Pe32PlusDataDirectoriesOffset = 112;
        private const int CertificateDirectoryIndex = 4;
        private const int DataDirectorySize = 8;
        private const ushort WinCertificateTypePkcsSignedData = 0x0002;
        private const uint MaximumEmbeddedCertificates = 64;
        private const uint MaximumEmbeddedCertificateSize = 16 * 1024 * 1024;
        private static readonly Guid VerifyV2 =
            new Guid("00AAC56B-CD44-11d0-8CC2-00C04FC295EE");

        [DllImport("wintrust.dll", ExactSpelling = true, CharSet = CharSet.Unicode)]
        private static extern int WinVerifyTrust(
            IntPtr windowHandle,
            [In] ref Guid actionId,
            [In] ref WinTrustData trustData);

        [DllImport("wintrust.dll", ExactSpelling = true)]
        private static extern IntPtr WTHelperProvDataFromStateData(IntPtr stateData);

        [DllImport("wintrust.dll", ExactSpelling = true)]
        private static extern IntPtr WTHelperGetProvSignerFromChain(
            IntPtr providerData,
            uint signerIndex,
            [MarshalAs(UnmanagedType.Bool)] bool counterSigner,
            uint counterSignerIndex);

        [DllImport("wintrust.dll", ExactSpelling = true)]
        private static extern IntPtr WTHelperGetProvCertFromChain(
            IntPtr signer,
            uint certificateIndex);

        [DllImport(
            "crypt32.dll",
            EntryPoint = "CryptQueryObject",
            SetLastError = true,
            CharSet = CharSet.Unicode)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CryptQueryFile(
            uint objectType,
            [MarshalAs(UnmanagedType.LPWStr)] string filePath,
            uint expectedContentTypeFlags,
            uint expectedFormatTypeFlags,
            uint flags,
            out uint messageAndCertificateEncodingType,
            out uint contentType,
            out uint formatType,
            out IntPtr certificateStore,
            out IntPtr message,
            IntPtr context);

        [DllImport("crypt32.dll", EntryPoint = "CryptQueryObject", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CryptQueryBlob(
            uint objectType,
            ref CryptDataBlob blob,
            uint expectedContentTypeFlags,
            uint expectedFormatTypeFlags,
            uint flags,
            out uint messageAndCertificateEncodingType,
            out uint contentType,
            out uint formatType,
            out IntPtr certificateStore,
            out IntPtr message,
            IntPtr context);

        [DllImport("crypt32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CryptMsgGetParam(
            IntPtr message,
            uint parameterType,
            uint index,
            IntPtr data,
            ref uint dataSize);

        [DllImport("crypt32.dll", SetLastError = true)]
        private static extern IntPtr CertFindCertificateInStore(
            IntPtr certificateStore,
            uint certificateEncodingType,
            uint findFlags,
            uint findType,
            IntPtr findParameter,
            IntPtr previousCertificateContext);

        [DllImport("crypt32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CertFreeCertificateContext(IntPtr certificateContext);

        [DllImport("crypt32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CertCloseStore(IntPtr certificateStore, uint flags);

        [DllImport("crypt32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool CryptMsgClose(IntPtr message);

        private static X509Certificate2 GetCertificate(IntPtr signer)
        {
            if (signer == IntPtr.Zero)
            {
                return null;
            }
            IntPtr certificatePointer = WTHelperGetProvCertFromChain(signer, 0);
            if (certificatePointer == IntPtr.Zero)
            {
                return null;
            }
            CryptProviderCertificate providerCertificate =
                Marshal.PtrToStructure<CryptProviderCertificate>(certificatePointer);
            if (providerCertificate.CertificateContext == IntPtr.Zero)
            {
                return null;
            }
            return new X509Certificate2(providerCertificate.CertificateContext);
        }

        private static X509Certificate2 FindSignerCertificate(
            IntPtr certificateStore,
            IntPtr message,
            uint encodingType)
        {
            IntPtr signerInfo = IntPtr.Zero;
            IntPtr certificateContext = IntPtr.Zero;
            try
            {
                uint signerInfoSize = 0;
                if (!CryptMsgGetParam(
                    message,
                    MessageSignerCertificateInfo,
                    0,
                    IntPtr.Zero,
                    ref signerInfoSize) || signerInfoSize == 0)
                {
                    throw NativeFailure("CryptMsgGetParam(size)");
                }
                signerInfo = Marshal.AllocHGlobal(checked((int)signerInfoSize));
                if (!CryptMsgGetParam(
                    message,
                    MessageSignerCertificateInfo,
                    0,
                    signerInfo,
                    ref signerInfoSize))
                {
                    throw NativeFailure("CryptMsgGetParam(signer)");
                }

                certificateContext = CertFindCertificateInStore(
                    certificateStore,
                    encodingType,
                    0,
                    FindSubjectCertificate,
                    signerInfo,
                    IntPtr.Zero);
                if (certificateContext == IntPtr.Zero)
                {
                    throw NativeFailure("CertFindCertificateInStore");
                }
                return new X509Certificate2(certificateContext);
            }
            finally
            {
                if (certificateContext != IntPtr.Zero)
                {
                    CertFreeCertificateContext(certificateContext);
                }
                if (signerInfo != IntPtr.Zero)
                {
                    Marshal.FreeHGlobal(signerInfo);
                }
            }
        }

        private static X509Certificate2 QueryEmbeddedSignerCertificate(string filePath)
        {
            IntPtr certificateStore = IntPtr.Zero;
            IntPtr message = IntPtr.Zero;
            try
            {
                uint encodingType;
                uint contentType;
                uint formatType;
                if (!CryptQueryFile(
                    QueryObjectFile,
                    filePath,
                    QueryContentPkcs7SignedEmbed,
                    QueryFormatBinary,
                    0,
                    out encodingType,
                    out contentType,
                    out formatType,
                    out certificateStore,
                    out message,
                    IntPtr.Zero))
                {
                    return null;
                }
                return FindSignerCertificate(certificateStore, message, encodingType);
            }
            finally
            {
                if (message != IntPtr.Zero)
                {
                    CryptMsgClose(message);
                }
                if (certificateStore != IntPtr.Zero)
                {
                    CertCloseStore(certificateStore, 0);
                }
            }
        }

        private static uint GetDerObjectLength(IntPtr data, uint available)
        {
            if (available < 2 || Marshal.ReadByte(data, 0) != 0x30)
            {
                throw new InvalidOperationException(
                    "The embedded Authenticode certificate is not a DER sequence");
            }
            byte encodedLength = Marshal.ReadByte(data, 1);
            uint headerLength = 2;
            uint contentLength;
            if ((encodedLength & 0x80) == 0)
            {
                contentLength = encodedLength;
            }
            else
            {
                int lengthBytes = encodedLength & 0x7F;
                if (lengthBytes == 0 || lengthBytes > 4 || available < 2 + lengthBytes)
                {
                    throw new InvalidOperationException(
                        "The embedded Authenticode certificate has an invalid DER length");
                }
                headerLength += (uint)lengthBytes;
                contentLength = 0;
                for (int index = 0; index < lengthBytes; index++)
                {
                    contentLength = checked(
                        (contentLength << 8) | Marshal.ReadByte(data, 2 + index));
                }
            }
            uint totalLength = checked(headerLength + contentLength);
            if (totalLength > available)
            {
                throw new InvalidOperationException(
                    "The embedded Authenticode certificate is truncated");
            }
            return totalLength;
        }

        private static X509Certificate2 QueryPkcs7SignerCertificate(
            IntPtr data,
            uint available)
        {
            var blob = new CryptDataBlob
            {
                Size = GetDerObjectLength(data, available),
                Data = data
            };
            IntPtr certificateStore = IntPtr.Zero;
            IntPtr message = IntPtr.Zero;
            try
            {
                uint encodingType;
                uint contentType;
                uint formatType;
                if (!CryptQueryBlob(
                    QueryObjectBlob,
                    ref blob,
                    QueryContentFlagPkcs7Signed,
                    QueryFormatBinary,
                    0,
                    out encodingType,
                    out contentType,
                    out formatType,
                    out certificateStore,
                    out message,
                    IntPtr.Zero))
                {
                    throw NativeFailure("CryptQueryObject(PKCS7)");
                }
                if (contentType != QueryContentPkcs7Signed)
                {
                    throw new InvalidOperationException(
                        "The embedded Authenticode certificate is not PKCS#7 SignedData");
                }
                return FindSignerCertificate(certificateStore, message, encodingType);
            }
            finally
            {
                if (message != IntPtr.Zero)
                {
                    CryptMsgClose(message);
                }
                if (certificateStore != IntPtr.Zero)
                {
                    CertCloseStore(certificateStore, 0);
                }
            }
        }

        private static byte[] ReadBytesExact(
            BinaryReader reader,
            int length,
            string description)
        {
            byte[] value = reader.ReadBytes(length);
            if (value.Length != length)
            {
                throw new InvalidOperationException(
                    "The Windows image is truncated while reading " + description);
            }
            return value;
        }

        private static void RequireZeroPadding(byte[] padding)
        {
            foreach (byte value in padding)
            {
                if (value != 0)
                {
                    throw new InvalidOperationException(
                        "The embedded WIN_CERTIFICATE padding is not zero-filled");
                }
            }
        }

        private static X509Certificate2 QueryPkcs7SignerCertificate(byte[] data)
        {
            IntPtr certificate = Marshal.AllocHGlobal(data.Length);
            try
            {
                Marshal.Copy(data, 0, certificate, data.Length);
                return QueryPkcs7SignerCertificate(certificate, checked((uint)data.Length));
            }
            finally
            {
                Marshal.FreeHGlobal(certificate);
            }
        }

        private static X509Certificate2 GetPeEmbeddedSignerCertificate(
            string filePath,
            out bool portableExecutable)
        {
            portableExecutable = false;
            using (var stream = new FileStream(
                filePath,
                FileMode.Open,
                FileAccess.Read,
                FileShare.Read | FileShare.Delete))
            using (var reader = new BinaryReader(stream))
            {
                if (stream.Length < sizeof(ushort) || reader.ReadUInt16() != DosSignature)
                {
                    return null;
                }
                portableExecutable = true;
                if (stream.Length < DosPeHeaderOffset + sizeof(uint))
                {
                    throw new InvalidOperationException("The Windows image has a truncated DOS header");
                }

                stream.Position = DosPeHeaderOffset;
                uint peHeaderOffset = reader.ReadUInt32();
                long optionalHeaderOffset = checked((long)peHeaderOffset + 4 + CoffHeaderSize);
                if (peHeaderOffset > stream.Length - 4 - CoffHeaderSize ||
                    optionalHeaderOffset > stream.Length)
                {
                    throw new InvalidOperationException("The Windows image has an invalid PE header offset");
                }

                stream.Position = peHeaderOffset;
                if (reader.ReadUInt32() != PeSignature)
                {
                    throw new InvalidOperationException("The Windows image has an invalid PE signature");
                }
                stream.Position = checked((long)peHeaderOffset + 4 + 16);
                ushort optionalHeaderSize = reader.ReadUInt16();
                if (optionalHeaderOffset > stream.Length - optionalHeaderSize)
                {
                    throw new InvalidOperationException("The Windows image has a truncated optional header");
                }

                stream.Position = optionalHeaderOffset;
                ushort optionalHeaderMagic = reader.ReadUInt16();
                int directoryCountOffset;
                int dataDirectoriesOffset;
                if (optionalHeaderMagic == Pe32OptionalHeaderMagic)
                {
                    directoryCountOffset = Pe32DirectoryCountOffset;
                    dataDirectoriesOffset = Pe32DataDirectoriesOffset;
                }
                else if (optionalHeaderMagic == Pe32PlusOptionalHeaderMagic)
                {
                    directoryCountOffset = Pe32PlusDirectoryCountOffset;
                    dataDirectoriesOffset = Pe32PlusDataDirectoriesOffset;
                }
                else
                {
                    throw new InvalidOperationException(
                        "The Windows image has an unsupported optional-header format");
                }

                int certificateDirectoryEnd = checked(
                    dataDirectoriesOffset +
                    ((CertificateDirectoryIndex + 1) * DataDirectorySize));
                if (optionalHeaderSize < certificateDirectoryEnd)
                {
                    throw new InvalidOperationException(
                        "The Windows image does not contain a complete certificate directory");
                }
                stream.Position = checked(optionalHeaderOffset + directoryCountOffset);
                uint directoryCount = reader.ReadUInt32();
                if (directoryCount <= CertificateDirectoryIndex)
                {
                    return null;
                }

                stream.Position = checked(
                    optionalHeaderOffset +
                    dataDirectoriesOffset +
                    (CertificateDirectoryIndex * DataDirectorySize));
                uint certificateTableOffset = reader.ReadUInt32();
                uint certificateTableSize = reader.ReadUInt32();
                if (certificateTableOffset == 0 && certificateTableSize == 0)
                {
                    return null;
                }
                if (certificateTableOffset == 0 ||
                    certificateTableSize < 8 ||
                    certificateTableSize > MaximumEmbeddedCertificateSize ||
                    (certificateTableOffset & 7) != 0 ||
                    (ulong)certificateTableOffset + certificateTableSize > (ulong)stream.Length)
                {
                    throw new InvalidOperationException(
                        "The Windows image has an invalid embedded certificate table");
                }

                long certificateTableEnd = checked(
                    (long)certificateTableOffset + certificateTableSize);
                long certificateOffset = certificateTableOffset;
                uint certificateCount = 0;
                X509Certificate2 signer = null;
                try
                {
                    while (certificateOffset < certificateTableEnd)
                    {
                        long remaining = certificateTableEnd - certificateOffset;
                        if (remaining < 8)
                        {
                            stream.Position = certificateOffset;
                            RequireZeroPadding(ReadBytesExact(
                                reader,
                                checked((int)remaining),
                                "the certificate-table padding"));
                            break;
                        }
                        certificateCount = checked(certificateCount + 1);
                        if (certificateCount > MaximumEmbeddedCertificates)
                        {
                            throw new InvalidOperationException(
                                "The Windows image contains too many embedded certificates");
                        }

                        stream.Position = certificateOffset;
                        uint recordLength = reader.ReadUInt32();
                        reader.ReadUInt16();
                        ushort certificateType = reader.ReadUInt16();
                        if (recordLength < 8 ||
                            recordLength > remaining ||
                            recordLength > MaximumEmbeddedCertificateSize)
                        {
                            throw new InvalidOperationException(
                                "The embedded WIN_CERTIFICATE record has an invalid size");
                        }

                        if (certificateType == WinCertificateTypePkcsSignedData)
                        {
                            if (signer != null)
                            {
                                throw new InvalidOperationException(
                                    "The Windows image contains multiple embedded Authenticode records");
                            }
                            byte[] signedData = ReadBytesExact(
                                reader,
                                checked((int)recordLength - 8),
                                "the embedded Authenticode SignedData");
                            signer = QueryPkcs7SignerCertificate(signedData);
                        }

                        uint alignedLength = checked((recordLength + 7) & ~7U);
                        if (alignedLength > remaining)
                        {
                            throw new InvalidOperationException(
                                "The embedded WIN_CERTIFICATE alignment exceeds its table");
                        }
                        int paddingLength = checked((int)(alignedLength - recordLength));
                        if (paddingLength != 0)
                        {
                            stream.Position = checked(certificateOffset + recordLength);
                            RequireZeroPadding(ReadBytesExact(
                                reader,
                                paddingLength,
                                "the WIN_CERTIFICATE padding"));
                        }
                        certificateOffset = checked(certificateOffset + alignedLength);
                    }
                    return signer;
                }
                catch
                {
                    if (signer != null)
                    {
                        signer.Dispose();
                    }
                    throw;
                }
            }
        }

        private static X509Certificate2 GetEmbeddedSignerCertificate(string filePath)
        {
            bool portableExecutable;
            X509Certificate2 signer = GetPeEmbeddedSignerCertificate(
                filePath,
                out portableExecutable);
            // A PE image must be resolved only through its security directory. Falling back to a
            // generic file query could allow a catalog signer to replace missing embedded data.
            return portableExecutable ? signer : QueryEmbeddedSignerCertificate(filePath);
        }

        private static InvalidOperationException NativeFailure(string operation)
        {
            uint error = unchecked((uint)Marshal.GetLastWin32Error());
            return new InvalidOperationException(
                operation + " failed while reading the embedded Authenticode signer: 0x" +
                error.ToString("X8"));
        }

        public static EmbeddedSignature Inspect(string filePath)
        {
            var fileInfo = new WinTrustFileInfo
            {
                StructSize = (uint)Marshal.SizeOf<WinTrustFileInfo>(),
                FilePath = filePath
            };
            IntPtr fileInfoPointer = Marshal.AllocHGlobal((int)fileInfo.StructSize);
            bool initialized = false;
            var trustData = new WinTrustData();
            Guid actionId = VerifyV2;
            try
            {
                Marshal.StructureToPtr(fileInfo, fileInfoPointer, false);
                initialized = true;
                trustData = new WinTrustData
                {
                    StructSize = (uint)Marshal.SizeOf<WinTrustData>(),
                    UiChoice = UiNone,
                    UnionChoice = ChoiceFile,
                    FileInfo = fileInfoPointer,
                    StateAction = StateVerify
                };
                int status = WinVerifyTrust(new IntPtr(-1), ref actionId, ref trustData);
                // WinTrust can validate an embedded signature whose private root is intentionally
                // outside the machine trust store without exposing its leaf through provider-chain
                // metadata. Read the leaf from the signed file itself so a catalog signer can never
                // be mistaken for the requested embedded signer.
                X509Certificate2 signerCertificate = GetEmbeddedSignerCertificate(filePath);
                X509Certificate2 timestampCertificate = null;
                uint timestampCount = 0;
                if (trustData.StateData != IntPtr.Zero)
                {
                    IntPtr providerData = WTHelperProvDataFromStateData(trustData.StateData);
                    if (providerData != IntPtr.Zero)
                    {
                        IntPtr providerSigner = WTHelperGetProvSignerFromChain(
                            providerData,
                            0,
                            false,
                            0);
                        if (providerSigner != IntPtr.Zero)
                        {
                            CryptProviderSigner signer =
                                Marshal.PtrToStructure<CryptProviderSigner>(providerSigner);
                            timestampCount = signer.CounterSignerCount;
                            if (timestampCount == 1)
                            {
                                timestampCertificate = GetCertificate(signer.CounterSigners);
                            }
                        }
                    }
                }
                return new EmbeddedSignature(
                    status,
                    signerCertificate,
                    timestampCertificate,
                    timestampCount);
            }
            finally
            {
                if (trustData.StateData != IntPtr.Zero)
                {
                    trustData.StateAction = StateClose;
                    WinVerifyTrust(new IntPtr(-1), ref actionId, ref trustData);
                }
                if (initialized)
                {
                    Marshal.DestroyStructure<WinTrustFileInfo>(fileInfoPointer);
                }
                Marshal.FreeHGlobal(fileInfoPointer);
            }
        }

        public static string FormatStatus(int status)
        {
            return "0x" + unchecked((uint)status).ToString("X8");
        }
    }
}
'@
    }
    $ResolvedFile = (Resolve-Path -LiteralPath $File).Path
    return [CamelliaNexus.Build.WinTrust]::Inspect($ResolvedFile)
}

function Invoke-WindowsSignToolVerification {
    param(
        [Parameter(Mandatory)][string]$SignTool,
        [Parameter(Mandatory)][string]$File
    )

    & $SignTool verify /pa /all /v $File | Out-Host
    $ExitCode = $LASTEXITCODE
    # GitHub's PowerShell wrapper exits with the last native status. Encapsulate
    # the public-trust probe so an expected failure cannot poison a later,
    # successful isolated-private-root verification.
    $global:LASTEXITCODE = 0
    return $ExitCode
}

function Assert-WindowsSignature {
    param(
        [string]$File,
        [string]$SignTool,
        [string]$ExpectedThumbprint,
        [object[]]$PrivateRoots = @(),
        [object[]]$ChainCertificates = @()
    )

    $EmbeddedSignature = Get-WindowsEmbeddedSignature -File $File
    try {
        $SignerCertificate = $EmbeddedSignature.SignerCertificate
        $ActualThumbprint = if ($SignerCertificate) {
            $SignerCertificate.Thumbprint
        } else {
            "none"
        }
        if (-not $SignerCertificate -or $ActualThumbprint -ne $ExpectedThumbprint) {
            throw "The embedded Authenticode signer does not match the requested PFX: ${File} (expected $ExpectedThumbprint, found $ActualThumbprint)"
        }

        $TrustStatus = $EmbeddedSignature.Status
        $FormattedStatus = [CamelliaNexus.Build.WinTrust]::FormatStatus($TrustStatus)
        Write-Host "Embedded WinTrust verification status: $FormattedStatus"
        if ($PrivateRoots.Count -eq 0) {
            $SignToolExitCode = Invoke-WindowsSignToolVerification `
                -SignTool $SignTool `
                -File $File
            if ($SignToolExitCode -ne 0 -or $TrustStatus -ne [CamelliaNexus.Build.WinTrust]::Success) {
                throw "Authenticode verification failed for ${File}: $FormattedStatus"
            }
        } elseif (
            $TrustStatus -ne [CamelliaNexus.Build.WinTrust]::Success -and
            $TrustStatus -ne [CamelliaNexus.Build.WinTrust]::UntrustedRoot -and
            $TrustStatus -ne [CamelliaNexus.Build.WinTrust]::PartialChain
        ) {
            throw "Authenticode verification failed for ${File}: $FormattedStatus"
        }

        if ($EmbeddedSignature.TimestampCount -ne 1 -or -not $EmbeddedSignature.TimestampCertificate) {
            throw "The embedded Authenticode signature does not have exactly one RFC 3161 timestamp: $File"
        }

        if ($PrivateRoots.Count -eq 0) {
            Write-Host "Verified embedded Authenticode signature: $File"
            return
        }

        $Chain = [System.Security.Cryptography.X509Certificates.X509Chain]::new()
        try {
            $Chain.ChainPolicy.TrustMode = `
                [System.Security.Cryptography.X509Certificates.X509ChainTrustMode]::CustomRootTrust
            $Chain.ChainPolicy.RevocationMode = `
                [System.Security.Cryptography.X509Certificates.X509RevocationMode]::NoCheck
            $Chain.ChainPolicy.DisableCertificateDownloads = $true
            $Chain.ChainPolicy.ApplicationPolicy.Add(
                [System.Security.Cryptography.Oid]::new("1.3.6.1.5.5.7.3.3")
            ) | Out-Null
            foreach ($Root in $PrivateRoots) {
                $Chain.ChainPolicy.CustomTrustStore.Add($Root) | Out-Null
            }
            foreach ($Certificate in $ChainCertificates) {
                if ($Certificate -and $Certificate.Thumbprint -ne $ActualThumbprint) {
                    $Chain.ChainPolicy.ExtraStore.Add($Certificate) | Out-Null
                }
            }
            if (-not $Chain.Build($SignerCertificate)) {
                $Errors = @($Chain.ChainStatus | ForEach-Object {
                    "$($_.Status): $($_.StatusInformation.Trim())"
                }) -join "; "
                throw "The Authenticode certificate chain is invalid for ${File}: $Errors"
            }
            $ChainRoot = $Chain.ChainElements[$Chain.ChainElements.Count - 1].Certificate
            $ExpectedRoots = @($PrivateRoots | ForEach-Object { $_.Thumbprint })
            if ($ChainRoot.Thumbprint -notin $ExpectedRoots) {
                throw "The Authenticode chain does not terminate at an embedded private root: $File"
            }
        }
        finally {
            $Chain.Dispose()
        }
        Write-Host "Verified embedded Authenticode signature with isolated private trust: $File"
    }
    finally {
        $EmbeddedSignature.Dispose()
    }
}
