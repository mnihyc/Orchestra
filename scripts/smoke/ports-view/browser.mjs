// A Ports browser walk against the release's real server and frontend; no routes or sockets are mocked.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(resolve(here, '../../../frontend/package.json'));
const { chromium, expect } = require('@playwright/test');
const [worldFile, output] = process.argv.slice(2);
if (!worldFile || !output) throw new Error('supply the owned release world and evidence directory');
const world = JSON.parse(await readFile(worldFile, 'utf8'));
const browser = await chromium.launch({ headless: true,
  args: ['--host-resolver-rules=MAP proof.orch.test 127.0.0.1', '--disable-dev-shm-usage'] });
const failures = [];
try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true, locale: 'en-US' });
  const page = await context.newPage();
  page.on('pageerror', (error) => failures.push(error.message));
  // Every timeout is a hang guard, never evidence that an asynchronous act has completed.
  page.setDefaultTimeout(60_000);
  await page.goto(`${world.system}/login`);
  await page.locator('input[autocomplete="username"]').fill('ada');
  await page.locator('input[autocomplete="current-password"]').fill('correct horse battery');
  await page.getByRole('button', { name: 'Sign in', exact: true }).click();
  await page.waitForURL((url) => url.pathname !== '/login');
  const api = async (path) => {
    // Use Chromium's authenticated origin and its fixture DNS mapping, including for these reads.
    const reply = await page.evaluate(async (route) => {
      const response = await fetch(`/api/v1${route}`, { credentials: 'same-origin' });
      return { status: response.status, body: await response.json() };
    }, path);
    assert.equal(reply.status, 200, `${path} must be served by the real API`);
    return reply.body;
  };
  const path = `/environments/${world.environment}/ports`;
  const before = await api(path);
  assert.equal(before.openings.length, 0, 'this fixture starts with no wider audience');
  await page.goto(`${world.system}/resources/environments/${world.environment}?tab=ports`);
  const panel = page.getByTestId('ports-panel');
  await expect(panel).toBeVisible();
  await panel.getByLabel('Port number', { exact: true }).fill('8080');
  await panel.getByRole('button', { name: 'Show port', exact: true }).click();
  const row = panel.getByTestId('port-8080');
  await expect(row).toBeVisible();
  const heading = await row.getByRole('heading').innerText();
  const target = await api(`${path}/8080`);
  assert.equal(target.revision, 0, 'selecting an unlisted port is a pure read');
  assert.deepEqual((await api(path)).openings, before.openings, 'selection grants no audience');
  await page.reload();
  await panel.getByLabel('Port number', { exact: true }).fill('8080');
  await panel.getByRole('button', { name: 'Show port', exact: true }).click();
  await expect(row.getByRole('heading')).toHaveText(heading);
  assert.equal((await api(`${path}/8080`)).address, target.address, 'the address survives another live read');

  const place = panel.getByTestId('port-grant-place');
  await place.getByRole('button', { name: 'Grant to this environment', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Grant to this environment', exact: true }).click();
  // No manual Refresh: the accepted mutation must invalidate the shared Ports projection.
  await expect(place.getByRole('button', { name: 'Take back', exact: true })).toBeVisible();
  const granted = await api(path);
  const capability = granted.capabilities.find((item) => item.key === 'port_place');
  assert.equal(capability.admission.source, 'grant');
  assert.ok(capability.admission.by.display_name, 'the grant names its giver');
  assert.equal(granted.openings.length, 0, 'a capability grant itself opens no socket');
  await place.getByRole('button', { name: 'Take back', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Take back', exact: true }).click();
  await expect(place.getByRole('button', { name: 'Grant to this environment', exact: true })).toBeVisible();
  assert.equal((await api(path)).capabilities.find((item) => item.key === 'port_place').admission, null);
  assert.equal(failures.length, 0, JSON.stringify(failures));
  await page.screenshot({ path: resolve(output, 'ports.png'), fullPage: true });
  await writeFile(resolve(output, 'browser-result.json'), JSON.stringify({
    passed: true, real_frontend: true, real_api: true, mocked_requests: 0,
    checks: ['unlisted pure read', 'stable address', 'grant names giver', 'grant opens nothing', 'grant and revoke refresh'],
    page_errors: failures,
  }, null, 2));
} catch (error) {
  await writeFile(resolve(output, 'browser-result.json'), JSON.stringify({ passed: false, message: String(error), page_errors: failures }));
  throw error;
} finally {
  await browser.close();
}
