import { configDefaults, defineConfig } from 'vitest/config';
import { defineConfig as defineViteConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineViteConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    setupFiles: './src/test-setup.ts',
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
});
