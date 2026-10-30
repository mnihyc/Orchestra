/** A newcomer chooses a driver example, understands limits, fixes a field and keeps the same values in JSON. */
import { expect, test } from '@playwright/test';
import { bodySent, scenario, signIn } from './support.ts';
import { templateJson, templateSection } from './template-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function templateHosts(body: Record<string, unknown>): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/resources/template-hosts`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused the host scenario: ${response.status}`);
}

test.beforeEach(async () => { await scenario(); });

test('explains template purposes with one guide for the screen',async({page})=>{
  await signIn(page);await page.goto('/resources/templates');
  await page.getByRole('button',{name:'Add template'}).click();
  const dialog=page.getByRole('dialog',{name:'Add template'});
  await expect(dialog.getByRole('radio',{name:'Environment',exact:true})).toBeChecked();
  const provider=dialog.getByRole('radio',{name:'Provider',exact:true});
  await expect(provider).not.toHaveAttribute('aria-describedby');
  await expect(dialog.getByText("Keep a provider's credential and run its harness.",{exact:true})).toHaveCount(0);
  await expect(dialog.locator('[data-help-article="write-an-environment-template"]')).toHaveCount(1);
  await expect(dialog.getByRole('button',{name:'About Provider',exact:true})).toHaveCount(0);
  await dialog.getByRole('button',{name:'Write an environment template',exact:true}).click();
  await expect(page.getByRole('heading',{name:'Write an environment template',exact:true}).first()).toBeVisible();
  await expect(page.getByRole('heading',{name:'Provider',exact:true})).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('radio',{name:'Environment',exact:true})).toBeChecked();
  await expect(dialog.getByRole('radio',{name:'Provider',exact:true})).not.toBeChecked();
  await expect(dialog.getByLabel('Image reference')).toHaveValue('node:22-bookworm');
});

test('selects and saves the host program example with its account and command filled in', async ({ page }) => {
  await signIn(page); await page.goto('/resources/templates');
  const writes: Promise<unknown>[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && request.url().endsWith('/api/v1/templates')) writes.push(bodySent(request)); });
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await dialog.getByRole('button', { name: 'Start from an example', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Node', exact: true })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Host', exact: true }).click();
  await expect(dialog.getByRole('combobox', { name: 'Driver', exact: true })).toContainText('Host');
  await expect(dialog.getByLabel('Run as user')).toHaveValue('agent');
  await expect(dialog.getByLabel('Image reference')).toHaveCount(0);
  await expect(dialog.getByLabel('Program and arguments · one per line')).toHaveCount(0);
  await dialog.getByLabel('Name', { exact: true }).fill('Host tools');
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'Host tools', level: 1 })).toBeVisible();
  expect(await Promise.all(writes)).toEqual([expect.objectContaining({ driver: 'host', spec: { run_as: 'agent', after_up: [{ name: '10-ready', command: ['/usr/bin/printf', 'Host tools are ready.'] }] } })]);
  await page.getByRole('tab', { name: 'Configuration', exact: true }).click();
  await templateSection(page, 'User');
  await expect(page.getByLabel('Run as user')).toHaveValue('agent');
  await templateSection(page, 'Commands after start');
  await page.getByRole('button', { name: 'Edit Commands after start', exact: true }).click();
  await expect(page.getByLabel('Program and arguments · one per line')).toHaveValue('/usr/bin/printf\nHost tools are ready.');
  await expect(page.getByRole('button', { name: 'Write an environment template', exact: true })).toHaveCount(1);
});

test('names enrolled offline hosts and their recorded times instead of advising enrollment', async ({ page }) => {
  const since = '2026-10-15T09:00:00Z';
  await templateHosts({ state: 'offline', since, reported: false });
  await signIn(page); await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await expect(dialog.getByText('Enrolled but offline', { exact: true })).toBeVisible();
  await expect(dialog.getByText(/host-1 · offline since/)).toBeVisible();
  await expect(dialog.getByText(/build-host · offline since/)).toBeVisible();
  await expect(dialog.locator(`time[datetime="${since}"]`)).toHaveCount(2);
  await expect(dialog.getByText('No host available', { exact: true })).toHaveCount(0);
  await expect(dialog.getByText(/Enroll one/)).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Add template' })).toBeDisabled();
});

test('shows cached examples and saves a template while the enrolled hosts are offline', async ({ page }) => {
  await templateHosts({ state: 'offline', since: '2026-10-15T09:00:00Z', reported: true });
  await signIn(page); await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await expect(dialog.getByText('Enrolled but offline', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Start from an example', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Node', exact: true }).click();
  await expect(dialog.getByLabel('Image reference')).toHaveValue('node:22-bookworm');
  await dialog.getByLabel('Name', { exact: true }).fill('Ready when hosts reconnect');
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'Ready when hosts reconnect', level: 1 })).toBeVisible();
});

test('offers enrollment when there is no enrolled or shared host', async ({ page }) => {
  await templateHosts({ state: 'none', reported: false });
  await signIn(page); await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await expect(dialog.getByText('No host available', { exact: true })).toBeVisible();
  await expect(dialog.getByText('No host is enrolled or shared with you yet. Enroll one or ask its owner for access.')).toBeVisible();
  await expect(dialog.getByText('Enrolled but offline', { exact: true })).toHaveCount(0);
});

test('makes a web template from an example with explained fields and server validation before saving', async ({ page }) => {
  await signIn(page); await page.goto('/resources/templates');
  const writes: Promise<unknown>[] = [];
  const updates: Promise<unknown>[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && request.url().endsWith('/api/v1/templates')) writes.push(bodySent(request)); });
  page.on('request', (request) => { if (request.method() === 'PATCH' && request.url().includes('/api/v1/templates/')) updates.push(bodySent(request)); });
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await dialog.getByLabel('Name', { exact: true }).fill('First web environment');
  await dialog.getByRole('button', { name: 'Start from an example', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Rust', exact: true }).click();
  await expect(dialog.getByLabel('Image reference')).toHaveValue('rust:1-bookworm');
  await dialog.getByRole('button', { name: 'Start from an example', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Node', exact: true }).click();
  await expect(dialog.getByLabel('Image reference')).toHaveValue('node:22-bookworm');
  await expect(dialog.getByRole('spinbutton')).toHaveCount(0);
  await expect(dialog.getByRole('switch', { name: 'JSON', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'First web environment', level: 1 })).toBeVisible();
  expect(await Promise.all(writes)).toHaveLength(1);
  await page.getByRole('tab', { name: 'Configuration', exact: true }).click();
  await templateSection(page, 'What it uses');
  const limits = page.getByRole('group', { name: 'Limits', exact: true });
  await expect(limits.getByRole('spinbutton', { name: 'Processor time' })).toHaveValue('4000');
  await expect(limits.getByText(/Thousandths of a core/)).toBeVisible();
  const memory = limits.getByRole('spinbutton', { name: 'Memory', exact: true });
  await memory.fill('0');
  await expect(memory).toHaveAttribute('aria-invalid', 'true');
  await expect(memory).toHaveAccessibleDescription('the number is outside the allowed range');
  expect(await Promise.all(updates)).toHaveLength(0);
  await limits.getByRole('button', { name: 'More about Memory', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset to default', exact: true }).click();
  await expect(memory).toHaveValue('');
  await memory.fill('6144');
  await expect(memory).not.toHaveAttribute('aria-invalid', 'true');
  await templateJson(page);
  const editor = page.getByLabel('Configuration (JSON)');
  const spec = JSON.parse(await editor.innerText()) as Record<string, unknown>;
  expect(spec.resources).toMatchObject({ memory_mb: 6144, cpu_millis: 4000 });
  await editor.fill(JSON.stringify({ ...spec, env: { PROJECT_MODE: 'local development' } }, null, 2));
  await page.getByRole('switch', { name: 'JSON', exact: true }).click();
  await templateSection(page, 'Variables');
  const variables = page.getByRole('region', { name: 'Variables' });
  await variables.getByRole('button', { name: 'Edit Variables', exact: true }).click();
  await expect(variables.getByLabel('Name', { exact: true })).toHaveValue('PROJECT_MODE');
  await expect(variables.getByLabel('Value', { exact: true })).toHaveValue('local development');
  await page.getByRole('button', { name: 'Save template', exact: true }).click();
  await expect(page.getByText('First web environment was saved', { exact: true })).toBeVisible();
  expect(await Promise.all(updates)).toHaveLength(1);
  await templateSection(page, 'Image');
  await expect(page.getByLabel('Image reference')).toHaveValue('node:22-bookworm');
  await templateSection(page, 'What it uses');
  await expect(page.getByRole('group', { name: 'Limits', exact: true }).getByRole('spinbutton', { name: 'Memory', exact: true })).toHaveValue('6144');
  await page.reload();
  await templateSection(page, 'Variables');
  await page.getByRole('button', { name: 'Edit Variables', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Variables' }).getByLabel('Value', { exact: true })).toHaveValue('local development');
});

test('JSON offers schema property completion, key help and errors that go to their line', async ({ page }) => {
  await signIn(page); await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await dialog.getByLabel('Name', { exact: true }).fill('JSON starter');
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'JSON starter', level: 1 })).toBeVisible();
  await page.getByRole('tab', { name: 'Configuration', exact: true }).click();
  await templateJson(page);
  const editor = page.getByLabel('Configuration (JSON)');
  await editor.fill('{\n  "ima');
  await editor.press('Control+End'); await editor.press('Control+Space');
  await expect(page.locator('.cm-tooltip-autocomplete').getByText('image', { exact: true })).toBeVisible();
  await editor.press('Escape');
  await editor.fill('{\n  "image": "node:22-bookworm",\n  "resources": {\n    "memory_mb": 0\n  }\n}');
  await expect(page.getByRole('button', { name: 'Line 4: the number is outside the allowed range' })).toBeVisible();
  await page.getByRole('button', { name: 'Line 4: the number is outside the allowed range' }).click();
  await expect(page.locator('.cm-activeLine')).toContainText('memory_mb');
  await editor.getByText('"image"', { exact: true }).hover();
  await expect(page.locator('.cm-tooltip-hover')).toContainText('Image reference');
  await expect(page.locator('.cm-tooltip-hover')).toContainText('image to pull from a registry');
});

test('the provider kind offers complete examples and its own fields without an environment workspace', async ({ page }) => {
  await signIn(page); await page.goto('/resources/templates');
  await page.getByRole('button', { name: 'Add template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add template' });
  await dialog.getByRole('radio', { name: 'Provider', exact: true }).click();
  await dialog.getByLabel('Name', { exact: true }).fill('Python provider runtime');
  await dialog.getByRole('button', { name: 'Start from an example', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Python', exact: true })).toHaveAccessibleDescription(/separately managed PostgreSQL/);
  await page.getByRole('menuitem', { name: 'Python', exact: true }).click();
  await expect(dialog.getByLabel('Image reference')).toHaveValue('python:3.13-bookworm');
  await expect(dialog.getByText(/separately managed PostgreSQL/)).toHaveCount(0);
  await expect(dialog.getByLabel('Workspace path')).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('heading', { name: 'Python provider runtime', level: 1 })).toBeVisible();
  await page.getByRole('tab', { name: 'Configuration', exact: true }).click();
  await expect(page.getByLabel('Workspace path')).toHaveCount(0);
  await templateSection(page, 'Registered programs');
  await expect(page.getByRole('region', { name: 'Registered programs' })).toBeVisible();
});
