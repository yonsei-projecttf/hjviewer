# Exercises the release boundary with real Windows Authenticode. A temporary
# self-signed certificate is used only as an untrusted negative fixture; it is
# never added to a trust store, exported, or accepted as a release certificate.
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
        Signed = (Join-Path $base 'output'); Manifest = (Join-Path $base 'state.json')
        Report = (Join-Path $base 'report.json')
    }
}

function Run-Mode($Case, [string]$Phase) {
    # Invoke the same entry point as the CLI, without intercepting any Windows
    # signature APIs. Dynamic scope supplies the script's normal parameters.
    $Mode = $Phase; $Root = $Case.Root; $Stage = $Case.Stage
    $Signed = if ($Phase -eq 'Verify') { $Case.Signed } else { $null }
    $Manifest = if ($Phase -in @('Prepare', 'Verify')) { $Case.Manifest } else { $null }
    $Report = $Case.Report
    $ExpectedSignerThumbprint = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
    return (Invoke-WindowsSigning | ConvertFrom-Json)
}

function Copy-UnsignedOutput($Case) {
    $null = New-Item -ItemType Directory -Path $Case.Signed
    Get-ChildItem -LiteralPath $Case.Stage -File | Copy-Item -Destination $Case.Signed
}

function Mutate-Byte([string]$Path, [int]$Offset) {
    $bytes = [IO.File]::ReadAllBytes($Path)
    $bytes[$Offset] = $bytes[$Offset] -bxor 1
    [IO.File]::WriteAllBytes($Path, $bytes)
}

try {
    . $tool -Mode Preflight -Root $work
    $fixture = Join-Path $work 'fixture.dll'
    $type = 'HjvSigningFixture' + ([guid]::NewGuid().ToString('N'))
    Add-Type -TypeDefinition "public class $type { public static int Value() { return 7; } }" -OutputAssembly $fixture
    Assert-Test ((Get-AuthenticodeSignature -LiteralPath $fixture).Status.ToString() -eq 'NotSigned') 'fixture is unsigned'

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
    Assert-Test ($zero.SignedCount -eq 0 -and -not (Test-Path $onlyVendor.Signed)) 'empty batch needs no service output directory'
    $script:passed++

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

    $missing = New-Case; $null = Run-Mode $missing Prepare
    $null = New-Item -ItemType Directory $missing.Signed
    Expect-Failure { Run-Mode $missing Verify } 'SIGNED_FILE_SET_MISMATCH'

    $extra = New-Case; $null = Run-Mode $extra Prepare; Copy-UnsignedOutput $extra
    [IO.File]::WriteAllText((Join-Path $extra.Signed 'unexpected.txt'), 'extra')
    Expect-Failure { Run-Mode $extra Verify } 'SIGNED_FILE_SET_MISMATCH'

    $unsigned = New-Case; $null = Run-Mode $unsigned Prepare; Copy-UnsignedOutput $unsigned
    Expect-Failure { Run-Mode $unsigned Verify } 'PE_CERTIFICATE_LAYOUT_CHANGED'
    Assert-Test ((Get-FileSha (Join-Path $unsigned.Root 'program.dll')) -ceq (Get-FileSha $fixture)) 'failed verify does not write source'

    $sourceChanged = New-Case; $null = Run-Mode $sourceChanged Prepare; Copy-UnsignedOutput $sourceChanged
    [IO.File]::WriteAllText((Join-Path $sourceChanged.Root 'data.txt'), 'changed after prepare')
    Expect-Failure { Run-Mode $sourceChanged Verify } 'SOURCE_CONTENT_CHANGED'

    # Manifest traversal cannot cause a read/copy outside the release root.
    $traversal = New-Case; $null = Run-Mode $traversal Prepare; Copy-UnsignedOutput $traversal
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
    $null = New-Item -ItemType Junction -Path $linkedOutput.Signed -Target $onlyVendor.Root
    $script:junctions.Add($linkedOutput.Signed)
    Expect-Failure { Run-Mode $linkedOutput Verify } 'REPARSE_POINT'

    # A fake .exe and a malformed MZ file cannot hide from PE inspection.
    $malformed = New-Case
    [IO.File]::WriteAllText((Join-Path $malformed.Root 'bad.exe'), 'not a PE')
    Expect-Failure { Run-Mode $malformed Preflight } 'PE_HEADER_MISSING'
    [IO.File]::WriteAllBytes((Join-Path $malformed.Root 'bad.exe'), [byte[]]@(0x4d, 0x5a))
    Expect-Failure { Run-Mode $malformed Preflight } 'PE_HEADER_INVALID'

    # An ephemeral self-signed fixture lets the byte-identity guard inspect an
    # actual Authenticode layout, while Windows must still reject its trust.
    $certificate = New-SelfSignedCertificate -Type CodeSigningCert -Subject ('CN=HJViewer signing TEST ONLY ' + [guid]::NewGuid().ToString()) `
        -CertStoreLocation Cert:\CurrentUser\My -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddDays(1)
    $selfSigned = Join-Path $work 'self-signed.dll'
    Copy-Item $fixture $selfSigned
    $sig = Set-AuthenticodeSignature -LiteralPath $selfSigned -Certificate $certificate -HashAlgorithm SHA256
    Assert-Test ($sig.Status.ToString() -ne 'Valid') 'test certificate never became trusted'
    Assert-PeUnchanged $fixture $selfSigned
    $script:passed++
    Expect-Failure { Assert-NewSignature $selfSigned $certificate.Thumbprint } ('EXISTING_SIGNATURE_' + $sig.Status.ToString().ToUpperInvariant())
    Mutate-Byte $selfSigned 64
    Expect-Failure { Assert-PeUnchanged $fixture $selfSigned } 'PE_CONTENT_CHANGED'

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
