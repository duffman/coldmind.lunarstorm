import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// Compiler-only consumer checks stay separate from runtime and source-only checks.
export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  test: {
    name: '@coldmind/inject:types',
    include: [],
    typecheck: {
      enabled: true,
      checker: 'tsc',
      include: ['tests/types/**/*.test-d.ts'],
      tsconfig: './tsconfig.types.json',
      ignoreSourceErrors: false,
    },
  },
});
