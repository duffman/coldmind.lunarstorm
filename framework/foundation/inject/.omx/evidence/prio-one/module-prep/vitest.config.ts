import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

/** Isolates pure module preparation from the active core suites and intentionally red G04 fixtures. */
export default defineConfig({
  root: fileURLToPath(new URL('../../../../', import.meta.url)),
  test: {
    name: '@coldmind/inject:module-prep',
    include: ['.omx/prepared-tests/module-definition.test.ts'],
    setupFiles: ['tests/runtime.setup.ts'],
    typecheck: {
      enabled: true,
      checker: 'tsc',
      include: ['.omx/prepared-tests/module-types.test-d.ts'],
      tsconfig: '.omx/evidence/prio-one/module-prep/tsconfig.json',
      ignoreSourceErrors: false,
    },
  },
});
