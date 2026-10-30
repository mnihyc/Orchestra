/** Every joined route has keyboard, theme, narrow-screen and axe evidence on the actual candidate, without constructed scene events. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Page } from '@playwright/test';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expectUniqueGuideEntries } from '../accessibility/guide-entries';
import { expectOneScrollerPerArea } from '../accessibility/scrolling';

type State = 'ready' | 'empty' | 'denied' | 'busy' | 'offline' | 'failed' | 'unavailable';
interface Target { role: 'button' | 'link' | 'textbox' | 'combobox' | 'checkbox' | 'radio'; name: string }
interface RouteCase { pattern: string; url: string; state: State; heading: string; cue: string; storageState?: string;
  locale: 'en' | 'zh-CN'; actor: string; keyboardTargets: Target[]; note: string }
interface Walk { candidate: string; baseURL: string; routes: RouteCase[] }

const path = process.env.ORCH_ROUTE_WALK;
const walk = path === undefined ? undefined : JSON.parse(readFileSync(path, 'utf8')) as Walk;
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
