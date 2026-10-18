# Windows release signing boundary. This script never contacts a signing service.
# Prepare copies unsigned PE images to a flat input directory; Verify accepts a
# separate service output directory and changes Root only after every check passes.
# Run with Windows PowerShell 5.1 or PowerShell 7 on Windows.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Preflight', 'Prepare', 'Verify', 'Audit')][string]$Mode,
    [Parameter(Mandatory = $true)][string]$Root,
    [string]$Stage,
    [string]$Signed,
    [string]$Manifest,
    [string]$Report,
    [string]$ExpectedSignerThumbprint
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$script:CodeSigningOid = '1.3.6.1.5.5.7.3.3'
$script:TimeStampingOid = '1.3.6.1.5.5.7.3.8'
$script:ValidatedReportPath = $null

function Stop-Signing([string]$Code) {
    # Do not forward provider errors: they can contain credentials or machine paths.
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
    # Never repair an invalid/revoked/unknown signature by replacing it.
    Stop-Signing ('EXISTING_SIGNATURE_' + ($status -replace '[^A-Za-z0-9]', '').ToUpperInvariant())
}

function Get-Inventory([string]$Directory) {
    $result = [Collections.Generic.List[object]]::new()
    $index = 0
    foreach ($file in @(Get-TreeFiles $Directory)) {
        $sha = Get-FileSha $file.FullName
        $pe = Get-PeInfo $file.FullName
        $disposition = 'Data'; $signatureStatus = $null; $stageName = $null
        if ($null -ne $pe -or [IO.Path]::GetExtension($file.FullName) -ieq '.cat') {
            $signature = Get-CheckedSignature $file.FullName $pe
            $signatureStatus = $signature.Status.ToString()
            $disposition = if ($signatureStatus -eq 'Valid') { 'Preserve' } else { 'Sign' }
            if ($disposition -eq 'Sign') {
                # Flat staging is required by eSigner batch_sign; a PE extension
                # also lets the Windows SIP inspect originally extensionless PEs.
                $stageName = ('{0:D4}.{1}' -f $index, $pe.Kind.ToLowerInvariant())
                $index++
            }
        }
        $result.Add([pscustomobject]@{
            Relative = $file.Relative; SHA256 = $sha; Disposition = $disposition
            SignatureStatus = $signatureStatus; Pe = $pe; StageName = $stageName
        })
    }
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

function Assert-NewSignature([string]$Path, [string]$Signer) {
    $pe = Get-PeInfo $Path
    $signature = Get-CheckedSignature $Path $pe
    if ($signature.Status.ToString() -ne 'Valid' -or $signature.SignatureType.ToString() -ne 'Authenticode') {
        Stop-Signing 'NEW_AUTHENTICODE_SIGNATURE_REQUIRED'
    }
    if ($signature.SignerCertificate.Thumbprint -ine $Signer) { Stop-Signing 'UNEXPECTED_SIGNER' }
    if ($signature.SignerCertificate.Subject -ceq $signature.SignerCertificate.Issuer) { Stop-Signing 'SELF_SIGNED_SIGNER' }
    if (-not (Test-CertificateEku $signature.TimeStamperCertificate $script:TimeStampingOid)) {
        Stop-Signing 'TRUSTED_TIMESTAMP_REQUIRED'
    }
    # Get-AuthenticodeSignature validates the file and timestamp with WinTrust.
    # Also require an online, currently valid code-signing chain in the clean
    # Windows runner's normal trust store. This does not independently audit the
    # Microsoft Trusted Root Program list. Never install test roots on the runner.
    $chain = [Security.Cryptography.X509Certificates.X509Chain]::new()
    try {
        $chain.ChainPolicy.RevocationMode = [Security.Cryptography.X509Certificates.X509RevocationMode]::Online
        $chain.ChainPolicy.RevocationFlag = [Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
        $chain.ChainPolicy.VerificationFlags = [Security.Cryptography.X509Certificates.X509VerificationFlags]::NoFlag
        $chain.ChainPolicy.UrlRetrievalTimeout = [TimeSpan]::FromSeconds(30)
        $null = $chain.ChainPolicy.ApplicationPolicy.Add([Security.Cryptography.Oid]::new($script:CodeSigningOid))
        if (-not $chain.Build($signature.SignerCertificate)) { Stop-Signing 'PUBLIC_SIGNER_CHAIN_INVALID' }
        if ($chain.ChainElements.Count -lt 2) { Stop-Signing 'SELF_SIGNED_SIGNER' }
    } finally { $chain.Dispose() }
    return [pscustomobject]@{
        Status = 'Valid'; SignerThumbprint = $signature.SignerCertificate.Thumbprint
        Timestamp = $true; WindowsTrustValidated = $true
    }
}

function Assert-ManifestFiles($State, [string]$Directory) {
    if ($State.Schema -ne 1 -or $State.Root -ine $Directory) { Stop-Signing 'MANIFEST_ROOT_MISMATCH' }
    $actual = @(Get-TreeFiles $Directory)
    $records = @($State.Files)
    if ($records.Count -ne $actual.Count) { Stop-Signing 'SOURCE_FILE_SET_CHANGED' }
    $map = @{}
    foreach ($record in $records) {
        $null = Get-SafeChild $Directory $record.Relative
        if ($record.SHA256 -notmatch '^[0-9a-f]{64}$' -or $map.ContainsKey($record.Relative) -or
            $record.Disposition -notin @('Data', 'Sign', 'Preserve')) { Stop-Signing 'MANIFEST_INVALID' }
        $map[$record.Relative] = $record
    }
    foreach ($file in $actual) {
        if (-not $map.ContainsKey($file.Relative) -or (Get-FileSha $file.FullName) -cne $map[$file.Relative].SHA256) {
            Stop-Signing 'SOURCE_CONTENT_CHANGED'
        }
        # Re-evaluate every signature, including preserved vendor signatures.
        $record = $map[$file.Relative]; $pe = Get-PeInfo $file.FullName
        if (($null -ne $pe) -ne ($null -ne $record.Pe)) { Stop-Signing 'MANIFEST_PE_MISMATCH' }
        if ($null -ne $pe -or [IO.Path]::GetExtension($file.FullName) -ieq '.cat') {
            $signature = Get-CheckedSignature $file.FullName $pe
            $expected = if ($signature.Status.ToString() -eq 'Valid') { 'Preserve' } else { 'Sign' }
            if ($record.Disposition -ne $expected) { Stop-Signing 'MANIFEST_SIGNATURE_MISMATCH' }
        } elseif ($record.Disposition -ne 'Data') { Stop-Signing 'MANIFEST_DATA_MISMATCH' }
    }
}

function Invoke-WindowsSigning {
    if ($env:OS -ne 'Windows_NT') { Stop-Signing 'WINDOWS_REQUIRED' }
    $rootPath = Get-FullSigningPath $Root
    Assert-NoReparse $rootPath
    $stagePath = if ($Stage) { Get-FullSigningPath $Stage } else { $null }
    $signedPath = if ($Signed) { Get-FullSigningPath $Signed } else { $null }
    $manifestPath = if ($Manifest) { Get-FullSigningPath $Manifest } else { $null }
    $reportPath = if ($Report) { Get-FullSigningPath $Report } else { $null }
    $directories = @($rootPath)
    foreach ($path in @($stagePath, $signedPath)) {
        if ($path) {
            Assert-NoReparse $path
            foreach ($existing in $directories) { Assert-Separate $path $existing }
            $directories += $path
        }
    }
    foreach ($path in @($manifestPath, $reportPath)) {
        if ($path) {
            Assert-NoReparse $path
            foreach ($directory in $directories) { Assert-Separate $path $directory }
        }
    }
    if ($manifestPath -and $reportPath -and $manifestPath -ieq $reportPath) { Stop-Signing 'OVERLAPPING_PATHS' }
    $script:ValidatedReportPath = $reportPath
    $result = $null
    if ($Mode -in @('Preflight', 'Prepare', 'Audit')) {
        $inventory = @(Get-Inventory $rootPath)
        $toSign = @($inventory | Where-Object Disposition -EQ 'Sign')
        if ($toSign.Count -gt 100) { Stop-Signing 'SIGNING_BATCH_LIMIT' }
        if ($Mode -eq 'Audit' -and $toSign.Count -ne 0) { Stop-Signing 'UNSIGNED_RELEASE' }
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
            $state = [pscustomobject]@{ Schema = 1; Root = $rootPath; Files = $inventory }
            Write-SigningJson $manifestPath $state
            if ($env:GITHUB_OUTPUT) {
                [IO.File]::AppendAllText($env:GITHUB_OUTPUT, ('pending_count=' + $toSign.Count + "`n"), [Text.UTF8Encoding]::new($false))
            }
        }
        $result = [pscustomobject]@{
            Schema = 1; Mode = $Mode; Success = $true; Files = $inventory
            SignCount = $toSign.Count; PreservedCount = @($inventory | Where-Object Disposition -EQ 'Preserve').Count
        }
    } else {
        if (-not $signedPath -or -not $manifestPath) { Stop-Signing 'VERIFY_PATHS_REQUIRED' }
        if ($ExpectedSignerThumbprint -notmatch '^[0-9a-fA-F]{40}$') {
            Stop-Signing 'APPROVED_CERTIFICATE_IDENTITIES_REQUIRED'
        }
        $state = ConvertFrom-Json ([IO.File]::ReadAllText($manifestPath))
        Assert-ManifestFiles $state $rootPath
        $expected = @{}
        foreach ($record in @($state.Files | Where-Object Disposition -EQ 'Sign')) {
            if ($record.StageName -notmatch '^[0-9]{4}\.(exe|dll|sys)$' -or $expected.ContainsKey($record.StageName)) {
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
            $check = Assert-NewSignature $file.FullName $ExpectedSignerThumbprint
            $verified.Add([pscustomobject]@{
                Relative = $record.Relative; StageName = $record.StageName
                SHA256 = (Get-FileSha $file.FullName); Signature = $check
            })
        }
        # No source changes occur until the complete batch has passed. Replace
        # files, rather than truncating them, so a hard link cannot redirect writes.
        Assert-ManifestFiles $state $rootPath
        foreach ($record in $verified) {
            $source = Get-SafeChild $signedPath $record.StageName
            $destination = Get-SafeChild $rootPath $record.Relative
            if ((Get-FileSha $source) -cne $record.SHA256) { Stop-Signing 'SIGNED_CONTENT_CHANGED' }
            $temporary = Join-Path ([IO.Path]::GetDirectoryName($destination)) ([guid]::NewGuid().ToString() + '.signed-tmp')
            try {
                [IO.File]::Copy($source, $temporary, $false)
                if ((Get-FileSha $temporary) -cne $record.SHA256) { Stop-Signing 'COPY_HASH_MISMATCH' }
                [IO.File]::Replace($temporary, $destination, $null)
            } finally {
                if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary -Force }
            }
        }
        foreach ($record in @($state.Files)) {
            $path = Get-SafeChild $rootPath $record.Relative
            if ($record.Disposition -ne 'Sign' -and (Get-FileSha $path) -cne $record.SHA256) { Stop-Signing 'PRESERVED_CONTENT_CHANGED' }
        }
        $result = [pscustomobject]@{
            Schema = 1; Mode = 'Verify'; Success = $true; SignedCount = $verified.Count
            PreservedCount = @($state.Files | Where-Object Disposition -EQ 'Preserve').Count; Files = $verified.ToArray()
        }
    }
    if ($reportPath) { Write-SigningJson $reportPath $result }
    # Reports contain hashes, relative paths and public certificate thumbprints;
    # never certificates, environment variables, service responses or private keys.
    ConvertTo-Json -InputObject $result -Depth 12
}

if ($MyInvocation.InvocationName -ne '.') {
    try { Invoke-WindowsSigning }
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
