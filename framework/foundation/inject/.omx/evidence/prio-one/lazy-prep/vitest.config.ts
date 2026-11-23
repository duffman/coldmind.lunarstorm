import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

/** Prepared leaf cases only; never includes active suites or unrelated red fixtures. */
export default defineConfig({
  root: fileURLToPath(new URL('../../../../', import.meta.url)),
  test: { name: '@coldmind/inject:lazy-prep', include: ['.omx/prepared-tests/lazy-leaf.test.ts'], setupFiles: ['tests/runtime.setup.ts'] },
});
