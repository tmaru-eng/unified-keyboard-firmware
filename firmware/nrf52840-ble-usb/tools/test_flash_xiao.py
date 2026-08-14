#!/usr/bin/env python3
"""Structure tests for the fail-closed XIAO UF2 flashing helper."""

from __future__ import annotations

import re
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("flash_xiao.ps1")


class FlashXiaoStructureTests(unittest.TestCase):
    """Keep the destructive boundary visible and reviewable without a device."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.source = SCRIPT.read_text(encoding="utf-8")

    def test_requires_explicit_write_intent_and_supports_dry_run(self) -> None:
        self.assertIn("SupportsShouldProcess = $true", self.source)
        self.assertIn("ConfirmImpact = 'High'", self.source)
        self.assertRegex(self.source, r"\[switch\]\s*\$ConfirmFlash")
        self.assertIn("$WhatIfPreference", self.source)
        self.assertIn("$PSCmdlet.ShouldProcess", self.source)
        self.assertIn("-Confirm:$false", self.source)
        self.assertIn("function Invoke-FlashShouldProcess", self.source)
        self.assertIn("Invoke-FlashShouldProcess $destination", self.source)

    def test_detects_only_info_uf2_roots_and_checks_exact_board_profile(self) -> None:
        self.assertIn("[System.IO.DriveInfo]::GetDrives()", self.source)
        self.assertIn("[System.IO.DriveType]::Removable", self.source)
        self.assertIn("INFO_UF2.TXT", self.source)
        self.assertIn("Seeed_XIAO_nRF52840_Sense", self.source)
        self.assertIn("S140", self.source)
        self.assertIn("7.3.0", self.source)
        self.assertRegex(self.source, r"Board-ID:\\s\*")
        self.assertRegex(self.source, r"SoftDevice:\\s\*")
        self.assertIn("$boardMatches.Count -ne 1", self.source)
        self.assertIn("$softDeviceMatches.Count -ne 1", self.source)

    def test_validates_candidate_with_existing_family_aware_packer(self) -> None:
        self.assertIn("xiao_uf2.py", self.source)
        self.assertRegex(self.source, r"(?i)\bcheck\b")
        self.assertIn("0xADA52840", self.source)

    def test_backup_is_timestamped_and_cannot_overwrite(self) -> None:
        self.assertIn("CURRENT.UF2", self.source)
        self.assertRegex(self.source, r"yyyyMMdd-HHmmssfff")
        self.assertRegex(
            self.source,
            re.compile(r"\[System\.IO\.File\]::Copy\([^\n]+,\s*\$false\)"),
        )
        self.assertNotRegex(self.source, r"(?i)Copy-Item[^\n]*-Force")

    def test_only_candidate_is_written_to_a_direct_child_of_drive_root(self) -> None:
        self.assertIn("Assert-DirectChildOfDriveRoot", self.source)
        self.assertRegex(
            self.source,
            r"Join-Path\s+\$volumeRoot\s+\$sourceFile\.Name",
        )
        self.assertIn("[System.IO.FileMode]::CreateNew", self.source)
        self.assertIn("$destinationStream.Write($candidateBytes", self.source)
        self.assertNotRegex(self.source, r"(?i)Remove-Item|Move-Item|Set-Content")

    def test_validated_snapshot_closes_candidate_toctou_window(self) -> None:
        self.assertIn("$candidateBytes = [System.IO.File]::ReadAllBytes", self.source)
        self.assertIn("[System.IO.FileShare]::Read", self.source)
        self.assertRegex(
            self.source,
            r"Invoke-Uf2Validation\s+-SourceFile\s+\(Get-Item\s+-LiteralPath\s+\$stagedPath\)",
        )

    def test_rejects_reparse_points_for_non_device_write_paths(self) -> None:
        self.assertIn("function Assert-NoReparsePointPath", self.source)
        self.assertIn("[System.IO.FileAttributes]::ReparsePoint", self.source)
        self.assertIn(
            "Assert-NoReparsePointPath -Path $sourceFile.FullName", self.source
        )
        self.assertIn("Assert-NoReparsePointPath -Path $backupRoot", self.source)
        self.assertIn("Assert-NoReparsePointPath -Path $temporaryRoot", self.source)

    def test_accepts_only_complete_write_followed_by_bootloader_detach(self) -> None:
        self.assertIn("function Test-CompletedWriteAndDetachedVolume", self.source)
        self.assertIn("function Test-Uf2VolumeDetached", self.source)
        self.assertIn("catch [System.IO.IOException]", self.source)
        self.assertIn("$BytesWritten -eq $ExpectedBytes", self.source)
        self.assertIn("[System.IO.DriveInfo]::GetDrives()", self.source)
        self.assertIn("-not $matchingDrives[0].IsReady", self.source)
        self.assertNotIn("[System.IO.Directory]::Exists($VolumeRoot)", self.source)
        self.assertIn("throw", self.source)
        self.assertRegex(
            self.source,
            r"Write-Warning[^\n]+UF2 volume detached",
        )


if __name__ == "__main__":
    unittest.main()
