/** Native-harness UI proofs run against an already deployed candidate with sign-ins supplied by its integration runner. */
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  testMatch: ['thread-native.spec.ts', 'release-routes-native.spec.ts', 'views-native.spec.ts', 'provider-sources-native.spec.ts'],
  fullyParallel: false,
  workers: 1,
  use: {
    ...devices['Desktop Chrome'],
    baseURL: process.env.ORCH_NATIVE_URL,
    // Native browser states and one-use route URLs can carry credentials; evidence uses safe screenshots and route patterns.
    trace: 'off',
    screenshot: 'only-on-failure',
  },
});
