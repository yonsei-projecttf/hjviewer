# Windows release signing boundary (docs/ci.md, "Windows 코드 서명"). This script never contacts
# a signing service. Prepare copies the PE images that have no valid signature to a flat input
# directory; Azure Artifact Signing signs that directory in place; Verify checks every signed copy
# and changes Root only after the whole batch has passed. Audit fails while an unsigned PE image is
# left. Run with PowerShell 7 on Windows.
#
# Artifact Signing hands out a new certificate every day, valid for 72 hours, so no certificate is
# pinned here. A signature is accepted when Windows calls it valid, its file digest is SHA-256, it
# carries an RFC 3161 timestamp (which keeps it valid after the certificate expires), its chain
# runs through Microsoft's Artifact Signing CA (ExpectedChainCA), the signer's common name is the
# publisher that identity validation approved (ExpectedPublisher), and every file of the batch was
# signed by the same certificate.
#
# A vendor file is kept as it is only with a valid signature embedded in the file: a catalog's
# signature lives on the machine that checked it, not in the package, so a catalog-only file stops
# every mode. Kept signatures that are not SHA-256 or carry no RFC 3161 timestamp are warned about
# (::warning::) in Preflight and Audit. PinFile lists vendor files, by SHA-256 and relative path,
# that are shipped as they are without a signature Windows accepts on its own: their own signature
# is not checked and they are never signed, only compared byte for byte
# (scripts/windows-signing-pins.txt).
#
# Errors are a code, and after two underscores the public causes where there are any: the chain
# status flags of X509Chain, the class of WinTrust's message, the timestamp check that failed. No
# message, certificate or path of the machine is printed.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Preflight', 'Prepare', 'Verify', 'Audit')][string]$Mode,
    [Parameter(Mandatory = $true)][string]$Root,
    [string]$Stage,
    [string]$Signed,
    [string]$Manifest,
    [string]$Report,
    [string]$PinFile,
    [string]$ExpectedPublisher,
    [string]$ExpectedChainCA = 'Microsoft ID Verified Code Signing PCA 2021'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# The CMS and ASN.1 readers of .NET, loaded here so that their type names resolve also when this
# file is dot-sourced (scripts/test_windows_signing.ps1). Windows PowerShell 5.1 has neither and
# stops at POWERSHELL_7_REQUIRED below.
if ($PSVersionTable.PSVersion.Major -ge 7) { Add-Type -AssemblyName System.Security.Cryptography.Pkcs, System.Formats.Asn1 }
$script:CodeSigningOid = '1.3.6.1.5.5.7.3.3'
$script:TimeStampingOid = '1.3.6.1.5.5.7.3.8'
$script:Sha256Oid = '2.16.840.1.101.3.4.2.1'
$script:SpcIndirectDataOid = '1.3.6.1.4.1.311.2.1.4'
$script:NestedSignatureOid = '1.3.6.1.4.1.311.2.4.1'
$script:Rfc3161TimestampOid = '1.3.6.1.4.1.311.3.3.1'
$script:ManifestSchema = 2
$script:ValidatedReportPath = $null
$script:Pins = @{}
# The pauses before the second and third online chain build (a revocation server can be slow)
$script:ChainRetrySeconds = @(2, 5)
$script:LastChainAttempts = 0

function Stop-Signing([string]$Code, [string[]]$Causes) {
    # Do not forward provider errors: they can contain credentials or machine paths. A cause is a
    # public name (a chain status flag, a class of WinTrust's message, a timestamp check), reduced
    # to letters and digits and appended after two underscores.
    $words = @($Causes | Where-Object { $_ } | ForEach-Object { ($_ -replace '[^A-Za-z0-9]', '').ToUpperInvariant() } |
        Where-Object { $_ } | Sort-Object -Unique)
    if ($words.Count -gt 0) { $Code = $Code + '__' + ($words -join '_') }
    throw [InvalidOperationException]::new("Windows signing check failed: $Code")
}

function Get-FullSigningPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { Stop-Signing 'PATH_REQUIRED' }
    return [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path)).TrimEnd('\', '/')
}

function Assert-NoReparse([string]$Path) {
    $cursor = $Path
    while ($cursor) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                Stop-Signing 'REPARSE_POINT'
            }
        }
        $parent = [IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) { break }
        $cursor = $parent
    }
}

function Test-Within([string]$Path, [string]$Directory) {
    return $Path.Equals($Directory, [StringComparison]::OrdinalIgnoreCase) -or
        $Path.StartsWith($Directory + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
}

function Assert-Separate([string]$A, [string]$B) {
    if ((Test-Within $A $B) -or (Test-Within $B $A)) { Stop-Signing 'OVERLAPPING_PATHS' }
}

function Get-SafeChild([string]$Directory, [string]$Relative) {
    if ([string]::IsNullOrWhiteSpace($Relative) -or $Relative -match '[\x00-\x1f:<>"|?*]' -or
        [IO.Path]::IsPathRooted($Relative) -or ($Relative -split '[/\\]') -contains '..' -or
        ($Relative -split '[/\\]') -contains '.' -or $Relative -match '(^|[/\\])($|[^/\\]*[ .]($|[/\\]))') {
        Stop-Signing 'UNSAFE_RELATIVE_PATH'
    }
    $path = [IO.Path]::GetFullPath((Join-Path $Directory $Relative))
    if (-not (Test-Within $path $Directory) -or $path -eq $Directory) { Stop-Signing 'PATH_ESCAPE' }
    Assert-NoReparse $path
    return $path
}

function Get-TreeFiles([string]$Directory) {
    Assert-NoReparse $Directory
    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) { Stop-Signing 'DIRECTORY_MISSING' }
    $queue = [Collections.Generic.Stack[string]]::new()
    $queue.Push($Directory)
    $result = [Collections.Generic.List[object]]::new()
    $seen = @{}
    while ($queue.Count -gt 0) {
        foreach ($item in @(Get-ChildItem -LiteralPath $queue.Pop() -Force)) {
            if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { Stop-Signing 'REPARSE_POINT' }
            $relative = $item.FullName.Substring($Directory.Length + 1).Replace('\', '/')
            $null = Get-SafeChild $Directory $relative
            if ($item.PSIsContainer) { $queue.Push($item.FullName); continue }
            if ($seen.ContainsKey($relative)) { Stop-Signing 'DUPLICATE_PATH' }
            $seen[$relative] = $true
            $result.Add([pscustomobject]@{ Relative = $relative; FullName = $item.FullName })
        }
    }
    return @($result | Sort-Object Relative)
}

function Get-FileSha([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-Pins([string]$Path) {
    # "<sha256>  <relative path>" per line, as sha256sum writes it; '#' starts a comment.
    $pins = @{}
    Assert-NoReparse $Path
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { Stop-Signing 'PIN_FILE_MISSING' }
    foreach ($line in [IO.File]::ReadAllLines($Path)) {
        $text = ($line -replace '#.*$', '').Trim()
        if (-not $text) { continue }
        $match = [regex]::Match($text, '^([0-9a-f]{64})\s+(\S+)$')
        if (-not $match.Success) { Stop-Signing 'PIN_FILE_INVALID' }
        $sha = $match.Groups[1].Value
        $relative = $match.Groups[2].Value.Replace('\', '/')
        # Keys compare without case, as Windows names do.
        if ($pins.ContainsKey($relative)) { Stop-Signing 'PIN_FILE_INVALID' }
        $pins[$relative] = $sha
    }
    if ($pins.Count -eq 0) { Stop-Signing 'PIN_FILE_INVALID' }
    return $pins
}

function Get-PeInfo([string]$Path) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $extension = [IO.Path]::GetExtension($Path).ToLowerInvariant()
    if ($bytes.Length -lt 2 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) {
        if ($extension -in @('.exe', '.dll', '.sys')) { Stop-Signing 'PE_HEADER_MISSING' }
        return $null
    }
    if ($bytes.Length -lt 64) { Stop-Signing 'PE_HEADER_INVALID' }
    $pe = [BitConverter]::ToUInt32($bytes, 60)
    if ([long]$pe + 24 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes, [int]$pe) -ne 0x4550) {
        Stop-Signing 'PE_HEADER_INVALID'
    }
    $optional = [int]$pe + 24
    $optionalSize = [BitConverter]::ToUInt16($bytes, [int]$pe + 20)
    if ($optionalSize -lt 96 -or [long]$optional + $optionalSize -gt $bytes.Length) { Stop-Signing 'PE_HEADER_INVALID' }
    $magic = [BitConverter]::ToUInt16($bytes, $optional)
    if ($magic -eq 0x20b) { $directory = $optional + 112; $countAt = $optional + 108 }
    elseif ($magic -eq 0x10b) { $directory = $optional + 96; $countAt = $optional + 92 }
    else { Stop-Signing 'PE_FORMAT_UNSUPPORTED' }
    $securityAt = $directory + 32
    if ($securityAt + 8 -gt $optional + $optionalSize -or [BitConverter]::ToUInt32($bytes, $countAt) -lt 5) {
        Stop-Signing 'PE_HEADER_INVALID'
    }
    $certificateAt = [BitConverter]::ToUInt32($bytes, $securityAt)
    $certificateSize = [BitConverter]::ToUInt32($bytes, $securityAt + 4)
    if (($certificateAt -eq 0) -ne ($certificateSize -eq 0) -or
        [long]$certificateAt + $certificateSize -gt $bytes.Length) { Stop-Signing 'PE_CERTIFICATE_INVALID' }
    $characteristics = [BitConverter]::ToUInt16($bytes, [int]$pe + 22)
    $subsystem = [BitConverter]::ToUInt16($bytes, $optional + 68)
    $kind = if ($subsystem -eq 1) { 'SYS' } elseif (($characteristics -band 0x2000) -ne 0) { 'DLL' } else { 'EXE' }
    return [pscustomobject]@{
        Kind = $kind; Machine = [int][BitConverter]::ToUInt16($bytes, [int]$pe + 4)
        Magic = [int]$magic; Characteristics = [int]$characteristics; Subsystem = [int]$subsystem
        ChecksumAt = $optional + 64; SecurityAt = $securityAt
        CertificateAt = [long]$certificateAt; CertificateSize = [long]$certificateSize; Length = [long]$bytes.Length
    }
}

function Test-CertificateEku($Certificate, [string]$Oid) {
    if ($null -eq $Certificate) { return $false }
    foreach ($extension in $Certificate.Extensions) {
        if ($extension.Oid.Value -eq '2.5.29.37') {
            $eku = [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($extension, $extension.Critical)
            foreach ($usage in $eku.EnhancedKeyUsages) { if ($usage.Value -eq $Oid) { return $true } }
        }
    }
    return $false
}

function Get-CommonName($Certificate) {
    return $Certificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
}

function Get-StatusClass($Signature) {
    # The class of WinTrust's message (English on the runners), never the message itself, which
    # can name the file's path.
    $message = [string]$Signature.StatusMessage
    $classes = [ordered]@{
        'UNTRUSTEDROOT' = 'root certificate which is not trusted'
        'REVOKED' = 'revoked'
        'REVOCATIONUNKNOWN' = 'revocation'
        'EXPIRED' = 'validity period|expired'
        'HASHMISMATCH' = 'hash'
        'NOSIGNATURE' = 'no signature'
        'UNTRUSTEDPUBLISHER' = 'publisher'
        'WRONGUSAGE' = 'usage|purpose'
    }
    foreach ($class in $classes.Keys) {
        if ($message -match $classes[$class]) { return $class }
    }
    return 'OTHER'
}

function Get-CheckedSignature([string]$Path, $Pe) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    $status = $signature.Status.ToString()
    if ($status -eq 'Valid') {
        if ($signature.SignatureType.ToString() -notin @('Authenticode', 'Catalog') -or
            -not (Test-CertificateEku $signature.SignerCertificate $script:CodeSigningOid)) {
            Stop-Signing 'SIGNER_CODE_SIGNING_EKU_REQUIRED'
        }
        return $signature
    }
    if ($status -eq 'NotSigned' -and $null -ne $Pe -and $Pe.CertificateSize -eq 0) { return $signature }
    # Never repair an invalid/revoked/unknown signature by replacing it. A vendor file that ships
    # with such a signature on purpose goes into the pin file instead.
    Stop-Signing ('EXISTING_SIGNATURE_' + ($status -replace '[^A-Za-z0-9]', '').ToUpperInvariant()) @(Get-StatusClass $signature)
}

function Get-Disposition($File, [string]$Sha, $Pe) {
    if ($script:Pins.ContainsKey($File.Relative)) {
        if ($script:Pins[$File.Relative] -cne $Sha) { Stop-Signing 'PINNED_FILE_CHANGED' }
        return [pscustomobject]@{ Disposition = 'Pinned'; SignatureStatus = $null }
    }
    if ($null -eq $Pe -and [IO.Path]::GetExtension($File.FullName) -ine '.cat') {
        return [pscustomobject]@{ Disposition = 'Data'; SignatureStatus = $null }
    }
    $status = (Get-CheckedSignature $File.FullName $Pe).Status.ToString()
    if ($status -eq 'Valid') {
        # Only a signature embedded in the file travels with it. WinTrust also calls a file valid
        # whose hash is in one of this runner's catalogs, which the users' PCs need not have.
        if ($null -eq $Pe) { Stop-Signing 'CATALOG_FILE_NOT_PINNED' }
        if ($Pe.CertificateSize -le 0) { Stop-Signing 'CATALOG_ONLY_SIGNATURE' }
        return [pscustomobject]@{ Disposition = 'Preserve'; SignatureStatus = $status }
    }
    # A kernel driver needs Microsoft's signature, which a code signing certificate cannot give.
    if ($Pe.Kind -eq 'SYS') { Stop-Signing 'DRIVER_NOT_SIGNED' }
    return [pscustomobject]@{ Disposition = 'Sign'; SignatureStatus = $status }
}

function Get-Inventory([string]$Directory) {
    $result = [Collections.Generic.List[object]]::new()
    foreach ($relative in $script:Pins.Keys) { $null = Get-SafeChild $Directory $relative }
    $pinned = 0
    $index = 0
    foreach ($file in @(Get-TreeFiles $Directory)) {
        $sha = Get-FileSha $file.FullName
        $pe = Get-PeInfo $file.FullName
        $state = Get-Disposition $file $sha $pe
        $stageName = $null
        if ($state.Disposition -eq 'Pinned') { $pinned++ }
        if ($state.Disposition -eq 'Sign') {
            # Flat staging under numbered names: the signing step signs one folder without
            # recursion, and a PE extension lets Windows inspect originally extensionless PEs.
            $stageName = ('{0:D4}.{1}' -f $index, $pe.Kind.ToLowerInvariant())
            $index++
        }
        $result.Add([pscustomobject]@{
            Relative = $file.Relative; SHA256 = $sha; Disposition = $state.Disposition
            SignatureStatus = $state.SignatureStatus; Pe = $pe; StageName = $stageName
        })
    }
    if ($pinned -ne $script:Pins.Count) { Stop-Signing 'PINNED_FILE_MISSING' }
    if (@($result | Where-Object { $null -ne $_.Pe }).Count -eq 0) { Stop-Signing 'NO_PE_FILES' }
    return $result.ToArray()
}

function Write-SigningJson([string]$Path, $Value) {
    Assert-NoReparse $Path
    if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($Path)) -PathType Container)) {
        Stop-Signing 'OUTPUT_PARENT_MISSING'
    }
    [IO.File]::WriteAllText($Path, (ConvertTo-Json -InputObject $Value -Depth 12) + "`n", [Text.UTF8Encoding]::new($false))
}

function Assert-PeUnchanged([string]$Original, [string]$Result) {
    $old = Get-PeInfo $Original; $new = Get-PeInfo $Result
    if ($null -eq $old -or $null -eq $new -or $old.CertificateSize -ne 0) { Stop-Signing 'PE_COMPARISON_INVALID' }
    foreach ($field in @('Kind', 'Machine', 'Magic', 'Characteristics', 'Subsystem', 'ChecksumAt', 'SecurityAt')) {
        if ($old.$field -ne $new.$field) { Stop-Signing 'PE_IDENTITY_CHANGED' }
    }
    $aligned = [long]([Math]::Ceiling($old.Length / 8.0) * 8)
    if ($new.CertificateAt -ne $aligned -or $new.CertificateSize -lt 8 -or
        $new.CertificateAt + $new.CertificateSize -ne $new.Length) { Stop-Signing 'PE_CERTIFICATE_LAYOUT_CHANGED' }
    $before = [IO.File]::ReadAllBytes($Original); $after = [IO.File]::ReadAllBytes($Result)
    for ($i = [int]$old.Length; $i -lt $aligned; $i++) {
        if ($after[$i] -ne 0) { Stop-Signing 'PE_PADDING_CHANGED' }
    }
    # Only the checksum and certificate directory may change in the old image.
    # Hash the entire normalized prefix: section contents, resources and overlays
    # must be byte-identical, not merely have the same version/name/PE header.
    [Array]::Clear($before, $old.ChecksumAt, 4); [Array]::Clear($after, $new.ChecksumAt, 4)
    [Array]::Clear($before, $old.SecurityAt, 8); [Array]::Clear($after, $new.SecurityAt, 8)
    $hasher = [Security.Cryptography.SHA256]::Create()
    try {
        $a = [Convert]::ToBase64String($hasher.ComputeHash($before))
        $b = [Convert]::ToBase64String($hasher.ComputeHash($after, 0, $before.Length))
        if ($a -cne $b) { Stop-Signing 'PE_CONTENT_CHANGED' }
    } finally { $hasher.Dispose() }
}

function Get-DerValue([byte[]]$Blob) {
    # The PKCS#7 value of a certificate table entry, without the zero padding after it.
    if ($Blob.Length -lt 2 -or $Blob[0] -ne 0x30) { Stop-Signing 'SIGNATURE_UNREADABLE' }
    $first = [int]$Blob[1]
    if ($first -lt 0x80) {
        $size = [long]$first; $header = 2
    } else {
        $count = $first -band 0x7f
        if ($count -lt 1 -or $count -gt 4 -or $Blob.Length -lt 2 + $count) { Stop-Signing 'SIGNATURE_UNREADABLE' }
        $size = [long]0
        for ($i = 0; $i -lt $count; $i++) { $size = $size * 256 + [long]$Blob[2 + $i] }
        $header = 2 + $count
    }
    $total = [long]$header + $size
    if ($total -gt $Blob.Length -or $Blob.Length - $total -gt 7) { Stop-Signing 'SIGNATURE_UNREADABLE' }
    for ($i = [int]$total; $i -lt $Blob.Length; $i++) {
        if ($Blob[$i] -ne 0) { Stop-Signing 'SIGNATURE_TRAILING_DATA' }
    }
    $value = [byte[]]::new([int]$total)
    [Array]::Copy($Blob, 0, $value, 0, [int]$total)
    return , $value
}

function Get-FileDigestOid([byte[]]$Content) {
    # SpcIndirectDataContent ::= SEQUENCE { data SpcAttributeTypeAndOptionalValue,
    #   messageDigest DigestInfo }; DigestInfo ::= SEQUENCE { digestAlgorithm, digest }
    $rules = [Formats.Asn1.AsnEncodingRules]::BER
    $options = [Formats.Asn1.AsnReaderOptions]::new()
    $reader = [Formats.Asn1.AsnReader]::new([ReadOnlyMemory[byte]]::new($Content), $rules, $options)
    if ($reader.PeekTag().TagValue -eq 4) {
        # some decoders keep the content wrapped in an OCTET STRING
        $reader = [Formats.Asn1.AsnReader]::new([ReadOnlyMemory[byte]]::new($reader.ReadOctetString($null)), $rules, $options)
    }
    $indirect = $reader.ReadSequence($null)
    $null = $indirect.ReadEncodedValue()
    $digestInfo = $indirect.ReadSequence($null)
    $algorithm = $digestInfo.ReadSequence($null)
    return $algorithm.ReadObjectIdentifier($null)
}

function Read-SignatureCms([string]$Path, $Pe) {
    # The first WIN_CERTIFICATE entry of a signed PE, decoded as PKCS#7 SignedData, its file
    # digest algorithm, and whether more entries follow it.
    if ($null -eq $Pe -or $Pe.CertificateSize -lt 16) { Stop-Signing 'PE_CERTIFICATE_INVALID' }
    $bytes = [IO.File]::ReadAllBytes($Path)
    $at = [int]$Pe.CertificateAt
    $length = [long][BitConverter]::ToUInt32($bytes, $at)
    $revision = [BitConverter]::ToUInt16($bytes, $at + 4)
    $type = [BitConverter]::ToUInt16($bytes, $at + 6)
    if ($revision -ne 0x0200 -or $type -ne 0x0002 -or $length -le 8 -or $length -gt $Pe.CertificateSize) {
        Stop-Signing 'PE_CERTIFICATE_INVALID'
    }
    # Entries are padded to eight bytes; anything after the first one is a second signature.
    $more = [long]([Math]::Ceiling($length / 8.0) * 8) -ne $Pe.CertificateSize
    $blob = [byte[]]::new([int]$length - 8)
    [Array]::Copy($bytes, $at + 8, $blob, 0, $blob.Length)
    $der = Get-DerValue $blob
    $cms = [Security.Cryptography.Pkcs.SignedCms]::new()
    $fileDigest = $null
    try {
        $cms.Decode($der)
        if ($cms.ContentInfo.ContentType.Value -eq $script:SpcIndirectDataOid) {
            $fileDigest = Get-FileDigestOid $cms.ContentInfo.Content
        }
    } catch { Stop-Signing 'SIGNATURE_UNREADABLE' }
    return [pscustomobject]@{ Cms = $cms; FileDigest = $fileDigest; MoreEntries = $more }
}

function Get-SignatureStructure([string]$Path, $Pe, $TimeStamper) {
    # What WinTrust's "Valid" does not say: one certificate table entry with one signer, SHA-256
    # file and signer digests, no appended (nested) signature, and one RFC 3161 timestamp whose
    # imprint is this signature. Returns the signer certificate, the timestamp and the
    # signature's own certificates (candidates for the chain's intermediates).
    $read = Read-SignatureCms $Path $Pe
    if ($read.MoreEntries) { Stop-Signing 'SECOND_CERTIFICATE_ENTRY' }
    $cms = $read.Cms
    if ($cms.ContentInfo.ContentType.Value -ne $script:SpcIndirectDataOid) { Stop-Signing 'NOT_AUTHENTICODE' }
    if ($cms.SignerInfos.Count -ne 1) { Stop-Signing 'ONE_SIGNER_REQUIRED' }
    $signer = $cms.SignerInfos[0]
    if ($read.FileDigest -ne $script:Sha256Oid -or $signer.DigestAlgorithm.Value -ne $script:Sha256Oid) {
        Stop-Signing 'FILE_DIGEST_NOT_SHA256'
    }
    if ($null -eq $signer.Certificate) { Stop-Signing 'SIGNER_CERTIFICATE_MISSING' }
    $tokens = [Collections.Generic.List[byte[]]]::new()
    foreach ($attribute in $signer.UnsignedAttributes) {
        if ($attribute.Oid.Value -eq $script:NestedSignatureOid) { Stop-Signing 'NESTED_SIGNATURE' }
        if ($attribute.Oid.Value -eq $script:Rfc3161TimestampOid) {
            foreach ($value in $attribute.Values) { $tokens.Add($value.RawData) }
        }
    }
    # A legacy Authenticode countersignature is not enough: Artifact Signing's timestamp
    # service answers RFC 3161 requests only.
    if ($tokens.Count -eq 0) { Stop-Signing 'RFC3161_TIMESTAMP_REQUIRED' }
    if ($tokens.Count -gt 1) { Stop-Signing 'RFC3161_TIMESTAMP_INVALID' @('MORETHANONE') }
    $token = $null
    $consumed = 0
    $decoded = [Security.Cryptography.Pkcs.Rfc3161TimestampToken]::TryDecode(
        [ReadOnlyMemory[byte]]::new($tokens[0]), [ref]$token, [ref]$consumed)
    if (-not $decoded) { Stop-Signing 'RFC3161_TIMESTAMP_INVALID' @('UNREADABLE') }
    if ($consumed -ne $tokens[0].Length) { Stop-Signing 'RFC3161_TIMESTAMP_INVALID' @('TRAILINGDATA') }
    if ($token.TokenInfo.HashAlgorithmId.Value -ne $script:Sha256Oid) { Stop-Signing 'TIMESTAMP_DIGEST_NOT_SHA256' }
    # The timestamp is over this signature's value, then signed by its authority.
    $imprint = [Security.Cryptography.SHA256]::HashData($signer.GetSignature())
    if ([Convert]::ToBase64String($imprint) -cne [Convert]::ToBase64String($token.TokenInfo.GetMessageHash().ToArray())) {
        Stop-Signing 'RFC3161_TIMESTAMP_INVALID' @('IMPRINTMISMATCH')
    }
    $extra = [Security.Cryptography.X509Certificates.X509Certificate2Collection]::new()
    if ($null -ne $TimeStamper) { $null = $extra.Add($TimeStamper) }
    $authority = $null
    if (-not $token.VerifySignatureForSignerInfo($signer, [ref]$authority, $extra)) {
        Stop-Signing 'RFC3161_TIMESTAMP_INVALID' @('AUTHORITYSIGNATURE')
    }
    $time = $token.TokenInfo.Timestamp.UtcDateTime
    if ($time -lt $signer.Certificate.NotBefore.ToUniversalTime()) { Stop-Signing 'TIMESTAMP_OUTSIDE_CERTIFICATE' @('BEFORE') }
    if ($time -gt $signer.Certificate.NotAfter.ToUniversalTime()) { Stop-Signing 'TIMESTAMP_OUTSIDE_CERTIFICATE' @('AFTER') }
    return [pscustomobject]@{ Certificate = $signer.Certificate; TimestampUtc = $time; Certificates = $cms.Certificates }
}

function Assert-SignerChain($Leaf, $Extra, [string]$ChainCA) {
    # Get-AuthenticodeSignature validates the file and timestamp with WinTrust. Also require an
    # online, currently valid code-signing chain in the clean Windows runner's normal trust store
    # that runs through ChainCA, with the signature's own certificates as candidates for the
    # intermediates. A revocation server can be slow or briefly unreachable, so the chain is built
    # up to three times before the build fails with its status flags. Never install test roots on
    # the runner.
    $causes = @()
    $script:LastChainAttempts = 0
    for ($attempt = 0; $attempt -le $script:ChainRetrySeconds.Count; $attempt++) {
        if ($attempt -gt 0) { Start-Sleep -Seconds $script:ChainRetrySeconds[$attempt - 1] }
        $script:LastChainAttempts++
        $chain = [Security.Cryptography.X509Certificates.X509Chain]::new()
        try {
            $chain.ChainPolicy.RevocationMode = [Security.Cryptography.X509Certificates.X509RevocationMode]::Online
            $chain.ChainPolicy.RevocationFlag = [Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
            $chain.ChainPolicy.VerificationFlags = [Security.Cryptography.X509Certificates.X509VerificationFlags]::NoFlag
            $chain.ChainPolicy.UrlRetrievalTimeout = [TimeSpan]::FromSeconds(30)
            $null = $chain.ChainPolicy.ApplicationPolicy.Add([Security.Cryptography.Oid]::new($script:CodeSigningOid))
            if ($null -ne $Extra) { $chain.ChainPolicy.ExtraStore.AddRange($Extra) }
            if ($chain.Build($Leaf)) {
                if ($chain.ChainElements.Count -lt 2) { Stop-Signing 'SELF_SIGNED_SIGNER' }
                for ($i = 1; $i -lt $chain.ChainElements.Count; $i++) {
                    if ((Get-CommonName $chain.ChainElements[$i].Certificate) -ceq $ChainCA) { return }
                }
                Stop-Signing 'ARTIFACT_SIGNING_CHAIN_REQUIRED'
            }
            $causes = @($chain.ChainStatus | ForEach-Object { $_.Status.ToString() })
        } finally { $chain.Dispose() }
    }
    Stop-Signing 'PUBLIC_SIGNER_CHAIN_INVALID' $causes
}

function Assert-NewSignature([string]$Path, [string]$Publisher, [string]$ChainCA = $ExpectedChainCA) {
    $pe = Get-PeInfo $Path
    $signature = Get-CheckedSignature $Path $pe
    if ($signature.Status.ToString() -ne 'Valid' -or $signature.SignatureType.ToString() -ne 'Authenticode' -or
        $null -eq $pe -or $pe.CertificateSize -le 0) {
        Stop-Signing 'NEW_AUTHENTICODE_SIGNATURE_REQUIRED' @($signature.SignatureType.ToString())
    }
    $leaf = $signature.SignerCertificate
    if ($leaf.Subject -ceq $leaf.Issuer) { Stop-Signing 'SELF_SIGNED_SIGNER' }
    if ((Get-CommonName $leaf) -cne $Publisher) { Stop-Signing 'UNEXPECTED_PUBLISHER' }
    if ($null -eq $signature.TimeStamperCertificate) { Stop-Signing 'TRUSTED_TIMESTAMP_REQUIRED' @('NOTIMESTAMPER') }
    if (-not (Test-CertificateEku $signature.TimeStamperCertificate $script:TimeStampingOid)) {
        Stop-Signing 'TRUSTED_TIMESTAMP_REQUIRED' @('NOTIMESTAMPINGUSAGE')
    }
    $structure = Get-SignatureStructure $Path $pe $signature.TimeStamperCertificate
    if ($structure.Certificate.Thumbprint -ine $leaf.Thumbprint) { Stop-Signing 'SIGNER_MISMATCH' }
    Assert-SignerChain $leaf $structure.Certificates $ChainCA
    return [pscustomobject]@{
        Status = 'Valid'; SignerThumbprint = $leaf.Thumbprint.ToUpperInvariant()
        CertificateNotAfterUtc = $leaf.NotAfter.ToUniversalTime().ToString('o')
        TimestampUtc = $structure.TimestampUtc.ToString('o'); FileDigest = 'SHA256'
        WindowsTrustValidated = $true; ChainAttempts = $script:LastChainAttempts
    }
}

function Get-KeptSignatureWarnings([string]$Directory, $Inventory) {
    # Vendor signatures kept as they are that a later Windows may trust less: not SHA-256, or
    # without an RFC 3161 timestamp (valid only while the vendor's certificate is). Warnings only.
    $warnings = [Collections.Generic.List[string]]::new()
    foreach ($record in @($Inventory | Where-Object { $_.Disposition -eq 'Preserve' -and $null -ne $_.Pe })) {
        $path = Get-SafeChild $Directory $record.Relative
        $notes = @()
        try {
            $read = Read-SignatureCms $path $record.Pe
            $signers = @($read.Cms.SignerInfos)
            $weak = @($signers | Where-Object { $_.DigestAlgorithm.Value -ne $script:Sha256Oid })
            if ($read.FileDigest -ne $script:Sha256Oid -or $weak.Count -gt 0) { $notes += 'its signature is not SHA-256' }
            $stamped = $false
            foreach ($signer in $signers) {
                foreach ($attribute in $signer.UnsignedAttributes) {
                    if ($attribute.Oid.Value -eq $script:Rfc3161TimestampOid) { $stamped = $true }
                }
            }
            if (-not $stamped) { $notes += 'its signature has no RFC 3161 timestamp' }
        } catch { $notes += 'its signature could not be read' }
        foreach ($note in $notes) { $warnings.Add("$($record.Relative): kept vendor signature, but $note") }
    }
    # unrolled: callers collect the lines with @()
    return $warnings.ToArray()
}

function Assert-OneSigner($Checks) {
    # One batch, one certificate: Artifact Signing renews daily, so a batch signed across a
    # renewal fails here and is simply signed again.
    $thumbprints = @{}
    foreach ($check in @($Checks)) { $thumbprints[$check.SignerThumbprint] = $true }
    if ($thumbprints.Count -gt 1) { Stop-Signing 'MIXED_SIGNERS' }
}

function Assert-ManifestFiles($State, [string]$Directory) {
    if ($State.Schema -ne $script:ManifestSchema -or $State.Root -ine $Directory) { Stop-Signing 'MANIFEST_ROOT_MISMATCH' }
    $actual = @(Get-TreeFiles $Directory)
    $records = @($State.Files)
    if ($records.Count -ne $actual.Count) { Stop-Signing 'SOURCE_FILE_SET_CHANGED' }
    $map = @{}
    foreach ($record in $records) {
        $null = Get-SafeChild $Directory $record.Relative
        if ($record.SHA256 -notmatch '^[0-9a-f]{64}$' -or $map.ContainsKey($record.Relative) -or
            $record.Disposition -notin @('Data', 'Sign', 'Preserve', 'Pinned')) { Stop-Signing 'MANIFEST_INVALID' }
        $map[$record.Relative] = $record
    }
    foreach ($file in $actual) {
        if (-not $map.ContainsKey($file.Relative) -or (Get-FileSha $file.FullName) -cne $map[$file.Relative].SHA256) {
            Stop-Signing 'SOURCE_CONTENT_CHANGED'
        }
        # Re-evaluate every file, including preserved vendor signatures and pins.
        $record = $map[$file.Relative]; $pe = Get-PeInfo $file.FullName
        if (($null -ne $pe) -ne ($null -ne $record.Pe)) { Stop-Signing 'MANIFEST_PE_MISMATCH' }
        $now = Get-Disposition $file $record.SHA256 $pe
        if ($record.Disposition -ne $now.Disposition) { Stop-Signing 'MANIFEST_SIGNATURE_MISMATCH' }
    }
    foreach ($relative in $script:Pins.Keys) {
        if (-not $map.ContainsKey($relative) -or $map[$relative].Disposition -ne 'Pinned') { Stop-Signing 'PINNED_FILE_MISSING' }
    }
}

function Apply-VerifiedFile([string]$Source, [string]$Destination, [string]$ExpectedSha) {
    # Called only after the entire batch has passed PE/signature verification.
    Assert-NoReparse $Source
    Assert-NoReparse $Destination
    if ((Get-FileSha $Source) -cne $ExpectedSha) { Stop-Signing 'SIGNED_CONTENT_CHANGED' }
    $temporary = Join-Path ([IO.Path]::GetDirectoryName($Destination)) ([guid]::NewGuid().ToString() + '.signed-tmp')
    try {
        [IO.File]::Copy($Source, $temporary, $false)
        if ((Get-FileSha $temporary) -cne $ExpectedSha) { Stop-Signing 'COPY_HASH_MISMATCH' }
        # PowerShell converts $null to an empty string for string parameters.
        # Replace requires an actual null to omit a backup filename.
        [IO.File]::Replace($temporary, $Destination, [NullString]::Value)
        if ((Get-FileSha $Destination) -cne $ExpectedSha) { Stop-Signing 'APPLIED_HASH_MISMATCH' }
    } finally {
        if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
    }
}

function Invoke-WindowsSigning {
    if ($env:OS -ne 'Windows_NT') { Stop-Signing 'WINDOWS_REQUIRED' }
    # SignedCms, Rfc3161TimestampToken and AsnReader come with .NET, which PowerShell 7 runs on.
    if ($PSVersionTable.PSVersion.Major -lt 7) { Stop-Signing 'POWERSHELL_7_REQUIRED' }
    $rootPath = Get-FullSigningPath $Root
    Assert-NoReparse $rootPath
    $stagePath = if ($Stage) { Get-FullSigningPath $Stage } else { $null }
    $signedPath = if ($Signed) { Get-FullSigningPath $Signed } else { $null }
    $manifestPath = if ($Manifest) { Get-FullSigningPath $Manifest } else { $null }
    $reportPath = if ($Report) { Get-FullSigningPath $Report } else { $null }
    $pinPath = if ($PinFile) { Get-FullSigningPath $PinFile } else { $null }
    $directories = @($rootPath)
    foreach ($path in @($stagePath, $signedPath)) {
        if ($path) {
            Assert-NoReparse $path
            foreach ($existing in $directories) { Assert-Separate $path $existing }
            $directories += $path
        }
    }
    foreach ($path in @($manifestPath, $reportPath, $pinPath)) {
        if ($path) {
            Assert-NoReparse $path
            foreach ($directory in $directories) { Assert-Separate $path $directory }
        }
    }
    if ($manifestPath -and $reportPath -and $manifestPath -ieq $reportPath) { Stop-Signing 'OVERLAPPING_PATHS' }
    $script:ValidatedReportPath = $reportPath
    $script:Pins = if ($pinPath) { Read-Pins $pinPath } else { @{} }
    $result = $null
    if ($Mode -in @('Preflight', 'Prepare', 'Audit')) {
        $inventory = @(Get-Inventory $rootPath)
        $toSign = @($inventory | Where-Object Disposition -EQ 'Sign')
        if ($toSign.Count -gt 100) { Stop-Signing 'SIGNING_BATCH_LIMIT' }
        if ($Mode -eq 'Audit' -and $toSign.Count -ne 0) { Stop-Signing 'UNSIGNED_RELEASE' }
        $warnings = @()
        if ($Mode -in @('Preflight', 'Audit')) {
            $warnings = @(Get-KeptSignatureWarnings $rootPath $inventory)
            # GitHub turns these lines into annotations; they name relative paths only
            foreach ($warning in $warnings) { Write-Host ('::warning::' + $warning) }
        }
        if ($Mode -eq 'Prepare') {
            if (-not $stagePath -or -not $manifestPath) { Stop-Signing 'PREPARE_PATHS_REQUIRED' }
            if ((Test-Path -LiteralPath $stagePath) -or (Test-Path -LiteralPath $manifestPath)) { Stop-Signing 'OUTPUT_ALREADY_EXISTS' }
            if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($manifestPath)) -PathType Container)) {
                Stop-Signing 'OUTPUT_PARENT_MISSING'
            }
            $null = New-Item -ItemType Directory -Path $stagePath
            foreach ($record in $toSign) {
                $source = Get-SafeChild $rootPath $record.Relative
                if ((Get-FileSha $source) -cne $record.SHA256) { Stop-Signing 'SOURCE_CONTENT_CHANGED' }
                [IO.File]::Copy($source, (Join-Path $stagePath $record.StageName), $false)
            }
            $state = [pscustomobject]@{ Schema = $script:ManifestSchema; Root = $rootPath; Files = $inventory }
            Write-SigningJson $manifestPath $state
            if ($env:GITHUB_OUTPUT) {
                [IO.File]::AppendAllText($env:GITHUB_OUTPUT, ('pending_count=' + $toSign.Count + "`n"), [Text.UTF8Encoding]::new($false))
            }
        }
        $result = [pscustomobject]@{
            Schema = $script:ManifestSchema; Mode = $Mode; Success = $true; Files = $inventory
            SignCount = $toSign.Count; PreservedCount = @($inventory | Where-Object Disposition -EQ 'Preserve').Count
            PinnedCount = @($inventory | Where-Object Disposition -EQ 'Pinned').Count; Warnings = $warnings
        }
    } else {
        if (-not $signedPath -or -not $manifestPath) { Stop-Signing 'VERIFY_PATHS_REQUIRED' }
        $publisher = if ($ExpectedPublisher) { $ExpectedPublisher.Trim() } else { '' }
        if (-not $publisher -or $publisher -match '[\x00-\x1f]') { Stop-Signing 'EXPECTED_PUBLISHER_REQUIRED' }
        if ([string]::IsNullOrWhiteSpace($ExpectedChainCA)) { Stop-Signing 'EXPECTED_CHAIN_CA_REQUIRED' }
        $state = ConvertFrom-Json ([IO.File]::ReadAllText($manifestPath))
        Assert-ManifestFiles $state $rootPath
        $expected = @{}
        foreach ($record in @($state.Files | Where-Object Disposition -EQ 'Sign')) {
            if ($record.StageName -notmatch '^[0-9]{4}\.(exe|dll)$' -or $expected.ContainsKey($record.StageName)) {
                Stop-Signing 'MANIFEST_STAGE_INVALID'
            }
            $expected[$record.StageName] = $record
        }
        $signedFiles = if ($expected.Count -eq 0 -and -not (Test-Path -LiteralPath $signedPath)) { @() } else { @(Get-TreeFiles $signedPath) }
        $signedFiles = @($signedFiles)
        if ($signedFiles.Count -ne $expected.Count) { Stop-Signing 'SIGNED_FILE_SET_MISMATCH' }
        $verified = [Collections.Generic.List[object]]::new()
        foreach ($file in $signedFiles) {
            if (-not $expected.ContainsKey($file.Relative)) { Stop-Signing 'SIGNED_FILE_SET_MISMATCH' }
            $record = $expected[$file.Relative]
            $source = Get-SafeChild $rootPath $record.Relative
            Assert-PeUnchanged $source $file.FullName
            $check = Assert-NewSignature $file.FullName $publisher
            $verified.Add([pscustomobject]@{
                Relative = $record.Relative; StageName = $record.StageName
                SHA256 = (Get-FileSha $file.FullName); Signature = $check
            })
        }
        Assert-OneSigner @($verified | ForEach-Object { $_.Signature })
        # No source changes occur until the complete batch has passed. Replace
        # files, rather than truncating them, so a hard link cannot redirect writes.
        Assert-ManifestFiles $state $rootPath
        foreach ($record in $verified) {
            $source = Get-SafeChild $signedPath $record.StageName
            $destination = Get-SafeChild $rootPath $record.Relative
            Apply-VerifiedFile $source $destination $record.SHA256
        }
        foreach ($record in @($state.Files)) {
            $path = Get-SafeChild $rootPath $record.Relative
            if ($record.Disposition -ne 'Sign' -and (Get-FileSha $path) -cne $record.SHA256) { Stop-Signing 'PRESERVED_CONTENT_CHANGED' }
        }
        $result = [pscustomobject]@{
            Schema = $script:ManifestSchema; Mode = 'Verify'; Success = $true; SignedCount = $verified.Count
            PreservedCount = @($state.Files | Where-Object Disposition -EQ 'Preserve').Count
            PinnedCount = @($state.Files | Where-Object Disposition -EQ 'Pinned').Count; Files = $verified.ToArray()
        }
    }
    if ($reportPath) { Write-SigningJson $reportPath $result }
    # Reports contain hashes, relative paths, public certificate thumbprints and times; never
    # certificates, names, environment variables, service responses or private keys.
    ConvertTo-Json -InputObject $result -Depth 12
}

if ($MyInvocation.InvocationName -ne '.') {
    try {
        Invoke-WindowsSigning
        $global:LASTEXITCODE = 0
    }
    catch {
        $code = 'CHECK_FAILED'
        if ($_.Exception.Message -match '^Windows signing check failed: ([A-Z0-9_]+)$') { $code = $Matches[1] }
        $failure = @{ Success = $false; Mode = $Mode; Error = $code }
        if ($script:ValidatedReportPath) {
            try { Write-SigningJson $script:ValidatedReportPath $failure } catch { }
        }
        [Console]::Error.WriteLine((ConvertTo-Json -Compress -InputObject $failure))
        exit 1
    }
}
