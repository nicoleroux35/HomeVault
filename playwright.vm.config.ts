import { defineConfig } from '@playwright/test';

/**
 * Playwright config for **real-VM** tests (`*.vm.test.ts`).
 *
 * These are deliberately separate from the default `playwright.config.ts`:
 *
 * - The default suite runs against a locally-started server with a scratch
 *   `DATA_DIRECTORY` and never touches real hardware.
 * - The VM suite runs against a **real, already-provisioned HomeVault host**
 *   (a VM or a spare box) reached over the network, so it can assert the
 *   things a scratch server cannot: real disks / SMART, real systemd units,
 *   a real reboot, real mDNS. It is opt-in and never part of the default
 *   `npx playwright test` run.
 *
 * Usage:
 *   HV_VM_BASE_URL=http://hv-test.local:8080 \
 *   HV_VM_ADMIN_USER=admin HV_VM_ADMIN_PASS=… \
 *   npx playwright test --config=playwright.vm.config.ts
 *
 * Without `HV_VM_BASE_URL` the specs skip themselves with a clear message
 * (there is no VM to talk to), so the file can live in the repo and be wired
 * into CI as an optional job.
 */
export default defineConfig({
  testDir: './tests/vm',
  testMatch: '**/*.vm.test.ts',
  timeout: 180_000,
  expect: { timeout: 20_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-vm' }]],
  use: {
    baseURL: process.env.HV_VM_BASE_URL,
    headless: true,
    viewport: { width: 1280, height: 720 },
    actionTimeout: 20_000,
    navigationTimeout: 30_000,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    ignoreHTTPSErrors: true,
  },
  projects: [{ name: 'vm-firefox', use: { browserName: 'firefox' } }],
});
