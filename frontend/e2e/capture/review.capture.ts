/** Manual integrator tool; .capture.ts keeps it out of CI's fake-world *.spec.ts discovery. */
import { expect, test } from '@playwright/test';
import { relative } from 'node:path';
import { readConfig } from './config.ts';
import { runCapture, runTimeout } from './run.ts';

test('capture the running review world', async ({ browser }) => {
  const config = readConfig();
  test.setTimeout(runTimeout(config));
  const { directory, run } = await runCapture(browser, config);
  await test.info().attach('review-output', { body: relative(process.cwd(), directory), contentType: 'text/plain' });
  expect(run.screens.length, 'The capture must attempt every shared route/state/actor/size/theme.').toBe(run.expected_screens);
  expect(run.session_issues, 'Review requests.json for a sign-in, appearance or cleanup fault.').toEqual([]);
  expect(run.artifact_issues, 'Every HTML sheet needs its PNG. Originals are retained on failure.').toEqual([]);
  expect(run.screens.filter((row) => !row.ready || row.screenshot === null || row.issues.length !== 0).map((row) => row.number), 'Incomplete screens remain labelled in the contact sheets.').toEqual([]);
  expect(run.screens.filter((row) => !row.requests.budgets.idle_complete).map((row) => row.number), 'Every screen needs a full 30-second idle observation.').toEqual([]);
  // Budget violations retain their evidence and fail the capture command.
  expect(run.screens.filter((row) => !row.requests.budgets.first_load_ok || !row.requests.budgets.idle_ok).map((row) => row.number), 'Request budget: at most eight first-load API requests, zero in 30 seconds idle.').toEqual([]);
});
