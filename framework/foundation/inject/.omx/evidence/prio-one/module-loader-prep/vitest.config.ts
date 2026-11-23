import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
export default defineConfig({
  root: fileURLToPath(new URL('../../../../', import.meta.url)),
  test: {
    name: '@coldmind/inject:module-loader-prep',
    include: ['.omx/prepared-tests/module-loader.test.ts'],
    setupFiles: ['tests/runtime.setup.ts'],
  },
});
