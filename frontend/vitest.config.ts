import path from 'node:path';
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { svelteTesting } from '@testing-library/svelte/vite';

/**
 * Vitest — the frontend unit/component runner.
 *
 * Until this file existed the Svelte stores, the pure helpers and the shared
 * page composites were only exercised indirectly through the (slow, fragile)
 * Playwright suites. This runner closes that gap: `npm test` runs in seconds,
 * needs no server and no browser, and `npm run test:coverage` reports which
 * branches no test ever executes.
 *
 * Svelte 5 components are mounted with `@testing-library/svelte`; the DOM is
 * jsdom. The alias mirrors `vite.config.ts` so `$lib/...` imports resolve
 * exactly as they do in the build.
 */
export default defineConfig({
  // `svelteTesting()` switches Svelte to its browser build and wires the
  // testing-library cleanup; without it Vitest resolves the SSR entry and
  // `mount()` throws `lifecycle_function_unavailable`.
  plugins: [svelte(), svelteTesting()],
  resolve: {
    alias: {
      $lib: path.resolve(import.meta.dirname, './src/lib'),
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.{test,spec}.{ts,js}'],
    exclude: ['**/node_modules/**', '**/dist/**'],
    restoreMocks: true,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov', 'html'],
      reportsDirectory: './coverage',
      // Start with the layers this runner owns. Screens and the desktop shell
      // are exercised by Playwright; including them here would only inflate
      // the denominator with untested files. Ratchet these numbers up as
      // tests are added — never down.
      include: [
        'src/lib/utils/**/*.ts',
        'src/lib/stores/**/*.ts',
        'src/lib/components/app/**/*.svelte',
      ],
      exclude: ['src/**/*.test.ts', 'src/**/*.spec.ts', 'src/test/**'],
      thresholds: {
        // Baseline measured on 2026-11-11 with the first 54 tests. This is a
        // RATCHET: raise it as tests are added, never lower it. The huge
        // stores (auth, monitorStore, fileProvider) dominate the denominator
        // and are the next targets.
        lines: 17,
        functions: 13,
        branches: 15,
        statements: 16,
      },
    },
  },
});
