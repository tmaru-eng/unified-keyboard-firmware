#Requires -Version 5.1

<#
.SYNOPSIS
Repairs directory symlinks that Git for Windows checked out as plain text files.

.DESCRIPTION
`nrf-sdc-sys/third_party` is a symlink to `../nrf-mpsl-sys/third_party`. Git for
Windows ships `core.symlinks=false` in its system config because creating a
symbolic link needs either administrator rights or Windows Developer Mode. With
that setting, git writes a small text file containing the link target instead of
a link, and `nrf-sdc-sys/build.rs` then fails with

    Unable to generate bindings: NotExist(".../softdevice_controller/include/sdc_soc.h")

Directory junctions do not need elevation, so this script replaces each broken
link with a junction pointing at the same directory.

The proper fix is to enable Windows Developer Mode and set
`git config --global core.symlinks true`, after which cargo checks the tree out
correctly and this script becomes unnecessary. Run this only until that is done.

Cargo re-creates `~/.cargo/git/checkouts` when it refetches a git dependency, so
this may need to be re-run after `cargo clean` or a dependency revision change.

.EXAMPLE
.\Repair-CargoGitSymlinks.ps1 -WhatIf
#>

[CmdletBinding(SupportsShouldProcess = $true)]
param(
    # Root to scan. Defaults to cargo's git checkout cache.
    [string]$CheckoutRoot = (Join-Path $env:USERPROFILE '.cargo\git\checkouts'),

    # Largest file that will be considered a stand-in for a symlink. A real
    # link target path is short; anything larger is left alone.
    [int]$MaxLinkFileBytes = 1024
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $CheckoutRoot)) {
    Write-Host "No cargo git checkout cache at $CheckoutRoot; nothing to repair."
    return
}

$repaired = 0
$candidates = Get-ChildItem -LiteralPath $CheckoutRoot -Recurse -File -Force `
    -ErrorAction SilentlyContinue |
    Where-Object { $_.Length -gt 0 -and $_.Length -le $MaxLinkFileBytes -and -not $_.Extension }

foreach ($candidate in $candidates) {
    $content = [System.IO.File]::ReadAllText($candidate.FullName)
    # A checked-out-as-text symlink is exactly the target path: one line, no
    # newline, and it must resolve to a directory next to the placeholder.
    if ($content -match '[\r\n]' -or $content -notmatch '[\\/]') {
        continue
    }

    $target = Join-Path $candidate.DirectoryName $content
    if (-not (Test-Path -LiteralPath $target -PathType Container)) {
        continue
    }
    $resolved = (Resolve-Path -LiteralPath $target).ProviderPath

    if ($PSCmdlet.ShouldProcess($candidate.FullName, "Replace text placeholder with a junction to $resolved")) {
        Remove-Item -LiteralPath $candidate.FullName -Force
        New-Item -ItemType Junction -Path $candidate.FullName -Target $resolved | Out-Null
        Write-Host "Repaired: $($candidate.FullName) -> $resolved"
        $repaired++
    }
}

if ($WhatIfPreference) {
    # ShouldProcess reported each candidate above; the counter stays at zero
    # because nothing was written.
    return
}
if ($repaired -eq 0) {
    Write-Host 'No broken directory symlinks found.'
}
else {
    Write-Host "Repaired $repaired link(s)."
}
