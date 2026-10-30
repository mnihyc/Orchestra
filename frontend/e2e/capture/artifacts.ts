/** Review files are independent of Playwright's transient test-results directory. */
import type { Browser, BrowserContext, Page } from '@playwright/test';
import { mkdir, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import type { Actor, CaptureLocale, CaptureTheme } from './config.ts';
import type { RequestReceipt } from './requests.ts';
import { bounded } from './deadline.ts';

export interface ScreenReceipt {
  number: number;
  state: string;
  label: string;
  pattern: string;
  route: string;
  landed: string | null;
  actor: Actor;
  as_admin: boolean;
  observed_as_admin: boolean | null;
  viewport: { width: number; height: number };
  theme: CaptureTheme;
  locale: CaptureLocale;
  kind: string;
  note?: string;
  ready: boolean;
  issues: string[];
  screenshot: string | null;
  requests: RequestReceipt;
}
export interface CaptureRun {
  format: 1;
  candidate: string;
  world: string;
  started: string;
  finished: string | null;
  expected_screens: number;
  minimum_idle_minutes: number;
  policy: { first_load_max: 8; idle_ms: 30000; idle_max: 0; network_quiet_ms: 500; workers: 1 };
  session_issues: string[];
  artifact_issues: string[];
  screens: ScreenReceipt[];
  sheets: { width: number; height: number; theme: CaptureTheme; locale: CaptureLocale; html: string; pngs: string[] }[];
}
export const slug = (value: string): string => value.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 80) || 'screen';
const escapes: Readonly<Record<string, string>> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
export const escapeHTML = (value: string): string => value.replace(/[&<>"']/g, (part) => escapes[part] ?? part);

/** Sixteen screens are a deliberate 4x4 review page, never one unreadably tall PNG for a whole run. */
export function chunks<T>(rows: readonly T[]): T[][] {
  const pages: T[][] = [];
  for (let start = 0; start < rows.length; start += 16) pages.push(rows.slice(start, start + 16));
  return pages;
}

export function contactHTML(title: string, rows: readonly ScreenReceipt[], theme: CaptureTheme): string {
  // A phone retains its original 400px width; four columns + three 16px gutters + two 24px edges = 1696px.
  const pages = chunks(rows);
  const cards = (page: ScreenReceipt[]) => page.map((row) => {
    const complete = row.ready && row.screenshot !== null && row.issues.length === 0 && row.requests.budgets.idle_complete;
    const quiet = row.requests.budgets.first_load_ok && row.requests.budgets.idle_ok;
    const state = complete ? quiet ? 'Captured' : 'CAPTURED / REQUEST BUDGET FAILED' : `INCOMPLETE: ${row.issues.join('; ')}`;
    const budget = `${row.requests.first_load.count} load + ${row.requests.opening.count} opening / ${row.requests.idle.count} in ${Math.round(row.requests.idle.observed_ms / 1000)} s idle`;
    const image = row.screenshot === null ? '<div class="missing">No screenshot. See the failed stage in requests.json.</div>'
      : `<a href="../${escapeHTML(row.screenshot)}"><img loading="lazy" width="400" height="${row.viewport.width === 400 ? 860 : 250}" src="../${escapeHTML(row.screenshot)}" alt="${escapeHTML(row.label)}"></a>`;
    return `<figure class="${complete && quiet ? 'captured' : 'incomplete'}"><figcaption><strong>${row.number}. ${escapeHTML(row.actor)}${row.as_admin ? ' (As admin: On)' : row.actor === 'ada' ? ' (As admin: Off)' : ''}</strong><span>${escapeHTML(row.label)}</span><small>${escapeHTML(state)} · ${escapeHTML(budget)}</small></figcaption>${image}</figure>`;
  }).join('');
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self' file:; style-src 'unsafe-inline'"><title>${escapeHTML(title)}</title><style>
*{box-sizing:border-box}body{margin:0;font:14px/1.35 system-ui,sans-serif;background:${theme === 'dark' ? '#17191d' : '#f3f4f6'};color:${theme === 'dark' ? '#f4f5f6' : '#17191d'}}
.sheet{width:1696px;padding:24px;break-after:page}h1{font-size:20px;margin:0 0 16px}h1 small{font-size:14px;font-weight:400}main{display:grid;grid-template-columns:repeat(4,400px);gap:16px;align-items:start}figure{margin:0;min-width:0;border:2px solid ${theme === 'dark' ? '#7c8798' : '#526177'};border-radius:4px;overflow:hidden}.incomplete{border-color:${theme === 'dark' ? '#ff9b9b' : '#ae1826'}}figcaption{min-height:104px;padding:8px}figcaption>*{display:block;overflow-wrap:anywhere}small{font-size:12px}img{display:block;width:100%;height:auto}a{color:inherit}.missing{height:250px;padding:24px;background:${theme === 'dark' ? '#352125' : '#ffe7eb'}}
</style></head><body>${pages.map((page, index) => `<section class="sheet" id="sheet-${index + 1}"><h1>${escapeHTML(title)} <small>Sheet ${index + 1} · 4 columns · open a thumbnail for the original</small></h1><main>${cards(page)}</main></section>`).join('')}</body></html>`;
}

export async function writeReceipt(directory: string, run: CaptureRun): Promise<void> {
  // Checkpoint after every page. An interrupted long walk still leaves all completed measurements.
  const temporary = join(directory, 'requests.json.next');
  await writeFile(temporary, `${JSON.stringify(run, null, 2)}\n`, { mode: 0o600 });
  await rename(temporary, join(directory, 'requests.json'));
}

export async function writeSheets(browser: Browser, directory: string, run: CaptureRun, final = true): Promise<void> {
  await mkdir(join(directory, 'sheets'), { recursive: true });
  const groups = new Map<string, ScreenReceipt[]>();
  for (const row of run.screens) {
    const key = `${row.viewport.width}x${row.viewport.height}-${row.theme}-${row.locale}`;
    const group = groups.get(key) ?? []; group.push(row); groups.set(key, group);
  }
  let context: BrowserContext | undefined;
  try {
    context = await browser.newContext({ viewport: { width: 1696, height: 1100 }, deviceScaleFactor: 1 });
    context.setDefaultTimeout(30_000); context.setDefaultNavigationTimeout(30_000);
  } catch { run.artifact_issues.push('contact_sheet_browser_unavailable'); }
  try {
    for (const [key, rows] of groups) {
      const first = rows[0]; if (first === undefined) continue;
      const existing = run.sheets.find((group) => group.width === first.viewport.width && group.theme === first.theme && group.locale === first.locale);
      const count = final ? chunks(rows).length : Math.floor(rows.length / 16);
      if (count <= (existing?.pngs.length ?? 0)) continue;
      const html = `sheets/${key}.html`;
      await writeFile(join(directory, html), contactHTML(`${run.candidate} / ${key}`, rows, first.theme), { mode: 0o600 });
      const group: CaptureRun['sheets'][number] = existing ?? { width: first.viewport.width, height: first.viewport.height, theme: first.theme, locale: first.locale, html, pngs: [] };
      if (existing === undefined) run.sheets.push(group);
      if (context === undefined) { await writeReceipt(directory, run); continue; }
      let sheet: Page | undefined;
      try {
        sheet = await context.newPage();
        await sheet.goto(pathToFileURL(join(directory, html)).href, { waitUntil: 'domcontentloaded', timeout: 30_000 });
        for (let index = group.pngs.length; index < count; index += 1) {
          const section = sheet.locator(`#sheet-${index + 1}`);
          await section.scrollIntoViewIfNeeded();
          await bounded(section.locator('img').evaluateAll((images) => { for (const image of images) (image as HTMLImageElement).loading = 'eager'; }));
          await sheet.waitForFunction((id) => Array.from(document.querySelectorAll<HTMLImageElement>(`#${id} img`)).every((image) => image.complete && image.naturalWidth > 0), `sheet-${index + 1}`, { timeout: 30_000 });
          await sheet.waitForFunction(() => document.fonts.status === 'loaded', undefined, { timeout: 30_000 });
          const png = `sheets/${key}-${String(index + 1).padStart(3, '0')}.png`;
          await section.screenshot({ path: join(directory, png), animations: 'disabled', timeout: 30_000 });
          group.pngs.push(png);
        }
      } catch {
        run.artifact_issues.push(`${key}: contact_sheet_png_failed`);
      } finally {
        try { if (sheet !== undefined) await sheet.close(); }
        catch { run.artifact_issues.push(`${key}: contact_sheet_page_close_failed`); }
      }
      await writeReceipt(directory, run);
    }
  } finally {
    try { if (context !== undefined) await context.close(); }
    catch { run.artifact_issues.push('contact_sheet_context_close_failed'); }
  }
  const links = run.sheets.map((group) => `<li><a href="${escapeHTML(group.html)}">${group.width}×${group.height} ${escapeHTML(group.theme)} ${escapeHTML(group.locale)}</a> — ${group.pngs.map((png, index) => `<a href="${escapeHTML(png)}">PNG ${index + 1}</a>`).join(', ')}</li>`).join('');
  await writeFile(join(directory, 'index.html'), `<!doctype html><html lang="en"><meta charset="utf-8"><title>Orch capture review</title><h1>${escapeHTML(run.candidate)}</h1><p>${run.screens.length}/${run.expected_screens} screens. <a href="requests.json">Request counts and capture failures</a></p><ul>${links}</ul></html>`, { mode: 0o600 });
}

