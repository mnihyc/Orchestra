/** Thread views remain useful without a workspace and direct environment doors use only their own grants. */
import { expect, test } from '@playwright/test';
import { begin, MAIN, MAIN_ENVIRONMENT, openThread, showView, workspace } from './inspector-support';
import { bodySent, ACME, deny, emit, PEOPLE, signIn } from './support';

test.use({ viewport: { width: 1600, height: 1000 } });
test.beforeEach(async () => { await begin(); });

test('History, People and Knowledge are actual thread views, including an unbound thread', async ({ page }) => {
  await emit(MAIN, { thread: { environment: null } });
  await openThread(page);
  await showView(page, /^History$/);
  await expect(page.getByTestId('thread-history')).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Kind', exact: true })).toBeVisible();
  await showView(page, /^People$/);
  const people = page.getByTestId('thread-people');
  await expect(people.getByRole('region', { name: 'Can view', exact: true })).toBeVisible();
  await people.getByRole('button', { name: 'Access…', exact: true }).click();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');
  await showView(page, /^Knowledge$/);
  await expect(page.getByText('This agent reads nothing but its own thread.', { exact: true })).toBeVisible();
  await showView(page, /^Diagnostics$/);
  await expect(page.getByTestId('thread-diagnostics')).toContainText('This thread has no agent segments yet.');
});

test('Diagnostics names its Manage requirement without requesting raw frames for a viewer', async ({ page }) => {
  await deny('vera', ['thread.manage']);
  const requested: string[] = [];
  page.on('request', (request) => { if (new URL(request.url()).pathname.endsWith('/raw')) requested.push(request.url()); });
  await openThread(page, PEOPLE.vera);
  await showView(page, /^Diagnostics$/);
  await expect(page.getByText('Managing this thread is required to read its diagnostics.')).toBeVisible();
  expect(requested).toEqual([]);
});

test('Knowledge shares the chosen source with this agent, keeping the thread as the recipient', async ({ page }) => {
  await openThread(page);
  await showView(page, /^Knowledge$/);
  await page.getByRole('button', { name: 'Share something with this agent…', exact: true }).click();
  await page.getByRole('button', { name: 'What to share: Choose a thing', exact: true }).click();
  await page.getByRole('textbox', { name: 'Find a place, thread or document', exact: true }).fill('Web');
  await page.getByRole('dialog', { name: 'What to share', exact: true }).getByRole('button', { name: /^Web(?:\s|$)/ }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Share “Web” with agents', exact: true });
  await expect(dialog).toContainText('Main thread');
  const sent = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/share/agents'));
  await dialog.getByRole('button', { name: /^Let the agents of Main thread read/ }).click();
  const body = (await bodySent(await sent)) as { what: { node_id: string }; to: { thread_id: string }[] };
  expect(body.what.node_id).toBe(ACME.web);
  expect(body.to).toEqual([{ thread_id: MAIN }]);
});

test('Files and Terminal open from the environment without naming a thread in either request', async ({ page }) => {
  await signIn(page);
  const through: string[] = [];
  page.on('request', (request) => { const url = new URL(request.url()); if (url.pathname.includes(`/environments/${MAIN_ENVIRONMENT}/`) && url.searchParams.has('thread')) through.push(url.pathname); });
  await page.goto(`/resources/environments/${MAIN_ENVIRONMENT}?tab=files`);
  await expect(page.getByTestId('environment-files')).toBeVisible();
  await expect(page.getByRole('tree', { name: /^Files of/ })).toBeVisible();
  await page.getByRole('tab', { name: 'Terminal', exact: true }).click();
  await expect(page.getByTestId('environment-terminal')).toBeVisible();
  await page.getByRole('button', { name: 'Open a new shell', exact: true }).first().click();
  await expect(page.locator('.xterm-rows')).toContainText('$');
  expect(through).toEqual([]);
});

test('a thread viewer without environment doors sees their exact refusal on the environment page', async ({ page }) => {
  await workspace('/environment', { environment: MAIN_ENVIRONMENT, patch: { remote_write: false } });
  await deny('vera', ['files.view', 'files.manage', 'terminal.manage']);
  await signIn(page, PEOPLE.vera);
  await page.goto(`/resources/environments/${MAIN_ENVIRONMENT}?tab=files`);
  await expect(page.getByTestId('files-panel')).toHaveCount(0);
  await expect(page.getByText(/takes|requires|ask/i).first()).toBeVisible();
  await page.getByRole('tab', { name: 'Terminal', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Open a new shell', exact: true })).toHaveCount(0);
});
