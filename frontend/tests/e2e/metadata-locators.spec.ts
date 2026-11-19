/** Native metadata workflows plus an explicitly constructed Git snapshot; successful authority/resource replies stay unchanged. */
import { expect, type Locator, type Page, type Request, type TestInfo, test } from '@playwright/test';
import type { Agent, GitStatus, HarnessKindInfo, Machine } from '../../src/generated/api-types.ts';
import words from '../../src/i18n/en.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures.ts';
import { asAdmin } from './admin-mode-support.ts';
import { begin, MAIN, workspace } from './inspector-support.ts';
import { scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

const layouts = [{ width: 1440, theme: 'light' }, { width: 1440, theme: 'dark' }, { width: 400, theme: 'light' }, { width: 400, theme: 'dark' }] as const;
type Layout = (typeof layouts)[number];
// Two existing 250 ms debounce intervals catch accidental delayed typing reads; this is not a latency budget.
const OBSERVE_MS = 500;
interface Call { method: string; path: string; query: Record<string, string> }
interface Body { path: string; method: string; body: unknown }
type ObservedWindow = typeof window & { __metadataWrites: Body[] };

const businessPath = (path: string) => path === '/api/v1/harness-kinds' || path === '/api/v1/drivers' || path === '/api/v1/notifiers'
  || path === '/api/v1/everything' || path === '/api/v1/provider-setup'
  || /^\/api\/v1\/(?:agents|admin\/agents|templates|machines|notification-channels|admin\/notification-channels|orgs\/[^/]+\/(?:agents|notification-channels))(?:\/|$)/u.test(path)
  || path.includes('/git/');
function observe(page: Page) {
  const calls: Call[] = [];
  const receive = (request: Request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/')) calls.push({ method: request.method(), path: url.pathname, query: Object.fromEntries(url.searchParams) });
  };
  page.on('request', receive);
  return { calls, relevant: () => calls.filter((call) => businessPath(call.path)), stop: () => page.off('request', receive) };
}
type Observation = ReturnType<typeof observe>;

async function bodyObserver(page: Page) {
  // Chromium's network observer can omit streamed request bodies. Observe a clone;
  // send the original Request/init unchanged and never record authorization headers.
  await page.addInitScript(() => {
    const observed = window as ObservedWindow;
    observed.__metadataWrites = [];
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      if (input instanceof Request && input.method === 'PATCH' && /^\/api\/v1\/agents\/[^/]+$/u.test(new URL(input.url).pathname)) {
        const entry = { method: input.method, path: new URL(input.url).pathname };
        void input.clone().json().then((body: unknown) => observed.__metadataWrites.push({ ...entry, body }), () => observed.__metadataWrites.push({ ...entry, body: 'Unreadable JSON' }));
      }
      return send(input, init);
    };
  });
}
async function bodies(page: Page): Promise<Body[]> { return await page.evaluate(() => (window as ObservedWindow).__metadataWrites); }

function answer(page: Page, path: string, method = 'GET', matches: (query: URLSearchParams) => boolean = () => true) {
  return page.waitForResponse((response) => {
    const url = new URL(response.url());
    return response.request().method() === method && url.pathname === path && matches(url.searchParams);
  });
}

async function settled(page: Page) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => {
      const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end);
    }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
}
async function layout(page: Page, mode: Layout) {
  await page.setViewportSize({ width: mode.width, height: 900 }); await page.emulateMedia({ colorScheme: mode.theme });
  await expect(page.locator('html')).toHaveAttribute('data-theme', mode.theme); await settled(page);
}
async function prepare(page: Page) {
  await page.setViewportSize({ width: 1440, height: 900 }); await page.emulateMedia({ colorScheme: 'light' });
  await signIn(page); await expect(page.locator('html')).toHaveAttribute('lang', 'en'); await layout(page, layouts[0]);
}

async function fitting(page: Page, input: Locator, list?: Locator) {
  const field = await input.evaluate((element) => {
    const rect = (node: Element) => { const box = node.getBoundingClientRect(); return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, height: box.height }; };
    const control = rect(element);
    const owner = rect(element.closest('[role="dialog"]') ?? element.closest('[data-view-instance]') ?? document.documentElement);
    const ids = (element.getAttribute('aria-labelledby') ?? '').split(/\s+/u).filter(Boolean);
    const visible = (node: Element) => node.closest('.sr-only,[hidden]') === null && getComputedStyle(node).visibility === 'visible' && node.getBoundingClientRect().width > 1;
    let labels = ids.flatMap((id) => { const label = document.getElementById(id); return label !== null && visible(label) ? [label] : []; });
    // Profile intentionally hides its field label; the shared setting row supplies the visible name.
    if (labels.length === 0) {
      const name = element.closest('[data-setting-row]')?.querySelector(':scope > div:first-child > div:first-child > span');
      if (name !== null && name !== undefined && visible(name)) labels = [name as HTMLElement];
    }
    return { control, owner, labels: labels.map((label) => {
      const range = document.createRange(); range.selectNodeContents(label);
      return { box: rect(label), text: label.textContent, textBoxes: Array.from(range.getClientRects(), (box) => ({ left: box.left, right: box.right })) };
    }) };
  });
  expect(field.control.width).toBeGreaterThan(0); expect(field.control.height).toBeGreaterThan(0);
  expect(field.control.left).toBeGreaterThanOrEqual(Math.max(0, field.owner.left) - 1);
  expect(field.control.right).toBeLessThanOrEqual(Math.min(page.viewportSize()?.width ?? 0, field.owner.right) + 1);
  expect(field.labels.length).toBeGreaterThan(0);
  for (const label of field.labels) {
    expect(label.box.left).toBeGreaterThanOrEqual(field.owner.left - 1); expect(label.box.right).toBeLessThanOrEqual(field.owner.right + 1);
    for (const text of label.textBoxes) { expect(text.left).toBeGreaterThanOrEqual(label.box.left - 1); expect(text.right).toBeLessThanOrEqual(label.box.right + 1); }
    expect(label.box.bottom <= field.control.top + 1 || label.box.top >= field.control.bottom - 1 || label.box.right <= field.control.left + 1 || label.box.left >= field.control.right - 1).toBe(true);
  }
  const popup = list === undefined ? null : await list.evaluate((element) => {
    // The list may extend inside the scrolling popup. Measure its visible frame,
    // not the unbounded list content (especially the thirteen-remote snapshot).
    const frame = element.closest('[data-radix-popper-content-wrapper]')?.firstElementChild;
    if (frame === null || frame === undefined) throw new Error('The native list has no popup frame');
    const rect = frame.getBoundingClientRect();
    return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, clientWidth: frame.clientWidth, scrollWidth: frame.scrollWidth };
  });
  if (popup !== null) {
    expect(popup.left).toBeGreaterThanOrEqual(-1); expect(popup.right).toBeLessThanOrEqual((page.viewportSize()?.width ?? 0) + 1);
    expect(popup.top).toBeGreaterThanOrEqual(-1); expect(popup.bottom).toBeLessThanOrEqual((page.viewportSize()?.height ?? 0) + 1);
    expect(popup.scrollWidth).toBeLessThanOrEqual(popup.clientWidth + 1);
  }
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  return { field, popup };
}

async function capture(page: Page, info: TestInfo, stem: string, input: Locator, list?: Locator) {
  await page.mouse.move(0, 0); await settled(page);
  const geometry = await fitting(page, input, list), path = info.outputPath(`${stem}.png`);
  await page.screenshot({ path, fullPage: false, caret: 'hide' });
  await info.attach(stem, { path, contentType: 'image/png' });
  await info.attach(`${stem}-geometry`, { body: JSON.stringify(geometry, null, 2), contentType: 'application/json' });
}

async function closeSearch(input: Locator) {
  await expect(input).toBeVisible();
  // Dialog autofocus may open the first locator. Read the committed value only
  // after closing that list; an open locator deliberately displays its query.
  if (await input.getAttribute('aria-expanded') === 'true') await input.press('Escape');
  await expect(input).toHaveAttribute('aria-expanded', 'false');
}

async function scrollPositions(input: Locator) {
  return input.evaluate((element) => {
    const ancestors: { left: number; top: number }[] = [];
    for (let parent = element.parentElement; parent !== null; parent = parent.parentElement) {
      ancestors.push({ left: parent.scrollLeft, top: parent.scrollTop });
    }
    return { window: { left: scrollX, top: scrollY }, ancestors };
  });
}

/** The real current value survives local query/Escape at both requested sizes; no caller receives the query. */
async function inspect(page: Page, info: TestInfo, observed: Observation, input: Locator, name: string, target: { query: string; label: string }, count?: number) {
  await closeSearch(input); const current = await input.inputValue(); expect(current).not.toBe('');
  const label = await input.getAttribute('aria-labelledby'); expect(label).toBeTruthy();
  for (const mode of layouts) {
    await layout(page, mode); await input.scrollIntoViewIfNeeded();
    const beforeOpenScroll = await scrollPositions(input);
    await input.click();
    const listId = await input.getAttribute('aria-controls');
    if (listId === null) throw new Error('The native locator did not name its popup');
    const list = page.locator(`[id="${listId}"]`); await expect(list).toHaveAttribute('role', 'listbox'); await expect(list).toBeVisible();
    if (count !== undefined) await expect(list.getByRole('option')).toHaveCount(count);
    const selected = list.getByRole('option', { name: current, exact: true });
    await expect(selected).toHaveCount(1); await expect(selected).toHaveAttribute('aria-selected', 'true');
    // ARIA selection alone can be offscreen after Portal/Popper finishes measuring.
    await expect(selected).toBeInViewport({ ratio: 1 });
    expect(await scrollPositions(input)).toEqual(beforeOpenScroll);
    await capture(page, info, `${name}-${mode.width}-${mode.theme}`, input, list);
    await page.waitForTimeout(OBSERVE_MS); const before = observed.relevant(), address = page.url();
    await input.fill(target.query); await expect(list.getByRole('option', { name: target.label, exact: true })).toBeVisible();
    await page.waitForTimeout(OBSERVE_MS); await fitting(page, input, list);
    expect(observed.relevant()).toEqual(before); expect(page.url()).toBe(address);
    await input.press('Escape'); await expect(input).toHaveValue(current);
  }
}
async function choose(page: Page, input: Locator, target: { query: string; label: string }) {
  await input.click(); await input.fill(target.query);
  const option = page.getByRole('option', { name: target.label, exact: true }); await expect(option).toBeVisible();
  // Descriptions also match (e.g. every driver says how many hosts report it).
  // Use Enter for a unique hit, otherwise choose the exact named option.
  const listId = await input.getAttribute('aria-controls');
  if (listId === null) throw new Error('The locator has no open popup');
  if (await page.locator(`[id="${listId}"]`).getByRole('option').count() === 1) await input.press('Enter'); else await option.click();
  await expect(input).toHaveValue(target.label);
}
const writes = (observed: Observation) => observed.relevant().filter((call) => call.method !== 'GET');

test('native Add Agent searches its complete machine and adapter choices without losing the creation draft', async ({ page }, info) => {
  await scenario(); await prepare(page); const observed = observe(page);
  try {
    const registryRead = answer(page, '/api/v1/harness-kinds'); await page.goto('/resources/agents');
    const registryReply = await registryRead; expect(registryReply.status()).toBe(200);
    const registry = await registryReply.json() as HarnessKindInfo[];
    const codex = registry.find((entry) => entry.kind === 'codex');
    if (codex === undefined) throw new Error('The native registry needs its Codex adapter');
    await page.locator('[data-resource-kind="agents"]').getByRole('button', { name: words.agents.list.add, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.agents.add.title, exact: true });
    const name = dialog.getByRole('textbox', { name: words.agents.add.name, exact: true }); await name.fill('Retained profile draft');
    let program = dialog.getByRole('combobox', { name: words.agents.add.harness, exact: true });
    await choose(page, program, { query: codex.kind, label: codex.title });
    await inspect(page, info, observed, program, 'create-agent-available', { query: codex.kind, label: codex.title }, registry.length);
    expect(writes(observed)).toEqual([]);

    await layout(page, layouts[0]); const machinesRead = answer(page, '/api/v1/machines');
    await dialog.getByRole('combobox', { name: words.agentCreation.mode, exact: true }).click();
    await page.getByRole('option', { name: words.agentCreation.onMachine, exact: true }).click();
    const machineReply = await machinesRead; expect(machineReply.status()).toBe(200);
    const machines = await machineReply.json() as Machine[], selected = machines.find((entry) => entry.id === RUNS_ON.laptop);
    if (selected === undefined || !selected.online || !selected.roles.includes('agent')) throw new Error('The native fixture needs its online agent machine');
    const machine = dialog.getByRole('combobox', { name: words.agentCreation.machine, exact: true });
    // machinesQuery already supplies complete authorized rows. machineQuery can
    // reuse that fresh row without another request when this field selects it.
    await choose(page, machine, { query: selected.name, label: selected.name });
    await inspect(page, info, observed, machine, 'create-agent-machine', { query: selected.name, label: selected.name }, machines.filter((entry) => entry.roles.includes('agent')).length);
    program = dialog.getByRole('combobox', { name: words.agents.add.harness, exact: true });
    await choose(page, program, { query: codex.kind, label: codex.title });
    await inspect(page, info, observed, program, 'create-agent-reported', { query: codex.kind, label: codex.title });
    expect(observed.calls.filter((call) => call.path === `/api/v1/machines/${selected.id}`)).toHaveLength(0);
    await expect(name).toHaveValue('Retained profile draft'); expect(writes(observed)).toEqual([]);
    await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click(); await expect(dialog).toHaveCount(0);
  } finally { observed.stop(); await info.attach('native-create-agent-calls', { body: JSON.stringify(observed.calls, null, 2), contentType: 'application/json' }); }
});

test('native agent metadata keeps pending/error identity, filters by raw kind and saves both build workflows explicitly', async ({ page }, info) => {
  await scenario(); await bodyObserver(page); await prepare(page);
  const observed = observe(page), written: Body[] = [];
  let release: () => void = () => undefined, arrived: () => void = () => undefined, first = true;
  const gate = new Promise<void>((resolve) => { release = resolve; }), waiting = new Promise<void>((resolve) => { arrived = resolve; });
  await page.route('**/api/v1/harness-kinds', async (route) => {
    if (!first) { await route.fallback(); return; }
    first = false; const native = await route.fetch(); expect(native.status()).toBe(200); arrived(); await gate;
    await route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', status: 500,
      title: 'Private registry diagnostic', detail: 'Private registry diagnostic', request_id: 'metadata-browser' } });
  });
  try {
    await page.goto('/resources/agents?harness=codex'); await waiting;
    await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
    const panel = page.locator('[data-resource-filter-panel]'), filter = panel.getByRole('combobox', { name: words.agents.list.harnessFilter, exact: true });
    await closeSearch(filter);
    await expect(filter).toHaveValue('Codex'); await expect(panel.getByText(words.agents.overview.notReported, { exact: true })).toHaveCount(0);
    await capture(page, info, 'agent-filter-pending-1440-light', filter);
    release(); await expect(panel.getByRole('alert')).toBeVisible();
    await layout(page, layouts[1]); await expect(filter).toHaveValue('Codex');
    await expect(panel).not.toContainText('Private registry diagnostic'); await expect(panel.getByText(words.agents.overview.notReported, { exact: true })).toHaveCount(0);
    await capture(page, info, 'agent-filter-error-400-dark', filter);
    expect(observed.calls.filter((call) => call.path === '/api/v1/harness-kinds')).toHaveLength(1);
    const retry = answer(page, '/api/v1/harness-kinds'); await panel.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    const replied = await retry; expect(replied.status()).toBe(200); const catalogue = await replied.json() as HarnessKindInfo[];
    expect(observed.calls.filter((call) => call.path === '/api/v1/harness-kinds')).toHaveLength(2);
    const other = catalogue.find((kind) => kind.kind !== 'codex');
    if (other === undefined) throw new Error('The real registry has no second adapter for this workflow');
    await inspect(page, info, observed, filter, 'agent-filter', { query: other.kind, label: other.title }, catalogue.length + 1);
    const filtered = answer(page, '/api/v1/everything', 'GET', (q) => q.get('harness') === other.kind);
    await choose(page, filter, { query: other.kind, label: other.title }); expect((await filtered).status()).toBe(200);
    expect(new URL(page.url()).searchParams.get('harness')).toBe(other.kind);
    await filter.click(); const all = answer(page, '/api/v1/everything', 'GET', (q) => !q.has('harness'));
    await page.getByRole('option', { name: words.choiceCatalog.statusFilter.any.name, exact: true }).click(); expect((await all).status()).toBe(200);
    expect(new URL(page.url()).searchParams.has('harness')).toBe(false); expect(writes(observed)).toEqual([]);

    await layout(page, layouts[0]);
    const agentPath = `/api/v1/agents/${RUNS_ON.codexOnLaptop}`, loaded = answer(page, agentPath);
    await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}`); const response = await loaded; expect(response.status()).toBe(200); const original = await response.json() as Agent;
    expect(original.access.permissions).toContain('agent.manage');
    const profile = typeof original.profile === 'object' && original.profile !== null && !Array.isArray(original.profile) ? original.profile as Record<string, unknown> : {};
    const available = catalogue.find((kind) => kind.kind === original.harness_kind)?.builds ?? [];
    if (typeof profile.harness_version !== 'string') throw new Error('The native owned profile needs its existing concrete pin');
    expect(available.some((build) => build.version === profile.harness_version), 'the existing fixture pin is retained despite absence from the successful registry').toBe(false);
    const version = available.find((build) => build.version !== profile.harness_version)?.version;
    if (version === undefined) throw new Error('The real registry needs a build distinct from this fixture’s retained pin');
    await page.locator('[data-main-area]').getByRole('button', { name: words.resources.act.editButton, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.resources.edit.title.replace('{{name}}', original.name), exact: true });
    const pin = dialog.getByRole('combobox', { name: words.agents.profile.pin, exact: true });
    await closeSearch(pin); await expect(pin).toHaveValue(profile.harness_version);
    await inspect(page, info, observed, pin, 'agent-edit-build', { query: version, label: version });
    await choose(page, pin, { query: version, label: version }); expect(writes(observed)).toEqual([]);
    const saved = answer(page, agentPath, 'PATCH'); await dialog.getByRole('button', { name: words.resources.edit.save, exact: true }).click();
    const saveResponse = await saved; expect(saveResponse.status()).toBe(200); const edited = await saveResponse.json() as Agent;
    await expect.poll(async () => (await bodies(page)).length).toBe(1);
    expect(await bodies(page)).toEqual([{ path: agentPath, method: 'PATCH', body: { expected_revision: original.revision, profile: { ...profile, harness_version: version } } }]);
    written.push(...await bodies(page)); await expect(dialog).toHaveCount(0);

    const profileRead = answer(page, agentPath); await page.goto(`/resources/agents/${original.id}?tab=profile`); expect((await profileRead).status()).toBe(200);
    const settings = page.locator('#agent-profile'), profilePin = settings.getByRole('combobox', { name: words.agents.profile.pin, exact: true });
    await expect(profilePin).toHaveValue(version);
    const fallback = { query: words.choiceCatalog.sourceKind.default.name, label: words.choiceCatalog.sourceKind.default.name };
    await inspect(page, info, observed, profilePin, 'agent-profile-build', fallback);
    // A default build's description may also match “Default”, so choose the specifically named sentinel.
    await profilePin.click(); await profilePin.fill(fallback.query); await page.getByRole('option', { name: fallback.label, exact: true }).click();
    expect(writes(observed)).toHaveLength(1);
    const profileSaved = answer(page, agentPath, 'PATCH'); await settings.getByRole('button', { name: words.agents.profile.save, exact: true }).click();
    expect((await profileSaved).status()).toBe(200); await expect.poll(async () => (await bodies(page)).length).toBe(1);
    expect(await bodies(page)).toEqual([{ path: agentPath, method: 'PATCH', body: { expected_revision: edited.revision, profile: { ...profile, harness_version: null } } }]);
    written.push(...await bodies(page)); expect(writes(observed)).toHaveLength(2);
  } finally { release(); observed.stop(); await info.attach('native-agent-calls', { body: JSON.stringify({ calls: observed.calls, writes: written }, null, 2), contentType: 'application/json' }); }
});

test('native system, machine, template and notifier forms retain local choice drafts and fit before explicit submission', async ({ page }, info) => {
  await scenario(); await prepare(page); const observed = observe(page);
  try {
    await asAdmin(page, true);
    const kinds = answer(page, '/api/v1/harness-kinds'); await page.goto('/admin/agents'); const reply = await kinds; expect(reply.status()).toBe(200); const catalogue = await reply.json() as HarnessKindInfo[];
    const target = catalogue.find((kind) => kind.kind === 'acme-agent'); if (target === undefined) throw new Error('The native Acme adapter is missing');
    await page.getByRole('button', { name: words.admin.agents.newTitle, exact: true }).click();
    let dialog = page.getByRole('dialog', { name: words.admin.agents.newTitle, exact: true });
    await dialog.getByRole('textbox', { name: words.admin.agents.name, exact: true }).fill('Metadata system draft');
    let input = dialog.getByRole('combobox', { name: words.admin.agents.adapter, exact: true });
    await expect(input).toHaveAttribute('placeholder', words.ui.combobox.search);
    await inspect(page, info, observed, input, 'system-agent-adapter', { query: target.kind, label: target.title }, catalogue.length);
    await choose(page, input, { query: target.kind, label: target.title }); expect(writes(observed)).toEqual([]);
    await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click(); await asAdmin(page, false);

    await layout(page, layouts[0]); const machineRead = answer(page, `/api/v1/machines/${RUNS_ON.laptop}`);
    await page.goto(`/resources/machines/${RUNS_ON.laptop}?tab=registered`); const machineReply = await machineRead; expect(machineReply.status()).toBe(200); const machine = await machineReply.json() as Machine;
    const facts = typeof machine.facts === 'object' && machine.facts !== null ? machine.facts as Record<string, unknown> : {};
    const found = Array.isArray(facts.harnesses) ? facts.harnesses.find((entry: unknown) => typeof entry === 'object' && entry !== null && 'kind' in entry && entry.kind === 'claude-code') as { kind: string; version: string; path: string } | undefined : undefined;
    if (found === undefined) throw new Error('The actual machine report has no Claude Code registration choice');
    await page.getByRole('button', { name: words.machines.registered.registerAgent, exact: true }).click();
    dialog = page.getByRole('dialog', { name: words.machines.agent.title.replace('{{machine}}', machine.name), exact: true });
    input = dialog.getByRole('combobox', { name: words.machines.agent.harness, exact: true });
    await expect(input).toHaveAttribute('placeholder', words.ui.combobox.search);
    const machineLabel = words.machines.agent.harnessOption.replace('{{harness}}', catalogue.find((kind) => kind.kind === found.kind)?.title ?? found.kind).replace('{{version}}', found.version);
    // Its registry title can arrive after the machine facts; wait for that real
    // title before taking the committed label as this workflow's baseline.
    await input.click(); await expect(page.getByRole('option', { name: machineLabel, exact: true })).toBeVisible();
    await inspect(page, info, observed, input, 'machine-reported-harness', { query: found.path, label: machineLabel });
    await choose(page, input, { query: found.path, label: machineLabel }); expect(writes(observed)).toEqual([]);
    await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();

    await layout(page, layouts[0]); await page.goto('/resources/templates');
    await page.getByRole('button', { name: words.templates.list.add, exact: true }).click();
    dialog = page.getByRole('dialog', { name: words.templates.new.title, exact: true });
    await dialog.getByRole('textbox', { name: words.templates.new.name, exact: true }).fill('Metadata template draft');
    input = dialog.getByRole('combobox', { name: words.templates.new.driver, exact: true });
    const host = { query: 'host', label: words.templates.driverChoices.host.name };
    await inspect(page, info, observed, input, 'template-native-driver', host);
    await choose(page, input, host); await expect(dialog.getByRole('textbox', { name: 'Run as user', exact: true })).toHaveValue('agent');
    await expect(dialog.getByRole('textbox', { name: words.templates.new.name, exact: true })).toHaveValue('Metadata template draft');
    expect(writes(observed)).toEqual([]); await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();

    await layout(page, layouts[0]); await page.goto('/org/acme/settings/channels');
    await page.getByRole('button', { name: words.settings.channels.add, exact: true }).click();
    dialog = page.getByRole('dialog', { name: words.settings.channels.addTitle.replace('{{name}}', 'Acme'), exact: true });
    input = dialog.getByRole('combobox', { name: words.settings.channels.kind, exact: true });
    await dialog.getByRole('textbox', { name: words.settings.channels.name, exact: true }).fill('Metadata delivery draft');
    await inspect(page, info, observed, input, 'notifier-native-kind', { query: 'webhook', label: 'Webhook' });
    await choose(page, input, { query: 'webhook', label: 'Webhook' });
    await expect(dialog.getByRole('textbox', { name: 'Address', exact: true })).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: words.settings.channels.name, exact: true })).toHaveValue('Metadata delivery draft');
    expect(writes(observed)).toEqual([]); await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
  } finally { observed.stop(); await info.attach('native-form-preview-calls', { body: JSON.stringify(observed.calls, null, 2), contentType: 'application/json' }); }
});

test('declared Git snapshot stress keeps a late current remote and searches thirteen names/URLs without starting a push', async ({ page }, info) => {
  await begin();
  const status: GitStatus = { available: true, reason: null, repository_dir: 'web', branch: 'main', head: 'a'.repeat(40), status_hash: 'b'.repeat(64), ahead: 1, behind: 0,
    upstream: 'review-remote-13/main', changes: [], remotes: Array.from({ length: 13 }, (_, index) => ({ name: `review-remote-${index + 1}`,
      url: `https://git.example.test/team/long-project-workflow-repository-${index + 1}.git` })) };
  // Existing native-shaped snapshot control. This is declared stress data, not discovery of a real repository's remotes.
  await workspace('/git', { environment: RUNS_ON.workspace, status }); await prepare(page);
  const observed = observe(page);
  try {
    await page.goto(`/thread/${MAIN}`); await showWorkbenchView(page, 'Git');
    const panel = page.getByTestId('git-panel'); await expect(panel.getByTestId('git-branch')).toHaveText('main');
    await panel.getByRole('button', { name: words.source.git.push, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.source.git.pushTitle, exact: true }), input = dialog.getByRole('combobox', { name: words.source.git.remote, exact: true });
    await closeSearch(input);
    await expect(input).toHaveValue('review-remote-13');
    const target = { query: 'repository-12.git', label: 'review-remote-12' };
    await inspect(page, info, observed, input, 'declared-git-remotes', target, 13);
    await input.click();
    const listId = await input.getAttribute('aria-controls');
    if (listId === null) throw new Error('The remote locator must name its popup');
    const list = page.locator(`[id="${listId}"]`), beforeKeysScroll = await scrollPositions(input);
    for (const [key, name] of [['Home', 'review-remote-1'], ['End', 'review-remote-13'], ['ArrowUp', 'review-remote-12']] as const) {
      await input.press(key);
      const active = list.getByRole('option', { name, exact: true }), activeId = await active.getAttribute('id');
      if (activeId === null) throw new Error('The active remote must have its keyboard identity');
      await expect(input).toHaveAttribute('aria-activedescendant', activeId);
      await expect(active).toHaveAttribute('aria-selected', 'true');
      await expect(active).toBeInViewport({ ratio: 1 });
      await expect(input).toBeFocused();
      expect(await scrollPositions(input)).toEqual(beforeKeysScroll);
    }
    await input.press('Escape'); await expect(input).toHaveValue('review-remote-13');
    await choose(page, input, target); await expect(dialog.getByRole('textbox', { name: words.source.git.branch, exact: true })).toHaveValue('main');
    await expect(dialog.getByRole('checkbox', { name: words.source.git.setUpstream, exact: true })).not.toBeChecked();
    await page.waitForTimeout(OBSERVE_MS); expect(writes(observed)).toEqual([]);
    await page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0);
  } finally { observed.stop(); await info.attach('declared-git-snapshot-calls', { body: JSON.stringify({ snapshot: status, calls: observed.calls }, null, 2), contentType: 'application/json' }); }
});
