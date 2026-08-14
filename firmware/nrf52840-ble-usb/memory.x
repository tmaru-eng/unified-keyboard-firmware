MEMORY
{
  /*
   * Seeed XIAO nRF52840 Sense factory UF2 layout (S140 7.3.0):
   *   0x00000000..0x00027000  MBR + SoftDevice (preserved)
   *   0x00027000..0x000E9000  application
   *   0x000E9000..0x000EA000  keymap record (preserved)
   *   0x000EA000..0x000EB000  profile record (preserved)
   *   0x000EB000..0x000EC000  pairing bond record (preserved)
   *   0x000EC000..0x000F4000  bootloader-managed storage (preserved)
   *   0x000F4000..0x00100000  UF2 bootloader (preserved)
   */
  /*
   * The three pages above the application are kept separate so configuration
   * updates never erase another kind of state. The keymap page is
   * 0x000E9000..0x000EA000, the profile page is 0x000EA000..0x000EB000, and
   * the bond page is 0x000EB000..0x000EC000. All stay out of FLASH so the
   * linker can never place code there. The page above them belongs to the
   * bootloader and is not ours to write.
   */
  FLASH : ORIGIN = 0x00027000, LENGTH = 0x000C2000
  /* Conservatively reserve 0x20000000..0x20010000 for S140 until the
   * configured sd_ble_enable call reports its required application RAM base
   * during physical bring-up.
   */
  RAM   : ORIGIN = 0x20010000, LENGTH = 0x00030000
}
