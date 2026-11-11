# Exercises the release boundary of scripts/windows-signing.ps1 with real Windows Authenticode, on
# the Windows runner of every Windows build (client-build.yml, "Test Windows signing checks").
# A temporary self-signed certificate is used only as an untrusted negative fixture; it is never
# added to a trust store, exported, or accepted as a release certificate. Nothing here talks to
# Azure: an accepted Artifact Signing signature is seen only in a release build's Verify steps.
# Run with PowerShell 7 on Windows.
[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'These tests require Windows.' }
$tool = Join-Path $PSScriptRoot 'windows-signing.ps1'
$work = Join-Path ([IO.Path]::GetTempPath()) ('hjv-signing-tests-' + [guid]::NewGuid().ToString())
$null = New-Item -ItemType Directory -Path $work
$savedOutput = $env:GITHUB_OUTPUT
$env:GITHUB_OUTPUT = $null
$certificate = $null
$script:passed = 0
$script:caseNumber = 0
$script:junctions = [Collections.Generic.List[string]]::new()
$publisher = 'HJViewer signing TEST ONLY publisher'

function Assert-Test([bool]$Value, [string]$Message) {
    if (-not $Value) { throw "Assertion failed: $Message" }
}

function Expect-Failure([scriptblock]$Action, [string]$Code) {
    $failed = $false
    try { $null = & $Action }
    catch {
        if ($_.Exception.Message -notlike "*Windows signing check failed: $Code") { throw }
        $failed = $true
    }
    Assert-Test $failed "expected $Code"
    $script:passed++
}

function New-Case {
    $script:caseNumber++
    $base = Join-Path $work ('case-' + $script:caseNumber)
    $root = Join-Path $base 'runtime'
    $null = New-Item -ItemType Directory -Path $root
    Copy-Item -LiteralPath $fixture -Destination (Join-Path $root 'program.dll')
    [IO.File]::WriteAllText((Join-Path $root 'data.txt'), 'Runtime data must stay byte-identical.')
    return [pscustomobject]@{
        Base = $base; Root = $root; Stage = (Join-Path $base 'input')
        Manifest = (Join-Path $base 'state.json'); Report = (Join-Path $base 'report.json')
        PinFile = $null; Publisher = $publisher
    }
}

function Run-Mode($Case, [string]$Phase) {
    # Invoke the same entry point as the CLI, without intercepting any Windows
    # signature APIs. Dynamic scope supplies the script's normal parameters.
    # As in the workflow, Artifact Signing signs the staged copies in place, so
    # Verify reads the signed files from the stage directory.
    $Mode = $Phase; $Root = $Case.Root
    $Stage = if ($Phase -eq 'Prepare') { $Case.Stage } else { $null }
    $Signed = if ($Phase -eq 'Verify') { $Case.Stage } else { $null }
    $Manifest = if ($Phase -in @('Prepare', 'Verify')) { $Case.Manifest } else { $null }
    $Report = $Case.Report
    $PinFile = $Case.PinFile
    $ExpectedPublisher = $Case.Publisher
    return (Invoke-WindowsSigning | ConvertFrom-Json)
}

function Write-Pins($Case, [string[]]$Lines) {
    $Case.PinFile = Join-Path $Case.Base 'pins.txt'
    [IO.File]::WriteAllLines($Case.PinFile, $Lines)
}

function Mutate-Byte([string]$Path, [int]$Offset) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $bytes[$Offset] = $bytes[$Offset] -bxor 1
    [IO.File]::WriteAllBytes($Path, $bytes)
}

function New-SignedFixture([string]$Name, [string]$Hash) {
    $path = Join-Path $work $Name
    Copy-Item $fixture $path
    # The signer certificate goes into the signature even though it is its own root.
    $result = Set-AuthenticodeSignature -LiteralPath $path -Certificate $certificate -HashAlgorithm $Hash -IncludeChain Signer
    Assert-Test ($result.Status.ToString() -ne 'Valid') 'test certificate never became trusted'
    return $path
}

try {
    . $tool -Mode Preflight -Root $work
    $fixture = Join-Path $work 'fixture.dll'
    $type = 'HjvSigningFixture' + ([guid]::NewGuid().ToString('N'))
    Add-Type -TypeDefinition "public class $type { public static int Value() { return 7; } }" -OutputAssembly $fixture
    Assert-Test ((Get-AuthenticodeSignature -LiteralPath $fixture).Status.ToString() -eq 'NotSigned') 'fixture is unsigned'

    # An ephemeral self-signed fixture gives real Authenticode layouts whose trust Windows must
    # still reject. Read back from the store so that signing gets a certificate with its key.
    $created = New-SelfSignedCertificate -Type CodeSigningCert -Subject ('CN=HJViewer signing TEST ONLY ' + [guid]::NewGuid().ToString()) `
        -CertStoreLocation Cert:\CurrentUser\My -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddDays(1)
    $certificate = Get-Item -LiteralPath ('Cert:\CurrentUser\My\' + $created.Thumbprint)
    $selfSigned = New-SignedFixture 'self-signed.dll' 'SHA256'
    $sha1Signed = New-SignedFixture 'sha1-signed.dll' 'SHA1'

    # Header-based PE detection includes an extensionless image and an image
    # whose extension incorrectly says EXE. Valid Windows binaries are preserved.
    $case = New-Case
    Copy-Item $fixture (Join-Path $case.Root 'extensionless')
    Copy-Item $fixture (Join-Path $case.Root 'misnamed.exe')
    $vendor = Join-Path $case.Root 'vendor.exe'
    Copy-Item (Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe') $vendor
    $vendorHash = Get-FileSha $vendor
    $env:GITHUB_OUTPUT = Join-Path $case.Base 'github-output.txt'
    $prepared = Run-Mode $case Prepare
    Assert-Test ($prepared.SignCount -eq 3) 'all unsigned PE images are staged'
    Assert-Test ($prepared.PreservedCount -eq 1) 'valid vendor binary is preserved'
    Assert-Test ($prepared.PinnedCount -eq 0) 'nothing is pinned without a pin file'
    Assert-Test (@(Get-ChildItem $case.Stage -File).Count -eq 3) 'staging excludes data and signed vendor'
    Assert-Test (@(Get-ChildItem $case.Stage -File | Where-Object Extension -NE '.dll').Count -eq 0) 'PE characteristics select extension'
    Assert-Test ((Get-FileSha $vendor) -ceq $vendorHash) 'preserved vendor bytes'
    Assert-Test ([IO.File]::ReadAllText($env:GITHUB_OUTPUT).Trim() -eq 'pending_count=3') 'GitHub pending count'
    $env:GITHUB_OUTPUT = $null
    $script:passed++
    Expect-Failure { Run-Mode $case Prepare } 'OUTPUT_ALREADY_EXISTS'
    Expect-Failure { Run-Mode $case Audit } 'UNSIGNED_RELEASE'

    # Signed vendor-only releases need no new signing and pass the final audit.
    $onlyVendor = New-Case
    Remove-Item (Join-Path $onlyVendor.Root 'program.dll')
    Copy-Item $vendor (Join-Path $onlyVendor.Root 'vendor.exe')
    $audited = Run-Mode $onlyVendor Audit
    Assert-Test ($audited.SignCount -eq 0 -and $audited.PreservedCount -eq 1) 'signed release audit'
    $script:passed++
    $null = Run-Mode $onlyVendor Prepare
    $zero = Run-Mode $onlyVendor Verify
    Assert-Test ($zero.SignedCount -eq 0) 'empty batch verifies'
    $script:passed++

    # Verify needs the approved publisher before it looks at anything.
    $noPublisher = New-Case; $null = Run-Mode $noPublisher Prepare
    $noPublisher.Publisher = ' '
    Expect-Failure { Run-Mode $noPublisher Verify } 'EXPECTED_PUBLISHER_REQUIRED'

    # Existing signed bytes that were tampered with must never enter signing.
    $badVendor = New-Case
    $bad = Join-Path $badVendor.Root 'vendor.exe'
    # Windows system executables can be catalog-only: after a mutation they may
    # simply become NotSigned. Git for Windows (also present on Actions runners)
    # supplies an embedded signature whose hash failure is observable on the file.
    $gitExe = (Get-Command git.exe -ErrorAction Stop).Source
    Assert-Test ((Get-AuthenticodeSignature -LiteralPath $gitExe).SignatureType.ToString() -eq 'Authenticode') 'embedded vendor signature fixture'
    $renamedSigned = New-Case
    Remove-Item (Join-Path $renamedSigned.Root 'program.dll')
    Copy-Item $gitExe (Join-Path $renamedSigned.Root 'extensionless')
    Copy-Item $gitExe (Join-Path $renamedSigned.Root 'disguised.dat')
    $renamedAudit = Run-Mode $renamedSigned Audit
    Assert-Test ($renamedAudit.PreservedCount -eq 2 -and $renamedAudit.SignCount -eq 0) 'signed PE works with absent or unrelated extension'
    $script:passed++
    Copy-Item $gitExe $bad
    Mutate-Byte $bad 64
    Assert-Test ((Get-AuthenticodeSignature -LiteralPath $bad).Status.ToString() -eq 'HashMismatch') 'real tampered Authenticode fixture'
    Expect-Failure { Run-Mode $badVendor Prepare } 'EXISTING_SIGNATURE_HASHMISMATCH'
    Assert-Test (-not (Test-Path $badVendor.Stage)) 'invalid vendor failure occurs before staging'

    # A vendor file whose own signature Windows does not accept (the virtual display driver's
    # DLL carries a test signature) is shipped as it is only when the pin file names its bytes.
    $untrusted = New-Case
    Remove-Item (Join-Path $untrusted.Root 'program.dll')
    $null = New-Item -ItemType Directory -Path (Join-Path $untrusted.Root 'driver')
    Copy-Item $selfSigned (Join-Path $untrusted.Root 'driver/vendor.dll')
    $untrustedStatus = (Get-AuthenticodeSignature -LiteralPath $selfSigned).Status.ToString().ToUpperInvariant()
    Expect-Failure { Run-Mode $untrusted Audit } ('EXISTING_SIGNATURE_' + $untrustedStatus)
    Write-Pins $untrusted @('# a comment line', '', ((Get-FileSha $selfSigned) + '  driver/vendor.dll  # why'))
    $pinnedAudit = Run-Mode $untrusted Audit
    Assert-Test ($pinnedAudit.PinnedCount -eq 1 -and $pinnedAudit.SignCount -eq 0) 'pinned vendor file passes the audit'
    $script:passed++
    $null = Run-Mode $untrusted Prepare
    Assert-Test (@(Get-ChildItem $untrusted.Stage -File).Count -eq 0) 'a pinned file is never staged'
    $pinnedVerify = Run-Mode $untrusted Verify
    Assert-Test ($pinnedVerify.PinnedCount -eq 1 -and $pinnedVerify.SignedCount -eq 0) 'pinned file verifies untouched'
    $script:passed++
    Write-Pins $untrusted @(((Get-FileSha $fixture) + '  driver/vendor.dll'))
    Expect-Failure { Run-Mode $untrusted Audit } 'PINNED_FILE_CHANGED'
    Write-Pins $untrusted @(((Get-FileSha $selfSigned) + '  driver/vendor.dll'), ((Get-FileSha $fixture) + '  driver/gone.dll'))
    Expect-Failure { Run-Mode $untrusted Audit } 'PINNED_FILE_MISSING'
    foreach ($lines in @(@('# only comments'), @('ABC  driver/vendor.dll'), @(((Get-FileSha $selfSigned).ToUpperInvariant()) + '  driver/vendor.dll'),
                         @(((Get-FileSha $selfSigned) + '  driver/vendor.dll'), ((Get-FileSha $selfSigned) + '  DRIVER/VENDOR.DLL')))) {
        Write-Pins $untrusted $lines
        Expect-Failure { Run-Mode $untrusted Audit } 'PIN_FILE_INVALID'
    }
    Write-Pins $untrusted @(((Get-FileSha $selfSigned) + '  ../outside.dll'))
    Expect-Failure { Run-Mode $untrusted Audit } 'UNSAFE_RELATIVE_PATH'
    $untrusted.PinFile = Join-Path $untrusted.Base 'missing-pins.txt'
    Expect-Failure { Run-Mode $untrusted Audit } 'PIN_FILE_MISSING'
    $untrusted.PinFile = Join-Path $untrusted.Root 'data.txt'
    Expect-Failure { Run-Mode $untrusted Audit } 'OVERLAPPING_PATHS'

    # An unsigned kernel driver is never signed with a code signing certificate.
    $driver = New-Case
    $sys = Join-Path $driver.Root 'program.dll'
    $bytes = [IO.File]::ReadAllBytes($sys)
    [BitConverter]::GetBytes([uint16]1).CopyTo($bytes, (Get-PeInfo $sys).ChecksumAt + 4)
    [IO.File]::WriteAllBytes($sys, $bytes)
    Assert-Test ((Get-PeInfo $sys).Kind -eq 'SYS') 'native subsystem fixture'
    Expect-Failure { Run-Mode $driver Prepare } 'DRIVER_NOT_SIGNED'

    $missing = New-Case; $null = Run-Mode $missing Prepare
    Remove-Item (Join-Path $missing.Stage '0000.dll')
    Expect-Failure { Run-Mode $missing Verify } 'SIGNED_FILE_SET_MISMATCH'

    $extra = New-Case; $null = Run-Mode $extra Prepare
    [IO.File]::WriteAllText((Join-Path $extra.Stage 'unexpected.txt'), 'extra')
    Expect-Failure { Run-Mode $extra Verify } 'SIGNED_FILE_SET_MISMATCH'

    $unsigned = New-Case; $null = Run-Mode $unsigned Prepare
    Expect-Failure { Run-Mode $unsigned Verify } 'PE_CERTIFICATE_LAYOUT_CHANGED'
    Assert-Test ((Get-FileSha (Join-Path $unsigned.Root 'program.dll')) -ceq (Get-FileSha $fixture)) 'failed verify does not write source'

    # A copy signed by anything Windows does not trust never reaches the release root.
    $untrustedSigner = New-Case; $null = Run-Mode $untrustedSigner Prepare
    $null = Set-AuthenticodeSignature -LiteralPath (Join-Path $untrustedSigner.Stage '0000.dll') -Certificate $certificate -HashAlgorithm SHA256
    Expect-Failure { Run-Mode $untrustedSigner Verify } ('EXISTING_SIGNATURE_' + $untrustedStatus)
    Assert-Test ((Get-FileSha (Join-Path $untrustedSigner.Root 'program.dll')) -ceq (Get-FileSha $fixture)) 'untrusted signature is not applied'

    $sourceChanged = New-Case; $null = Run-Mode $sourceChanged Prepare
    [IO.File]::WriteAllText((Join-Path $sourceChanged.Root 'data.txt'), 'changed after prepare')
    Expect-Failure { Run-Mode $sourceChanged Verify } 'SOURCE_CONTENT_CHANGED'

    # Manifest traversal cannot cause a read/copy outside the release root.
    $traversal = New-Case; $null = Run-Mode $traversal Prepare
    $state = ConvertFrom-Json ([IO.File]::ReadAllText($traversal.Manifest))
    $state.Files[0].Relative = '../outside.dll'
    Write-SigningJson $traversal.Manifest $state
    Expect-Failure { Run-Mode $traversal Verify } 'UNSAFE_RELATIVE_PATH'
    foreach ($relative in @('../escape', '/absolute', 'C:\escape', 'file:stream', 'dir/../escape', 'trailing.')) {
        Expect-Failure { Get-SafeChild $case.Root $relative } 'UNSAFE_RELATIVE_PATH'
    }

    # Junctions are reparse points just like directory symlinks; creating them
    # does not need developer mode or the elevated symlink privilege.
    $linked = New-Case
    $junction = Join-Path $linked.Root 'linked'
    $null = New-Item -ItemType Junction -Path $junction -Target $onlyVendor.Root
    $script:junctions.Add($junction)
    Expect-Failure { Run-Mode $linked Prepare } 'REPARSE_POINT'
    $linkedOutput = New-Case; $null = Run-Mode $linkedOutput Prepare
    Remove-Item -LiteralPath $linkedOutput.Stage -Recurse -Force
    $null = New-Item -ItemType Junction -Path $linkedOutput.Stage -Target $onlyVendor.Root
    $script:junctions.Add($linkedOutput.Stage)
    Expect-Failure { Run-Mode $linkedOutput Verify } 'REPARSE_POINT'

    # A fake .exe and a malformed MZ file cannot hide from PE inspection.
    $malformed = New-Case
    [IO.File]::WriteAllText((Join-Path $malformed.Root 'bad.exe'), 'not a PE')
    Expect-Failure { Run-Mode $malformed Preflight } 'PE_HEADER_MISSING'
    [IO.File]::WriteAllBytes((Join-Path $malformed.Root 'bad.exe'), [byte[]]@(0x4d, 0x5a))
    Expect-Failure { Run-Mode $malformed Preflight } 'PE_HEADER_INVALID'

    # The byte-identity guard on an actual Authenticode layout; Windows must still reject
    # the self-signed fixture's trust.
    Assert-PeUnchanged $fixture $selfSigned
    $script:passed++
    Expect-Failure { Assert-NewSignature $selfSigned $publisher } ('EXISTING_SIGNATURE_' + $untrustedStatus)

    # The structure checks that WinTrust's "Valid" does not make: SHA-256 digests, one
    # certificate table entry, and an RFC 3161 timestamp (the fixture has none).
    Expect-Failure { Get-SignatureStructure $selfSigned (Get-PeInfo $selfSigned) $null } 'RFC3161_TIMESTAMP_REQUIRED'
    Expect-Failure { Get-SignatureStructure $sha1Signed (Get-PeInfo $sha1Signed) $null } 'FILE_DIGEST_NOT_SHA256'
    $twoEntries = Join-Path $work 'two-entries.dll'
    $pe = Get-PeInfo $selfSigned
    $original = [IO.File]::ReadAllBytes($selfSigned)
    $bytes = [byte[]]::new($original.Length + 8)
    [Array]::Copy($original, $bytes, $original.Length)
    [BitConverter]::GetBytes([uint32]($pe.CertificateSize + 8)).CopyTo($bytes, $pe.SecurityAt + 4)
    [IO.File]::WriteAllBytes($twoEntries, $bytes)
    Expect-Failure { Get-SignatureStructure $twoEntries (Get-PeInfo $twoEntries) $null } 'SECOND_CERTIFICATE_ENTRY'
    $der = Get-DerValue ([byte[]]@(0x30, 0x03, 1, 2, 3, 0, 0))
    Assert-Test ($der.Length -eq 5 -and $der[4] -eq 3) 'DER value without its padding'
    $der = Get-DerValue ([byte[]]@(0x30, 0x81, 0x02, 9, 9))
    Assert-Test ($der.Length -eq 5) 'long-form DER length'
    $script:passed++
    Expect-Failure { Get-DerValue ([byte[]]@(0x30, 0x03, 1, 2, 3, 0, 1)) } 'SIGNATURE_TRAILING_DATA'
    Expect-Failure { Get-DerValue ([byte[]]@(0x31, 0x01, 0)) } 'SIGNATURE_UNREADABLE'
    Expect-Failure { Get-DerValue ([byte[]]@(0x30, 0x05, 1)) } 'SIGNATURE_UNREADABLE'
    Expect-Failure { Get-DerValue ([byte[]]@(0x30, 0x01, 1, 0, 0, 0, 0, 0, 0, 0, 0)) } 'SIGNATURE_UNREADABLE'

    # One batch, one certificate.
    Assert-OneSigner @([pscustomobject]@{ SignerThumbprint = 'A' * 40 }, [pscustomobject]@{ SignerThumbprint = 'A' * 40 })
    Assert-OneSigner @()
    $script:passed++
    Expect-Failure { Assert-OneSigner @([pscustomobject]@{ SignerThumbprint = 'A' * 40 }, [pscustomobject]@{ SignerThumbprint = 'B' * 40 }) } 'MIXED_SIGNERS'

    # Exercise the production copy/hash/Replace function on NTFS. Signature
    # verification is not mocked: this tests only the post-verification file
    # operation, including the real null backup argument needed by File.Replace.
    $apply = New-Case
    $destination = Join-Path $apply.Root 'program.dll'
    [IO.File]::WriteAllText($destination, 'old image')
    $hardlink = Join-Path $apply.Root 'old-image-link'
    $null = New-Item -ItemType HardLink -Path $hardlink -Target $destination
    $fixtureHash = Get-FileSha $fixture
    Apply-VerifiedFile $fixture $destination $fixtureHash
    Assert-Test ((Get-FileSha $destination) -ceq $fixtureHash) 'applied image matches verified bytes'
    Assert-Test ((Get-FileSha $fixture) -ceq $fixtureHash) 'applying preserves signed input bytes'
    Assert-Test ([IO.File]::ReadAllText($hardlink) -ceq 'old image') 'replace does not overwrite other hard links'
    Assert-Test (@(Get-ChildItem -LiteralPath $apply.Root -Filter '*.signed-tmp').Count -eq 0) 'replacement temporary file is cleaned'
    $script:passed++
    Expect-Failure { Apply-VerifiedFile $fixture $destination ('0' * 64) } 'SIGNED_CONTENT_CHANGED'
    Assert-Test ((Get-FileSha $destination) -ceq $fixtureHash) 'hash failure leaves destination unchanged'

    # A successful CLI call must clear a previous native-command failure, since
    # GitHub's shell wrapper propagates LASTEXITCODE after the script returns.
    $global:LASTEXITCODE = 19
    $null = & $tool -Mode Audit -Root $onlyVendor.Root
    Assert-Test ($global:LASTEXITCODE -eq 0) 'successful CLI clears stale native exit code'
    $script:passed++

    # The public CLI emits only a bounded error code, not raw provider messages.
    $engine = (Get-Process -Id $PID).Path
    $savedPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $cli = & $engine -NoProfile -NonInteractive -File $tool -Mode Audit -Root $unsigned.Root 2>&1
    $cliExit = $LASTEXITCODE
    $ErrorActionPreference = $savedPreference
    Assert-Test ($cliExit -eq 1) 'CLI unsigned release fails'
    Assert-Test (($cli | Out-String) -match 'UNSIGNED_RELEASE') 'CLI emits safe reason'
    Assert-Test (($cli | Out-String) -notmatch [regex]::Escape($unsigned.Root)) 'CLI does not echo local source paths'
    $script:passed++

    # The repository's own pin file reads.
    $script:Pins = Read-Pins (Join-Path $PSScriptRoot 'windows-signing-pins.txt')
    Assert-Test ($script:Pins.Count -ge 1) 'repository pin file'
    $script:Pins = @{}
    $script:passed++
    Write-Output "Windows signing tests passed: $script:passed"
} finally {
    $env:GITHUB_OUTPUT = $savedOutput
    # Remove junctions themselves before recursive fixture cleanup.
    foreach ($junction in $script:junctions) {
        if (Test-Path -LiteralPath $junction) { [IO.Directory]::Delete($junction) }
    }
    if ($certificate) {
        Remove-Item -LiteralPath ('Cert:\CurrentUser\My\' + $certificate.Thumbprint) -DeleteKey -Force
    }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
# The expected failing child CLI above leaves LASTEXITCODE=1. GitHub's pwsh
# wrapper exits with that variable after a successful script, so clear it only
# after every assertion and cleanup has succeeded.
$global:LASTEXITCODE = 0
