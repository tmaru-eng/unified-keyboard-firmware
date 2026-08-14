import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';

/** Pure codecs, validators, and protocol framing have no React or browser boundary. */
export default mergeConfig(viteConfig, defineConfig({
  test: {
    include: [
      'src/bondManagement.test.ts',
      'src/commands.test.ts',
      'src/configTransfer.test.ts',
      'src/diagnostics.test.ts',
      'src/keymap.test.ts',
      'src/profile.test.ts',
      'src/transfer.test.ts',
    ],
  },
}));
