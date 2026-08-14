import { defineConfig, mergeConfig } from 'vitest/config';
import viteConfig from './vite.config';

/** UI and transport seams exercised with a simulated or mocked bridge. */
export default mergeConfig(viteConfig, defineConfig({
  test: {
    include: [
      'src/App.test.tsx',
      'src/KeymapEditor.test.tsx',
      'src/VirtualKeyboard.test.tsx',
      'src/device.test.ts',
      'src/injection.test.ts',
      'src/simulatedBridge.test.ts',
      'src/webhid.test.ts',
    ],
  },
}));
