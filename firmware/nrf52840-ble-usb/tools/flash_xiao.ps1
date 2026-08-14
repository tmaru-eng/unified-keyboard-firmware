#Requires -Version 5.1

[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateNotNullOrEmpty()]
    [string]$Uf2Path,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$BackupDirectory,

    [switch]$ConfirmFlash
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Assert-DriveRoot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Root
    )

    $fullRoot = [System.IO.Path]::GetFullPath($Root)
    $pathRoot = [System.IO.Path]::GetPathRoot($fullRoot)
    if (-not [string]::Equals(
            $fullRoot.TrimEnd([char[]]'\/'),
            $pathRoot.TrimEnd([char[]]'\/'),
            [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing non-root UF2 volume path: $fullRoot"
    }
    return $pathRoot
}

function Assert-DirectChildOfDriveRoot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Root,

        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $driveRoot = Assert-DriveRoot -Root $Root
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $parent = [System.IO.Directory]::GetParent($fullPath)
    if ($null -eq $parent -or -not [string]::Equals(
            $parent.FullName.TrimEnd([char[]]'\/'),
            $driveRoot.TrimEnd([char[]]'\/'),
            [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing write outside the UF2 drive root: $fullPath"
    }
    return $fullPath
}

function Assert-NoReparsePointPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $cursor = [System.IO.Path]::GetFullPath($Path)
    while ($cursor) {
        if ([System.IO.File]::Exists($cursor) -or [System.IO.Directory]::Exists($cursor)) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Refusing path containing a junction or symbolic link: $cursor"
            }
        }

        $parent = [System.IO.Directory]::GetParent($cursor)
        if ($null -eq $parent) {
            break
        }
        $cursor = $parent.FullName
    }
}

function Test-XiaoMetadata {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Metadata
    )

    $boardMatches = [regex]::Matches($Metadata, '(?m)^Board-ID:\s*([^\r\n]*?)\s*$')
    $softDeviceMatches = [regex]::Matches($Metadata, '(?m)^SoftDevice:\s*([^\r\n]*?)\s*$')
    if ($boardMatches.Count -ne 1 -or $softDeviceMatches.Count -ne 1) {
        return $false
    }

    $board = $boardMatches[0].Groups[1].Value
    $softDevice = $softDeviceMatches[0].Groups[1].Value
    return (
        $board -ceq 'Seeed_XIAO_nRF52840_Sense' -and
        ($softDevice -ceq 'S140 version 7.3.0' -or $softDevice -ceq 'S140 7.3.0')
    )
}

function Invoke-FlashShouldProcess {
    [CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
    param(
        [Parameter(Mandatory = $true)]
        [string]$Target,

        [Parameter(Mandatory = $true)]
        [string]$Action
    )

    # Advanced scripts on some Windows PowerShell hosts do not populate the
    # script-scope $PSCmdlet. Keep the destructive boundary in this advanced
    # function, where ShouldProcess is always available, while preserving the
    # caller's WhatIf/Confirm semantics.
    # The script-level -ConfirmFlash switch is the explicit owner approval;
    # callers pass -Confirm:$false to suppress the nested high-impact prompt
    # so unattended CI/PowerShell hosts do not enter a null interactive path.
    return $PSCmdlet.ShouldProcess($Target, $Action)
}

function Test-Uf2VolumeDetached {
    param(
        [Parameter(Mandatory = $true)]
        [string]$VolumeRoot
    )

    try {
        $expectedRoot = Assert-DriveRoot -Root $VolumeRoot
        $matchingDrives = @(
            [System.IO.DriveInfo]::GetDrives() | Where-Object {
                [string]::Equals(
                    $_.RootDirectory.FullName.TrimEnd([char[]]'\/'),
                    $expectedRoot.TrimEnd([char[]]'\/'),
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            }
        )
        if ($matchingDrives.Count -eq 0) {
            return $true
        }
        if ($matchingDrives.Count -ne 1) {
            return $false
        }
        return (-not $matchingDrives[0].IsReady)
    }
    catch {
        # A probe error is not proof of bootloader detach. Fail closed so the
        # original destination IOException remains fatal.
        return $false
    }
}

function Test-CompletedWriteAndDetachedVolume {
    param(
        [Parameter(Mandatory = $true)]
        [long]$BytesWritten,

        [Parameter(Mandatory = $true)]
        [long]$ExpectedBytes,

        [Parameter(Mandatory = $true)]
        [string]$VolumeRoot
    )

    return (
        $BytesWritten -eq $ExpectedBytes -and
        (Test-Uf2VolumeDetached -VolumeRoot $VolumeRoot)
    )
}

function Get-XiaoUf2Volume {
    $matches = @()
    foreach ($drive in [System.IO.DriveInfo]::GetDrives()) {
        if (-not $drive.IsReady -or $drive.DriveType -ne [System.IO.DriveType]::Removable) {
            continue
        }

        $root = Assert-DriveRoot -Root $drive.RootDirectory.FullName
        $infoPath = Join-Path $root 'INFO_UF2.TXT'
        if (-not [System.IO.File]::Exists($infoPath)) {
            continue
        }

        try {
            $metadata = [System.IO.File]::ReadAllText($infoPath)
        }
        catch {
            Write-Warning "Could not read $infoPath; ignoring this volume: $($_.Exception.Message)"
            continue
        }

        if (Test-XiaoMetadata -Metadata $metadata) {
            $matches += [PSCustomObject]@{
                Root = $root
                Info = $metadata
            }
        }
    }

    if ($matches.Count -eq 0) {
        throw 'No removable UF2 volume has the required XIAO Sense Board-ID and S140 7.3.0 metadata.'
    }
    if ($matches.Count -ne 1) {
        $roots = ($matches | ForEach-Object { $_.Root }) -join ', '
        throw "Multiple matching XIAO UF2 volumes were found ($roots); disconnect all but the intended board."
    }
    return $matches[0]
}

function Invoke-Uf2Validation {
    param(
        [Parameter(Mandatory = $true)]
        [System.IO.FileInfo]$SourceFile
    )

    $validator = Join-Path $PSScriptRoot 'xiao_uf2.py'
    if (-not [System.IO.File]::Exists($validator)) {
        throw "UF2 validator is missing: $validator"
    }

    $python = Get-Command py -ErrorAction SilentlyContinue
    if (-not $python) {
        $python = Get-Command python -ErrorAction Stop
    }

    $validationOutput = & $python.Source $validator check $SourceFile.FullName 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "UF2 validation failed: $($validationOutput -join [Environment]::NewLine)"
    }
    Write-Host ($validationOutput -join [Environment]::NewLine)
    Write-Host 'Candidate Family ID verified by xiao_uf2.py: 0xADA52840'
}

$sourceFile = Get-Item -LiteralPath $Uf2Path -ErrorAction Stop
if ($sourceFile.PSIsContainer -or $sourceFile.Extension -ine '.uf2') {
    throw "The candidate must be one existing .uf2 file: $Uf2Path"
}
Assert-NoReparsePointPath -Path $sourceFile.FullName

Invoke-Uf2Validation -SourceFile $sourceFile
$volume = Get-XiaoUf2Volume
$volumeRoot = Assert-DriveRoot -Root $volume.Root

$sourceRoot = [System.IO.Path]::GetPathRoot($sourceFile.FullName)
if ([string]::Equals(
        $sourceRoot.TrimEnd([char[]]'\/'),
        $volumeRoot.TrimEnd([char[]]'\/'),
        [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The candidate UF2 must not be sourced from the detected bootloader volume.'
}

$backupRoot = [System.IO.Path]::GetFullPath($BackupDirectory)
$backupDriveRoot = [System.IO.Path]::GetPathRoot($backupRoot)
if ([string]::Equals(
        $backupDriveRoot.TrimEnd([char[]]'\/'),
        $volumeRoot.TrimEnd([char[]]'\/'),
        [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'The backup directory must be on a different volume from the UF2 bootloader.'
}
Assert-NoReparsePointPath -Path $backupRoot

if (-not $WhatIfPreference -and -not $ConfirmFlash.IsPresent) {
    throw 'Refusing to write without -ConfirmFlash. Use -WhatIf to inspect the planned operations.'
}

$candidateBytes = $null
if (-not $WhatIfPreference) {
    $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    Assert-NoReparsePointPath -Path $temporaryRoot
    $temporaryDriveRoot = [System.IO.Path]::GetPathRoot($temporaryRoot)
    if ([string]::Equals(
            $temporaryDriveRoot.TrimEnd([char[]]'\/'),
            $volumeRoot.TrimEnd([char[]]'\/'),
            [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'The temporary validation directory must not be on the UF2 bootloader volume.'
    }

    $stagedPath = Join-Path $temporaryRoot "ukf-xiao-$([Guid]::NewGuid().ToString('N')).uf2"
    if (-not (Invoke-FlashShouldProcess $stagedPath 'Create a locked candidate snapshot for final validation' -Confirm:$false)) {
        return
    }

    $candidateBytes = [System.IO.File]::ReadAllBytes($sourceFile.FullName)
    $stagedStream = [System.IO.FileStream]::new(
        $stagedPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::Read,
        4096,
        [System.IO.FileOptions]::SequentialScan
    )
    try {
        $stagedStream.Write($candidateBytes, 0, $candidateBytes.Length)
        $stagedStream.Flush($true)
        Invoke-Uf2Validation -SourceFile (Get-Item -LiteralPath $stagedPath)
    }
    finally {
        $stagedStream.Dispose()
        if ([System.IO.File]::Exists($stagedPath)) {
            [System.IO.File]::Delete($stagedPath)
        }
    }
}

Write-Host "Verified UF2 volume: $volumeRoot"
Write-Host 'Verified Board-ID: Seeed_XIAO_nRF52840_Sense'
Write-Host 'Verified SoftDevice: S140 7.3.0'

if (-not [System.IO.Directory]::Exists($backupRoot)) {
    if (Invoke-FlashShouldProcess $backupRoot 'Create backup directory' -Confirm:$false) {
        [System.IO.Directory]::CreateDirectory($backupRoot) | Out-Null
        Assert-NoReparsePointPath -Path $backupRoot
    }
    elseif (-not $WhatIfPreference) {
        throw 'Flashing cancelled because backup directory creation was not approved.'
    }
}

$currentUf2 = Join-Path $volumeRoot 'CURRENT.UF2'
if ([System.IO.File]::Exists($currentUf2)) {
    Assert-NoReparsePointPath -Path $backupRoot
    $timestamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmssfff')
    $backupPath = Join-Path $backupRoot "xiao-sense-current-$timestamp.uf2"
    if ([System.IO.File]::Exists($backupPath)) {
        throw "Refusing to overwrite existing recovery backup: $backupPath"
    }
    if (Invoke-FlashShouldProcess $backupPath "Back up $currentUf2 without overwrite" -Confirm:$false) {
        [System.IO.File]::Copy($currentUf2, $backupPath, $false)
        Write-Host "Recovery backup created: $backupPath"
    }
    elseif (-not $WhatIfPreference) {
        throw 'Flashing cancelled because the recovery backup was not approved.'
    }
}
else {
    Write-Warning "CURRENT.UF2 is absent on $volumeRoot; no recovery image was available to back up."
}

# Re-read the identity immediately before the destructive copy in case the drive
# was removed or its letter was reused while the backup was being created.
$infoPath = Join-Path $volumeRoot 'INFO_UF2.TXT'
if (-not [System.IO.File]::Exists($infoPath) -or
    -not (Test-XiaoMetadata -Metadata ([System.IO.File]::ReadAllText($infoPath)))) {
    throw 'The verified XIAO UF2 volume disappeared or changed identity before flashing.'
}

$destination = Join-Path $volumeRoot $sourceFile.Name
$destination = Assert-DirectChildOfDriveRoot -Root $volumeRoot -Path $destination
if ([System.IO.File]::Exists($destination)) {
    throw "Refusing to overwrite an existing file on the UF2 volume: $destination"
}

if (Invoke-FlashShouldProcess $destination "Flash only candidate $($sourceFile.FullName)" -Confirm:$false) {
    $destinationStream = [System.IO.FileStream]::new(
        $destination,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    $bytesWritten = 0L
    $acceptedDetach = $false
    try {
        $destinationStream.Write($candidateBytes, 0, $candidateBytes.Length)
        $bytesWritten = $destinationStream.Position
        try {
            $destinationStream.Flush($true)
        }
        catch [System.IO.IOException] {
            if (Test-CompletedWriteAndDetachedVolume `
                    -BytesWritten $bytesWritten `
                    -ExpectedBytes $candidateBytes.Length `
                    -VolumeRoot $volumeRoot) {
                $acceptedDetach = $true
                Write-Warning 'All candidate bytes were written and the UF2 volume detached during flush; treating the bootloader handoff as successful.'
            }
            else {
                throw
            }
        }
    }
    finally {
        try {
            $destinationStream.Dispose()
        }
        catch [System.IO.IOException] {
            if (Test-CompletedWriteAndDetachedVolume `
                    -BytesWritten $bytesWritten `
                    -ExpectedBytes $candidateBytes.Length `
                    -VolumeRoot $volumeRoot) {
                if (-not $acceptedDetach) {
                    Write-Warning 'All candidate bytes were written and the UF2 volume detached during stream close; treating the bootloader handoff as successful.'
                }
            }
            else {
                throw
            }
        }
    }
    Write-Host "Candidate copied to the verified XIAO UF2 root: $destination"
}
