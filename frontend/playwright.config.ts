/** The end-to-end tests: a real browser against the built application, with the fake API of `tests/doubles` behind it.
 *
 * The application is built once, when a run begins (`npm run build`), and served by `vite preview`, which sends `/api` and
 * `/health` to the fake API as the development server does: no module is handed over one by one for a test to wait for, and the
 * page is the page that is shipped. The development server is for work by hand: `ORCH_E2E_DEV=1` runs the tests against it
 * instead (hot reload under a test, and every page fetched module by module). Build as for any build: with `ORCH_BUILD_NUMBER_DIR`
 * set where the counters are not to move.
 *
 * Tests that need source modules carry the tag `@development`: the gallery of the shared controls is part of a
 * development build only (two tests in `accessibility/screens.spec.ts`, one in `accessibility/left-bar.spec.ts`), and two tests reach a module of the page by its source
 * path, for a mechanism no screen uses yet (the form that gives access to several things at once, `accessibility/give.spec.ts`,
 * and the list of keys, `e2e/finder.spec.ts`). The floating-layer matrix mounts the actual shared controls with long fixtures
 * from a source-only harness; ordinary screen layout remains checked on the built application. These tests run in a project
 * of their own against a development server that the run
 * starts beside the other (`ORCH_E2E_DEV_PORT`); with `ORCH_E2E_DEV=1` every test runs there and the project is not needed.
 *
 * One browser, one worker: the fake API holds one state, which each test puts into the scenario it
 * needs. The servers listen on this machine alone; the ports can be moved with `ORCH_E2E_PORT`, `ORCH_E2E_DEV_PORT` and
 * `ORCH_DOUBLE_PORT` where they are taken. The browser is started without the GPU (`--disable-gpu`): the headless shell
 * would otherwise start a process that draws with a software GL, which takes processors from everything else on the
 * machine and gives a test nothing it looks at.
 */
import { defineConfig, devices } from '@playwright/test';

const DOUBLE_PORT = Number(process.env.ORCH_DOUBLE_PORT ?? 41871);
const APP_PORT = Number(process.env.ORCH_E2E_PORT ?? 41872);
const DEV_PORT = Number(process.env.ORCH_E2E_DEV_PORT ?? 41873);
const HOST = '127.0.0.1';
/** Whether the tests run against the development server (work by hand) and not against the built application. */
const DEV = process.env.ORCH_E2E_DEV === '1';
/** Whether the run needs no development server beside the application (none of the tests it was asked for needs one): it would keep memory all the run. */
const NO_DEV_SERVER = process.env.ORCH_E2E_NO_DEV_SERVER === '1';
/** What the tests that need the development server carry in their title (see the head comment). */
const NEEDS_DEVELOPMENT_SERVER = /@development/;
// What the server is, for the budgets that depend on it (the frame a tree row may cost is the figure of the specification for a
// production build): a build that is served is a production build, the development server is not. Said by hand, it is kept.
process.env.ORCH_PERF_BUILD ??= DEV ? 'development' : 'production';

export default defineConfig({
  testDir: '.',
  testMatch: ['tests/e2e/**/*.spec.ts', 'tests/accessibility/**/*.spec.ts', 'tests/performance/**/*.spec.ts', 'e2e/**/*.spec.ts'],
  outputDir: 'test-results',
  fullyParallel: false,
  workers: 1,
  forbidOnly: process.env.CI !== undefined,
  // Chromium gives up every request in flight when a network interface of the machine comes or goes
  // (a container that starts beside it is enough), and a page that lost its modules stays empty. That
  // is no fault of the page, so a test that failed is given two more chances before it counts.
  retries: 2,
  // On a machine that is busy with other work a page can take seconds to come (and the development server hands the
  // application over module by module, anew for every test), so a test waits for what it expects far longer than a
  // person would, and fails only when the thing never comes.
  timeout: 120_000,
  expect: { timeout: 30_000 },
  reporter: [['list']],
  use: {
    baseURL: `http://${HOST}:${APP_PORT}`,
    launchOptions: { args: ['--disable-gpu'] },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    locale: 'en-US',
    timezoneId: 'UTC',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] }, ...(DEV ? {} : { grepInvert: NEEDS_DEVELOPMENT_SERVER }) },
    ...(DEV ? [] : [{ name: 'development', use: { ...devices['Desktop Chrome'], baseURL: `http://${HOST}:${DEV_PORT}` }, grep: NEEDS_DEVELOPMENT_SERVER }]),
  ],
  webServer: [
    {
      command: 'node tests/doubles/server.ts',
      url: `http://${HOST}:${DOUBLE_PORT}/health`,
      env: { ORCH_DOUBLE_PORT: String(DOUBLE_PORT) },
      reuseExistingServer: process.env.CI === undefined,
    },
    {
      // Built once for the whole run, then served: the first page waits for the build, so the server is given the time it takes.
      command: DEV ? `npm run dev -- --port ${APP_PORT} --strictPort` : `npm run build && npx vite preview --port ${APP_PORT} --strictPort`,
      url: `http://${HOST}:${APP_PORT}/login`,
      timeout: DEV ? 60_000 : 600_000,
      env: { ORCH_DEV_API: `http://${HOST}:${DOUBLE_PORT}` },
      reuseExistingServer: process.env.CI === undefined,
    },
    // The development server for the tests that need it: it only waits until the project of those tests is reached.
    ...(DEV || NO_DEV_SERVER
      ? []
      : [
          {
            command: `npm run dev -- --port ${DEV_PORT} --strictPort`,
            url: `http://${HOST}:${DEV_PORT}/login`,
            timeout: 60_000,
            env: { ORCH_DEV_API: `http://${HOST}:${DOUBLE_PORT}` },
            reuseExistingServer: process.env.CI === undefined,
          },
        ]),
  ],
});
