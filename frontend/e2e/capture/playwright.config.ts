/** No webServer: the URL names the integrator's retained, already running world. */
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: 'review.capture.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: 'line',
  outputDir: '../../.tmp/capture-playwright',
  use: { browserName: 'chromium', headless: true, trace: 'off', video: 'off', screenshot: 'off',
    launchOptions: { args: ['--disable-gpu'] } },
});
