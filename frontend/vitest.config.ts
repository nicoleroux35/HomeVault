import path from 'path';
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';

export default defineConfig({
  // `svelteTesting()` switches Svelte to its browser build and wires the
  // testing-library cleanup. Without it Vitest resolves the SSR entry and
  // `mount()` throws `lifecycle_function_unavailable` — every component test
  // in `src/lib/components/**/__snapshots__/` fails.
  plugins: [svelte(), svelteTesting()],
  test: {
    globals: true,
    environment: 'happy-dom',
    setupFiles: ['./tests/setup.ts'],
    include: ['src/**/*.{test,spec}.{js,ts}'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'html', 'lcov'],
      exclude: [
        'node_modules/',
        'tests/',
        '**/*.spec.ts',
        '**/*.test.ts',
        '**/types.ts',
      ],
      thresholds: {
        lines: 60,
        functions: 60,
        branches: 60,
        statements: 60,
      },
    },
    snapshotFormat: {
      printBasicPrototype: false,
    },
  },
  resolve: {
    alias: {
      $lib: path.resolve(import.meta.dirname, './src/lib'),
      $app: path.resolve(import.meta.dirname, './src'),
    },
  },
});
