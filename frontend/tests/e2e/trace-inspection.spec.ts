/** Retained and purged content is inspected under its own restore guard without writes, subscriptions or live workspace work. */
import { expect, test } from '@playwright/test';
import { TRACE } from '../doubles/fixtures';
import { ACME, deny, scenario, signIn, threadHeld } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41800'}`;
interface Recorded { requests: { method: string; path: string; query: Record<string, string> }[]; frames: { op?: string; feed?: string }[] }
async function recorded(): Promise<Recorded> { return (await fetch(`${DOUBLE}/__double/trace/state`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).json() as Promise<Recorded>; }
test.beforeEach(async () => { await scenario(); });

test('a deleted row has its name, person, day and independently guarded content acts', async ({ page }) => {
  await signIn(page); await page.goto('/org/acme/settings/audit?kind=deleted');
  const row = page.locator(`[data-trace-row][data-trace-object="${TRACE.thread}"]`); await expect(row).toHaveCount(1); await expect(row).toContainText('Deleted by Dora Developer'); await expect(row.getByRole('link', { name: 'Look at it', exact: true })).toBeVisible(); await expect(row.getByRole('button', { name: 'Bring it back', exact: true })).toBeVisible();
  const gone = page.locator('[data-trace-row]:visible').filter({ hasText: 'Purged draft' }); await expect(gone).toContainText('Gone on'); await expect(gone.getByRole('link', { name: 'Look at it' })).toHaveCount(0); await expect(gone.getByRole('button', { name: 'Bring it back' })).toHaveCount(0);
  await deny('ada', [`topic.manage@${ACME.fixLogin}`]); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(row.getByRole('link', { name: 'Look at it' })).toHaveCount(0); await expect(row).toContainText('restoration authority');
});

test('retained conversation reuses the timeline and sends no read, notification, subscription, input or environment act', async ({ page }) => {
  await signIn(page); const before = await threadHeld(TRACE.thread);
  const reply = page.waitForResponse((response) => new URL(response.url()).pathname.endsWith(`/threads/${TRACE.thread}/sync`));
  await page.goto(`/inspect/thread/${TRACE.thread}`);
  const wire: unknown = await (await reply).json();
  expect(wire).toMatchObject({ thread: { inspection: { readonly: true } } });
  expect(wire).not.toHaveProperty('inspection');
  await expect(page.getByText('Read-only inspection. Reading changes no read marks or notifications.')).toBeVisible(); await expect(page.locator('[data-stored-timeline] [data-item]').first()).toBeVisible(); await expect(page.locator('[contenteditable=true]')).toHaveCount(0); await expect(page.getByRole('button', { name: 'Send', exact: true })).toHaveCount(0);
  await page.waitForTimeout(2500); const after = await threadHeld(TRACE.thread); expect(after.read).toEqual(before.read); expect(after.inputs).toEqual(before.inputs);
  const traffic = await recorded(); const writes = traffic.requests.filter((request) => request.method !== 'GET' && /\/(read|seen|subscription|inputs|start|ops)$/.test(request.path)); expect(writes).toEqual([]); expect(traffic.frames.filter((frame) => frame.op === 'sub' && frame.feed === `thread:${TRACE.thread}`)).toEqual([]); expect(traffic.requests.filter((request) => request.path.startsWith('/environments/') && !request.query.inspect_deleted)).toEqual([]);
});

test('retained document uses the block renderer without editing, seen writes or a live document feed', async ({ page }) => {
  await signIn(page);
  const reply = page.waitForResponse((response) => new URL(response.url()).pathname.endsWith(`/documents/${TRACE.document}/sync`));
  await page.goto(`/inspect/document/${TRACE.document}`);
  const wire: unknown = await (await reply).json();
  expect(wire).toMatchObject({ document: { inspection: { readonly: true } } });
  expect(wire).not.toHaveProperty('inspection');
  await expect(page.getByRole('heading', { name: 'Investigation notes' })).toBeVisible(); await expect(page.getByRole('textbox', { name: 'Document text' })).toHaveAttribute('contenteditable', 'false'); await expect(page.getByText('The model and the workspace were used separately.')).toBeVisible(); await page.keyboard.press('Control+k'); await page.waitForTimeout(2500);
  const traffic = await recorded(); expect(traffic.requests.filter((request) => request.method !== 'GET' && request.path.startsWith(`/documents/${TRACE.document}`))).toEqual([]); expect(traffic.frames.filter((frame) => frame.op === 'sub' && frame.feed === `doc:${TRACE.document}`)).toEqual([]);
});

test('a deleted folder and topic reuse place content, check their children separately and cannot be mutated', async ({ page }) => {
  await signIn(page); await page.goto(`/inspect/project/${TRACE.project}`); await expect(page.getByRole('heading', { name: 'Archived experiment' })).toBeVisible(); await expect(page.getByText('Kept topic', { exact: true })).toBeVisible(); await page.getByRole('link', { name: 'Look at it', exact: true }).click(); await expect(page).toHaveURL(`/inspect/topic/${TRACE.child}`); await expect(page.getByRole('heading', { name: 'Kept topic' })).toBeVisible(); await expect(page.getByRole('button', { name: 'Edit', exact: true })).toHaveCount(0);
  await page.goto(`/inspect/topic/${TRACE.topic}`); await expect(page.getByRole('heading', { name: 'Removed topic' })).toBeVisible();
});

test('purged content and revoked restoration authority show the safe refusal without old content', async ({ page }) => {
  await signIn(page); await page.goto(`/inspect/thread/${TRACE.purged}`); await expect(page.getByText('Could not look at this deleted thing')).toBeVisible(); await expect(page.locator('[data-stored-timeline]')).toHaveCount(0);
  await deny('ada', [`topic.manage@${ACME.fixLogin}`]); await page.goto(`/inspect/thread/${TRACE.thread}`); await expect(page.getByText('Could not look at this deleted thing')).toBeVisible(); await expect(page.getByText('Ask Ada Lovelace')).toBeVisible(); await expect(page.locator('[data-stored-timeline]')).toHaveCount(0);
});
