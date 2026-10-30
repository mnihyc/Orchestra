/** Paste and drop proofs run against an already built and served application. */
import { defineConfig } from '@playwright/test';
import base from './playwright.config';

export default defineConfig(base, {
  testMatch: ['tests/e2e/paste-drop.spec.ts'],
  projects: [base.projects?.find((project) => project.name === 'chromium') ?? { name: 'chromium' }],
  webServer: [],
  outputDir: process.env.ORCH_PASTE_OUTPUT ?? 'test-results/paste-drop',
});
