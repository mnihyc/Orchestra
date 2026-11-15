/** Every joined route has keyboard, theme, narrow-screen and axe evidence on the actual candidate, without constructed scene events. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectUniqueGuideEntries } from '../accessibility/guide-entries';
import { expectOneScrollerPerArea } from '../accessibility/scrolling';

type State = 'ready' | 'empty' | 'denied' | 'busy' | 'offline' | 'failed' | 'unavailable';
interface Target { role: 'button' | 'link' | 'textbox' | 'combobox' | 'checkbox' | 'radio' | 'menuitem' | 'tab' | 'switch' | 'option'; name: string; nth?: number }
interface ActionStep extends Target { act: 'click' | 'fill' | 'press' | 'check'; value?: string }
interface ActionCase {
  name: string;
  url?: string;
  steps: ActionStep[];
  cue: string;
  navigation: 'preserve' | 'document' | 'popup' | 'download';
  nativeFlow?: 'identity' | 'fresh_proof' | 'port' | 'public_address' | 'new_version';
  timeoutMs?: number;
}
interface RouteCase { pattern: string; url: string; state: State; heading: string; cue: string; storageState?: string;
  locale: 'en' | 'zh-CN'; actor: string; keyboardTargets: Target[]; actions: ActionCase[]; note: string }
interface Walk { candidate: string; baseURL: string; routes: RouteCase[] }

const path = process.env.ORCH_ROUTE_WALK;
const walk = path === undefined ? undefined : JSON.parse(readFileSync(path, 'utf8')) as Walk;
if (walk?.routes.some((row) => !Array.isArray(row.actions)) === true) throw new Error('The private native route walk needs explicit actions for every route');
const normalize = (route: string) => route.replace(/\/$/, '') || '/';

function patterns(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return patterns(path);
    if (!entry.name.endsWith('.tsx')) return [];
    const source = readFileSync(path, 'utf8');
    return [...source.matchAll(/createFileRoute\(['"]([^'"]+)['"]\)/g)].map((match) => normalize(match[1] ?? '/'));
  });
}

/** One traversal of the actual focusable elements bounds a keyboard search without a guessed retry threshold. */
async function reachedByTab(page: Page, target: Target): Promise<void> {
  const control = page.getByRole(target.role, { name: target.name, exact: true });
  await expect(control).toHaveCount(1);
  await expect(control).toBeVisible();
  const count = await page.locator('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])').count();
  for (let step = 0; step <= count; step += 1) {
    await page.keyboard.press('Tab');
    if (await control.evaluate((element) => element === document.activeElement)) return;
  }
  throw new Error(`The keyboard cannot reach ${target.role} ${target.name} in one page traversal.`);
}

/** Desktop and ordinary phone sizes expose both pane layout and wrapped/overflow controls. */
const VIEWPORTS = { wide: { width: 1280, height: 900 }, narrow: { width: 390, height: 844 } } as const;
const THEMES = ['light', 'dark'] as const;
const ESCAPES: Readonly<Record<string, string>> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const escape = (text: string) => text.replace(/[&<>"']/g, (letter) => ESCAPES[letter] ?? letter);

test.describe('joined candidate route acceptance', () => {
  test.skip(walk === undefined, 'A real candidate and private integration-runner states are required.');

  test('the manifest covers each current route and the required visible states in both locales', () => {
    expect(walk).toBeDefined();
    if (walk === undefined) return;
    expect(walk.candidate).toMatch(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/);
    const declared = new Set(patterns(fileURLToPath(new URL('../../src/routes', import.meta.url))));
    for (const locale of ['en', 'zh-CN']) {
      const supplied = new Set(walk.routes.filter((row) => row.locale === locale).map((row) => normalize(row.pattern)));
      expect([...declared].filter((route) => !supplied.has(route)), `Unwalked routes in ${locale}`).toEqual([]);
    }
    for (const state of ['ready', 'empty', 'denied', 'busy', 'offline', 'failed']) {
      expect(walk.routes.some((row) => row.state === state), `Missing ${state} observation`).toBe(true);
    }
    for (const row of walk.routes) {
      expect(row.keyboardTargets.length, row.pattern).toBeGreaterThan(0);
      expect(Array.isArray(row.actions) && row.actions.length > 0, `An action walk is required for ${row.pattern}`).toBe(true);
      for (const action of row.actions) {
        expect(action.steps.length, action.name).toBeGreaterThan(0);
        expect(action.cue, action.name).not.toBe('');
        if (action.navigation === 'document') expect(action.nativeFlow, action.name).toBeDefined();
      }
      expect(row.note, row.pattern).not.toBe('');
      expect(row.cue, row.pattern).not.toBe('');
    }
  });

  test('each real route is usable by keyboard and accessible in both themes and sizes', async ({ browser }) => {
    if (walk === undefined) return;
    const receipts: object[] = [];
    const pictures: { label: string; bytes: Buffer }[] = [];
    for (const row of walk.routes) {
      const context = await browser.newContext({ baseURL: walk.baseURL, storageState: row.storageState, locale: row.locale });
      try {
        const page = await context.newPage();
        const broken: string[] = [];
        const doubles: string[] = [];
        page.on('pageerror', (error) => broken.push(error.message));
        page.on('request', (request) => { if (new URL(request.url()).pathname.startsWith('/__double')) doubles.push('constructed-scene endpoint'); });
        for (const [size, viewport] of Object.entries(VIEWPORTS)) {
          await page.setViewportSize(viewport);
          for (const theme of THEMES) {
            await page.emulateMedia({ colorScheme: theme });
            await page.goto(row.url);
            await expect(page.getByRole('heading', { name: row.heading, exact: true }).first()).toBeVisible();
            await expect(page.getByText(row.cue, { exact: false }).first()).toBeVisible();
            await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
            await expect(page.locator('html')).toHaveAttribute('lang', row.locale);
            for (const target of row.keyboardTargets) await reachedByTab(page, target);
            await expectUniqueGuideEntries(page);
            await expectOneScrollerPerArea(page);
            const axe = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
            const grave = axe.violations.filter((row) => row.impact === 'serious' || row.impact === 'critical');
            expect(grave.map((row) => row.id), `${row.pattern} ${row.locale} ${theme} ${size}`).toEqual([]);
            pictures.push({ label: `${row.pattern} · ${row.state} · ${row.locale} · ${theme} · ${size}`, bytes: await page.screenshot() });
            receipts.push({ candidate: walk.candidate, pattern: row.pattern, state: row.state, locale: row.locale,
              theme, viewport: size, actor: row.actor, keyboard: true, axe: true, help_entries: true, note: row.note });
          }
        }
        expect(broken, row.pattern).toEqual([]);
        expect(doubles, row.pattern).toEqual([]);
      } finally { await context.close(); }
    }
    // One contact sheet preserves actual screenshots; no credential-bearing URL, state file or HTTP trace is included.
    const sheet = `<!doctype html><html lang="en"><meta charset="utf-8"><title>Candidate route contact sheet</title><style>body{font:14px sans-serif}main{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:1rem}img{width:100%}figure{margin:0}</style><h1>${escape(walk.candidate)}</h1><main>${pictures.map((picture) => `<figure><figcaption>${escape(picture.label)}</figcaption><img alt="${escape(picture.label)}" src="data:image/png;base64,${picture.bytes.toString('base64')}"></figure>`).join('')}</main></html>`;
    await test.info().attach('route-contact-sheet', { body: Buffer.from(sheet), contentType: 'text/html' });
    await test.info().attach('safe-route-receipts', { body: Buffer.from(JSON.stringify(receipts, null, 2)), contentType: 'application/json' });
  });
});


/** Stable DOM signatures cover every rendered action without recording hrefs or typed values. */
async function actionKeys(page: Page): Promise<string[]> {
  return page.locator('button,a[href],input[type="button"],input[type="submit"],[role="button"],[role="menuitem"],[role="tab"],[role="switch"]').evaluateAll((elements) => {
    const key = (element: Element) => {
      const labelled = element.getAttribute('aria-labelledby')?.split(/\s+/u).map((id) => document.getElementById(id)?.textContent ?? '').join(' ');
      const label = element.getAttribute('aria-label') ?? labelled ?? element.getAttribute('title') ?? element.textContent ?? '';
      return [element.getAttribute('role') ?? element.tagName.toLowerCase(), element.getAttribute('data-action') ?? '', label.replace(/\s+/gu, ' ').trim()].join(':');
    };
    return [...new Set(elements.filter((element) => {
      const box = element.getBoundingClientRect(), style = getComputedStyle(element);
      return box.width > 0 && box.height > 0 && style.visibility !== 'hidden' && style.display !== 'none'
        && element.closest('[aria-hidden="true"]') === null && !element.hasAttribute('disabled') && element.getAttribute('aria-disabled') !== 'true';
    }).map(key))];
  });
}
async function step(page: Page, target: ActionStep, timeout: number): Promise<string> {
  const all = page.getByRole(target.role, { name: target.name, exact: true });
  const control = target.nth === undefined ? all : all.nth(target.nth);
  if (target.nth === undefined) await expect(control).toHaveCount(1, { timeout });
  await expect(control).toBeVisible({ timeout });
  const key = await control.evaluate((element) => {
    const labelled = element.getAttribute('aria-labelledby')?.split(/\s+/u).map((id) => document.getElementById(id)?.textContent ?? '').join(' ');
    const label = element.getAttribute('aria-label') ?? labelled ?? element.getAttribute('title') ?? element.textContent ?? '';
    return [element.getAttribute('role') ?? element.tagName.toLowerCase(), element.getAttribute('data-action') ?? '', label.replace(/\s+/gu, ' ').trim()].join(':');
  });
  switch (target.act) {
    case 'click': await control.click({ timeout }); break;
    case 'fill': if (target.value === undefined) throw new Error('A fill step needs its fixture value'); await control.fill(target.value, { timeout }); break;
    case 'press': if (target.value === undefined) throw new Error('A key step needs its fixture key'); await control.press(target.value, { timeout }); break;
    case 'check': await control.check({ timeout }); break;
  }
  return key;
}

for (const row of walk?.routes ?? []) for (const [size, viewport] of Object.entries(VIEWPORTS)) for (const theme of THEMES) {
  test(`actions preserve their document: ${row.pattern} ${row.state} ${row.locale} ${theme} ${size}`, async ({ browser }) => {
    if (walk === undefined) return;
    const budget = row.actions.reduce((sum, action) => sum + (action.timeoutMs ?? test.info().project.timeout), 0);
    test.setTimeout(budget);
    const required = new Set<string>(), covered = new Set<string>();
    const receipts: { name: string; navigation: ActionCase['navigation']; documentRequests: number; popup: number; downloads: number }[] = [];
    for (const action of row.actions) {
      const allowance = action.timeoutMs ?? test.info().project.timeout;
      const context = await browser.newContext({ baseURL: walk.baseURL, storageState: row.storageState, locale: row.locale, viewport });
      try {
        const page = await context.newPage();
        await page.emulateMedia({ colorScheme: theme });
        await page.goto(action.url ?? row.url, { timeout: allowance });
        await expect(page.getByRole('heading', { name: row.heading, exact: true }).first()).toBeVisible({ timeout: allowance });
        for (const key of await actionKeys(page)) required.add(key);
        const token = await page.evaluate(() => {
          const value = crypto.randomUUID();
          Object.defineProperty(window, '__orchActionDocument', { configurable: true, value });
          return value;
        });
        let documentRequests = 0, popup = 0, downloads = 0;
        page.on('request', (request) => { if (request.isNavigationRequest() && request.resourceType() === 'document' && request.frame() === page.mainFrame()) documentRequests += 1; });
        page.on('popup', () => { popup += 1; });
        page.on('download', () => { downloads += 1; });
        for (const target of action.steps) {
          covered.add(await step(page, target, allowance));
          if (await page.getByRole('heading', { name: row.heading, exact: true }).first().isVisible()) {
            for (const key of await actionKeys(page)) required.add(key);
          }
        }
        await expect(page.getByText(action.cue, { exact: false }).first()).toBeVisible({ timeout: allowance });
        if (action.navigation !== 'document') {
          const retained = await page.evaluate((expected) => (window as Window & { __orchActionDocument?: string }).__orchActionDocument === expected, token);
          expect(retained, action.name).toBe(true);
          if (action.navigation !== 'download') expect(documentRequests, action.name).toBe(0);
        } else {
          expect(action.nativeFlow, action.name).toBeDefined();
          expect(documentRequests, action.name).toBeGreaterThan(0);
        }
        if (action.navigation === 'popup') await expect.poll(() => popup, { timeout: allowance, message: action.name }).toBeGreaterThan(0);
        if (action.navigation === 'download') await expect.poll(() => downloads, { timeout: allowance, message: action.name }).toBeGreaterThan(0);
        receipts.push({ name: action.name, navigation: action.navigation, documentRequests, popup, downloads });
      } finally { await context.close(); }
    }
    expect([...required].filter((key) => !covered.has(key)), 'Every visible enabled action, including controls revealed by a dialog or menu, needs a named act in the private walk').toEqual([]);
    // No address, storage state, request body, typed value or credential enters this receipt.
    await test.info().attach('document-preservation-actions', { body: Buffer.from(JSON.stringify({ candidate: walk.candidate, pattern: row.pattern, state: row.state, locale: row.locale, theme, size, receipts }, null, 2)), contentType: 'application/json' });
  });
}
