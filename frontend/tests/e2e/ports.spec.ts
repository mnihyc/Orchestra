/** Ports persona walks and current guards against the single component-double world; actual carrier proof is separate. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, id, KIND, RUNS_ON } from '../doubles/fixtures.ts';
import { deny, expectAccessibleInBothThemes, PEOPLE, scenario, signIn, threadHeld } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41810'}`;
const ENV = RUNS_ON.sandbox;
async function control(path: string, body: Record<string, unknown> = {}) {
  const reply = await fetch(`${DOUBLE}/__double/ports${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(reply.ok).toBe(true);
  return reply.status === 204 ? null : reply.json() as Promise<{ openings: { id: string; way: string; state: string; desired: boolean }[]; mutations: { act: string; body: Record<string, unknown> }[] }>;
}
async function open(page: Page, person: { login: string; password: string } = PEOPLE.ada) {
  await signIn(page, person);
  await page.goto(`/resources/environments/${ENV}?tab=ports`);
  await expect(page.getByTestId('ports-panel')).toBeVisible();
}
const card = (page: Page, port = 3000) => page.getByTestId(`port-${port}`);
async function more(page: Page, audience: 'Place' | 'Link' | 'Host') {
  await card(page).getByRole('button', { name: 'Let more in', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Let more in', exact: true });
  await dialog.getByRole('radio', { name: audience, exact: true }).click();
  return dialog;
}
const expiry = () => new Date(Date.now() + 3_600_000).toISOString().slice(0, 16);
test.beforeEach(async () => { await scenario(); await control('/set', { scene: 'full' }); });

test('a person sees a stable address before a program starts; two viewers see the same name and selection opens nothing', async ({ page, browser }) => {
  await control('/set', { scene: 'empty' }); await open(page);
  const address = await card(page).getByRole('heading').innerText();
  await expect(page.getByText('Nothing is listening yet', { exact: true }).first()).toBeVisible();
  await card(page).focus(); await page.keyboard.press('Enter'); await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  expect((await control('/state'))?.openings).toHaveLength(0);
  const second = await browser.newContext(); const other = await second.newPage();
  try { await open(other, PEOPLE.vera); expect(await card(other).getByRole('heading').innerText()).toBe(address); await expect(card(other).getByRole('button', { name: 'Let more in' })).toHaveCount(0); }
  finally { await second.close(); }
  await page.getByLabel('Port number', { exact: true }).fill('8080'); await page.getByRole('button', { name: 'Show port', exact: true }).click();
  await expect(card(page, 8080)).toBeVisible(); expect((await control('/state'))?.openings).toHaveLength(0);
});

test('Internal has no opening act; UDP offers the helper and HTTP is explained; a chosen Ports view survives returning', async ({ page }) => {
  await open(page);
  await expect(card(page, 9222).getByRole('button')).toHaveCount(0);
  await expect(card(page, 5353).getByText('UDP uses the helper or a host port; a browser carries HTTP only.')).toBeVisible();
  await card(page, 5353).getByRole('button', { name: 'Show helper command' }).click();
  await expect(page.getByRole('dialog').getByText(/--token-stdin/).first()).toBeVisible();
  await expect(page.getByRole('dialog').getByText(/actual local address only after it binds/)).toBeVisible();
  await page.keyboard.press('Escape');
  await page.getByRole('tab', { name: 'Overview', exact: true }).click(); await page.getByRole('tab', { name: 'Ports', exact: true }).click();
  await page.goto('/resources/environments'); await page.goto(`/resources/environments/${ENV}`); await expect(page.getByRole('tab', { name: 'Ports', exact: true })).toHaveAttribute('aria-selected', 'true');
});

test('I show a port to a colleague in the project with its current audience count and no other environment door', async ({ page }) => {
  await open(page); const dialog = await more(page, 'Place');
  await dialog.getByLabel('Find a place', { exact: true }).fill('Web');
  await dialog.getByRole('treeitem', { name: 'Web', exact: true }).click();
  await expect(dialog.getByText('7 people can enter. No other environment door is given.')).toBeVisible();
  await expect(dialog.getByText('Everyone who may view Web, including guests, can use this port after signing in.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Allow the place' }).click();
  await expect(card(page).getByTestId('opening-place')).toBeVisible();
  const state = await control('/state'); expect(state?.mutations[0]?.body.place_id).toBe(ACME.web); expect(state?.mutations[0]?.body).not.toHaveProperty('listener_identity');
  await page.keyboard.press('Escape'); await expect(card(page).getByTestId('opening-place').getByText(/For everyone who may view Web, including guests/)).toBeVisible();
});

test('I show a port outside by a required bounded link; its credential is only in the successful new dialog', async ({ page }) => {
  await open(page); const dialog = await more(page, 'Link');
  await expect(dialog.getByRole('radio', { name: 'Both', exact: true })).toBeChecked();
  await expect(dialog.getByRole('button', { name: 'Create link', exact: true })).toHaveAttribute('aria-disabled', 'true');
  await dialog.getByLabel('Expires at', { exact: true }).fill(expiry());
  await expect(dialog.getByText(/Anyone with this link/)).toBeVisible();
  await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Create link', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Copy link', exact: true })).toBeVisible();
  await expect(dialog.getByText(/orch_token=/)).toBeVisible();
  await page.keyboard.press('Escape'); await expect(page.getByRole('button', { name: 'Copy link', exact: true })).toHaveCount(0);
  await expect(page.getByText(/orch_token=/)).toHaveCount(0); await page.reload(); await expect(page.getByTestId('ports-panel')).toBeVisible();
  await expect(page.getByText(/orch_token=/)).toHaveCount(0);
  expect(await page.evaluate(() => Object.values(localStorage).some((value: unknown) => typeof value === 'string' && value.includes('orch_token=')))).toBe(false);
});

test('a lost link reply reconciles one opening; only an explicit new-link act creates another', async ({ page }) => {
  await control('/set', { scene: 'full', lost_link: true }); await open(page); const dialog = await more(page, 'Link');
  await dialog.getByLabel('Expires at', { exact: true }).fill(expiry()); await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Create link', exact: true }).click();
  await expect(dialog.getByText('The link exists, but its one-time secret was not received.')).toBeVisible();
  expect((await control('/state'))?.openings).toHaveLength(1);
  await dialog.getByRole('button', { name: 'Check existing opening' }).click();
  await expect(dialog.getByRole('button', { name: 'Make a new link', exact: true })).toBeVisible(); expect((await control('/state'))?.openings).toHaveLength(1);
  await expect(dialog.getByRole('button', { name: 'Copy link', exact: true })).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Make a new link', exact: true }).click();
  await dialog.getByRole('checkbox', { name: 'Close the old opening' }).check();
  await dialog.getByLabel('Expires at', { exact: true }).fill(expiry()); await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Create link', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Copy link', exact: true })).toBeVisible();
  expect((await control('/state'))?.openings.map((row) => row.state)).toEqual(['closed', 'open']);
});

test('a reviewed host candidate cannot be replaced; pending bind has no live Copy address; managers match both connection ends', async ({ page }) => {
  await control('/set', { scene: 'full', bind_pending: true }); await open(page); const dialog = await more(page, 'Host');
  await expect(dialog.getByRole('radio', { name: 'TCP and UDP', exact: true })).toBeChecked();
  await dialog.getByRole('button', { name: 'Review address', exact: true }).click();
  await expect(dialog.getByText('Anybody who reaches host.example:41000 reaches this. Orch does not ask them who they are.')).toBeVisible();
  await control('/race'); await dialog.getByRole('button', { name: 'Publish this address' }).click();
  await expect(dialog.getByText('The reviewed candidate changed or expired. Review again.')).toBeVisible();
  expect((await control('/state'))?.openings).toHaveLength(0);
  await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Publish this address' }).click();
  await page.keyboard.press('Escape'); const opening = card(page).getByTestId('opening-host');
  await expect(opening.getByText('Opening…', { exact: true })).toBeVisible(); await expect(opening.getByRole('button', { name: 'Copy address' })).toHaveCount(0);
  await control('/acknowledge'); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(opening.getByText('host.example:41000', { exact: true })).toBeVisible();
  await control('/connections'); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await opening.getByText(/Who is connected \(/).click();
  await expect(opening.getByText('Seen inside as 127.0.0.1:48122')).toBeVisible(); await expect(opening.getByText('203.0.113.9:51900',{exact:true})).toBeVisible();
  await opening.getByRole('button', { name: 'Close opening' }).click(); await page.getByRole('dialog').getByRole('button', { name: 'Close opening' }).click();
  expect((await control('/state'))?.openings[0]?.desired).toBe(false);
});

test('links and openings survive program replacement and environment stop; expiry and removal still end them', async ({ page }) => {
  await open(page); const dialog = await more(page, 'Link'); await dialog.getByLabel('Expires at', { exact: true }).fill(expiry()); await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Create link', exact: true }).click(); await expect(dialog.getByRole('button', { name: 'Copy link', exact: true })).toBeVisible(); await page.keyboard.press('Escape');
  const address = await card(page).getByRole('heading').innerText(); await control('/replace-listener'); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); expect((await control('/state'))?.openings[0]?.state).toBe('open');
  await control('/lifecycle', { outcome: 'stop' }); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(card(page).getByTestId('opening-link').getByText('Environment stopped', { exact: true })).toBeVisible(); expect((await control('/state'))?.openings[0]?.desired).toBe(true);
  await control('/lifecycle', { outcome: 'restart' }); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); expect(await card(page).getByRole('heading').innerText()).toBe(address);
  await control('/lifecycle', { outcome: 'expiry' }); await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await expect(card(page).getByTestId('opening-link').getByText('Closed', { exact: true })).toBeVisible();
  await control('/lifecycle', { outcome: 'remove' }); expect((await control('/state'))?.openings[0]?.desired).toBe(false);
});

test('no domain keeps a helper link; current permission loss clears the projection and opens nothing', async ({ page }) => {
  await control('/set', { scene: 'no_domain' }); await open(page); const dialog = await more(page, 'Link');
  await expect(dialog.getByRole('radio', { name: 'Helper', exact: true })).toBeChecked(); await expect(dialog.getByRole('radio', { name: 'Both', exact: true })).toBeDisabled();
  await dialog.getByLabel('Expires at', { exact: true }).fill(expiry()); await deny('ada', ['ports.manage']); await dialog.getByRole('button', { name: 'Review address', exact: true }).click();
  await expect(dialog.getByText('Ports access changed. Ask Ada Lovelace.')).toBeVisible(); expect((await control('/state'))?.openings).toHaveLength(0);
  await page.keyboard.press('Escape'); await deny('ada', ['ports.view']); await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByTestId('port-3000')).toHaveCount(0); await expect(page.getByText('Ports access changed. Ask Ada Lovelace.')).toBeVisible();
});

test('a phone decides an agent request with the link destination stated before Allow once', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); await control('/request'); await open(page);
  const request = page.getByTestId('port-request'); await request.scrollIntoViewIfNeeded();
  await expect(request.getByText('This link is given to the agent and is readable in this thread.')).toBeVisible();
  await request.getByRole('checkbox', { name: 'For me only' }).check(); await request.getByRole('button', { name: 'Allow port 3000 once', exact: true }).click();
  await expect(request.getByRole('button', { name: 'Copy link', exact: true })).toBeVisible(); await expect(request.getByText('The server delivered the result to you.')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('the host form reviews its three explicit proposed fields and affected allocations, without a firewall claim', async ({ page }) => {
  await control('/set', { scene: 'no_range' }); await signIn(page); await page.goto(`/resources/hosts/${id(KIND.host, 1)}`);
  const host = page.getByTestId('host-ports'); await expect(host.getByText('No declaration saved')).toBeVisible(); await host.getByRole('button', { name: 'Edit ports…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Host ports', exact: true }); await expect(dialog.getByLabel('Incoming address', { exact: true })).toHaveValue('host.example'); await expect(dialog.getByRole('radio', { name: 'All addresses', exact: true })).toBeChecked(); await expect(dialog.getByLabel('Port ranges', { exact: true })).toHaveValue('41000–41015');
  await dialog.getByRole('button', { name: 'Review changes' }).click(); await expect(dialog.getByText('This does not configure a firewall.')).toBeVisible(); await expect(dialog.getByText('Affected openings')).toBeVisible(); await dialog.getByRole('button', { name: 'Save declaration' }).click();
  await expect(host.getByText('Saved values are shown. The host’s acknowledgement has not been reported.')).toBeVisible(); expect((await control('/state'))?.openings).toHaveLength(0);
});

test('empty, offline, unsupported, refused and failed reads remain distinct, and the view is accessible in both themes', async ({ page }) => {
  await open(page); await expectAccessibleInBothThemes(page, '[data-testid="ports-panel"]');
  for (const [scene, wording] of [['offline','Old observation'],['unsupported','This platform cannot list scoped listeners'],['local_off','Ports are disabled on Ada’s machine. Ask its owner or use its local console.'],['port_limit','16 of 16 ports are open to others. Close an opening before creating another.']] as const) {
    await control('/set', { scene }); await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    if (scene === 'local_off' || scene === 'port_limit') {
      await expect(page.getByText(wording, { exact: true })).toBeAttached();
      const summary = page.getByText('Policies and limits', { exact: true });
      if (await summary.locator('..').getAttribute('open') === null) await summary.click();
    }
    await expect(page.getByText(wording, { exact: true })).toBeVisible();
  }
  await control('/set', { scene: 'failed' }); await page.reload(); await expect(page.getByText('The current port snapshot could not be read.')).toBeVisible(); await expect(page.getByTestId('port-3000')).toHaveCount(0);
});

test('Access gives Ports its sixth area and explains what its current Manage cell gives', async ({ page }) => {
  await signIn(page); await page.goto(`/resources/environments/${ENV}`);
  await page.getByRole('tab', { name: 'Access', exact: true }).click();
  await page.getByRole('button', { name: 'Permissions', exact: true }).click();
  const dialog = page.getByRole('dialog'); await dialog.getByRole('button', { name: 'Permissions', exact: true }).click();
  const grid = dialog.getByRole('table', { name: 'Permissions by area' });
  for (const name of ['Environment', 'Files', 'Terminal', 'Browsers', 'VNC', 'Ports']) await expect(grid.getByRole('rowheader', { name, exact: true })).toBeVisible();
  await expect(grid).toContainText('They let more people in, make a link or publish a host port where policy allows.');
  await expect(grid.getByRole('columnheader', { name: /Interact|Use/ })).toHaveCount(0);
});


test('a program repair on an unlisted target makes no opening and refuses a stale first write', async ({ page }) => {
  await open(page); await page.getByLabel('Port number', { exact: true }).fill('8080'); await page.getByRole('button', { name: 'Show port', exact: true }).click();
  await card(page, 8080).getByRole('button', { name: 'Edit program options', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit program options', exact: true }); await dialog.getByText('How the program is spoken to').click();
  await dialog.getByRole('radio', { name: 'HTTPS', exact: true }).click(); await dialog.getByRole('radio', { name: 'Custom host', exact: true }).click();
  await dialog.getByLabel('Expected Host', { exact: true }).fill('localhost:8080'); await dialog.getByRole('checkbox', { name: 'Send a PROXY header' }).check();
  await expect(dialog.getByText('The program must expect this on every TCP connection; give it a port of its own.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Save program options' }).click(); await expect(dialog).toHaveCount(0); await expect(card(page, 8080).getByText(/HTTPS · localhost:8080/)).toBeVisible(); expect((await control('/state'))?.openings).toHaveLength(0);
});

test('the place administrator sees stopped wider admissions and closes only the selected opening', async ({ page }) => {
  await open(page); const dialog = await more(page, 'Link'); await dialog.getByLabel('Expires at', { exact: true }).fill(expiry());
  await dialog.getByRole('button', { name: 'Review address', exact: true }).click(); await dialog.getByRole('button', { name: 'Create link', exact: true }).click(); await expect(dialog.getByRole('button', { name: 'Copy link', exact: true })).toBeVisible(); await page.keyboard.press('Escape');
  await control('/lifecycle', { outcome: 'stop' }); await page.goto('/org/acme'); const list = page.getByTestId('outside-ports'); await expect(list).toBeVisible();
  await expect(list.getByText('Environment stopped', { exact: true })).toBeVisible(); await expect(list.getByText(/orch_token=/)).toHaveCount(0);
  await list.getByRole('button', { name: 'Close opening' }).click(); await page.getByRole('dialog', { name: 'Close this opening?' }).getByRole('button', { name: 'Close opening' }).click();
  await expect(list.getByText('Nothing is open to outside here.')).toBeVisible(); expect((await control('/state'))?.openings[0]?.state).toBe('closed');
});

test('the ordinary Approval list and thread show the same port card and a late outcome creates no turn', async ({ page }) => {
  await control('/request'); await signIn(page); await page.goto('/approvals?kind=port'); const request = page.getByTestId('port-request');
  await expect(request).toBeVisible(); await expect(request.getByRole('button', { name: 'Grant to this environment', exact: true })).toBeVisible();
  await page.goto(`/thread/${ACME.repro}`); const inThread = page.getByTestId('port-request'); await expect(inThread).toBeVisible();
  const before = await threadHeld(ACME.repro);
  await inThread.getByRole('button', { name: 'Allow port 3000 once', exact: true }).click();
  const timeline = page.getByRole('log', { name: 'Conversation', exact: true });
  await expect(timeline.getByText(/Your request for port 3000/)).toBeVisible();
  await expect(timeline.getByText(/orch_token=/)).toBeVisible();
  await expect(timeline.getByText(/<orch/)).toHaveCount(0);
  const after = await threadHeld(ACME.repro);
  expect(after.turns).toHaveLength(before.turns.length);
  expect(after.inputs).toHaveLength(before.inputs.length);
});


test('a port’s own connections need no opening; managers match both peers and permission loss leaves only counts', async ({ page }) => {
  await control('/own-connections'); await open(page); const port = card(page);
  await port.getByText('Who is connected (2)', { exact: true }).click();
  await expect(port.getByText('Ada Lovelace · ada · 203.0.113.8:51900', { exact: true })).toBeVisible();
  await expect(port.getByText('Seen inside as 127.0.0.1:48123', { exact: true })).toBeVisible();
  await port.getByLabel('Inside peer port').fill('48122');
  await expect(port.getByText('Seen inside as 127.0.0.1:48123', { exact: true })).toHaveCount(0);
  expect((await control('/state'))?.openings).toHaveLength(0);
  await deny('ada', ['ports.manage']); await page.reload();
  await expect(card(page).getByText(/2 connections/)).toBeVisible();
  await expect(page.getByText(/203.0.113.8:51900|48122|Grace Hopper · grace/)).toHaveCount(0);
});

test('reported UDP loss names its actual reasons and never fabricates a successful zero count', async ({ page }) => {
  await control('/set', { scene: 'full', udp_dropped: { oversized: 2, full: 0, senders: 0, local_send: 1, link_lost: 0 } });
  await open(page);
  await expect(page.getByText('UDP datagrams dropped', { exact: true })).toBeVisible();
  await expect(page.getByText('Oversized: 2', { exact: true })).toBeVisible();
  await expect(page.getByText('Local send failed: 1', { exact: true })).toBeVisible();
  await expect(page.getByText(/Buffer full: 0/)).toHaveCount(0);
});

test('host read refusal and failure keep the resource hierarchy and an honest error state', async ({ page }) => {
  await signIn(page);
  for (const scene of ['host_refused', 'host_failed']) {
    await control('/set', { scene }); await page.goto(`/resources/hosts/${id(KIND.host, 1)}`);
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Hosts', exact: true }).first()).toBeVisible();
    await expect(page.getByText('Loading…', { exact: true })).toHaveCount(0);
    await expect(page.getByTestId('host-ports')).toHaveCount(0);
  }
});
