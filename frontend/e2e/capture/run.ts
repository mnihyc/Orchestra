/** One browser, one visible page at a time, no fixtures/interception/server/build and no unbounded waits. */
import type { Browser, BrowserContext, Page } from '@playwright/test';
import { mkdir } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import { join, relative } from 'node:path';
import { ACTORS, ACTION_MS, IDLE_MS, READY_MS, THEMES, VIEWPORTS, type Actor, type CaptureConfig, type CaptureLocale, type CaptureTheme } from './config.ts';
import { slug, writeReceipt, writeSheets, type CaptureRun, type ScreenReceipt } from './artifacts.ts';
import { captureManifest, type CaptureState } from './manifest.ts';
import { openState } from './open.ts';
import { ready } from './ready.ts';
import { PageRequests, requestReceipt, safeEndpoint } from './requests.ts';
import { appearance, CaptureSession } from './session.ts';

function newScreen(run: CaptureRun, state: CaptureState, actor: Actor, viewport: { width: number; height: number }, locale: CaptureLocale, theme: CaptureTheme): ScreenReceipt {
  return { number: run.screens.length + 1, state: state.id, label: state.label, pattern: state.pattern.startsWith('/') ? safeEndpoint(state.pattern) : state.pattern, route: safeEndpoint(state.url), landed: null,
    actor, as_admin: actor === 'ada-admin', observed_as_admin: null, viewport, theme, locale, kind: state.kind, note: state.note, ready: false, issues: [], screenshot: null, requests: requestReceipt([], [], 0) };
}

async function capture(browser: Browser, config: CaptureConfig, directory: string, row: ScreenReceipt, state: CaptureState, session?: CaptureSession): Promise<void> {
  let context: BrowserContext | undefined; let page: Page | undefined; let meter: PageRequests | undefined;
  let stage = 'browser_context'; let idleStart: number | undefined; let observed = 0;
  try {
    context = session === undefined ? await browser.newContext({ baseURL: config.baseURL, viewport: row.viewport, colorScheme: row.theme,
      locale: row.locale === 'en' ? 'en-US' : row.locale, timezoneId: 'UTC', deviceScaleFactor: 1 }) : await session.context(row.viewport, row.locale, row.theme);
    if (session === undefined) await appearance(context, row.locale, row.theme);
    page = await context.newPage(); page.setDefaultTimeout(ACTION_MS); page.setDefaultNavigationTimeout(READY_MS);
    session?.track(page);
    page.on('pageerror', () => { if (!row.issues.includes('javascript_error')) row.issues.push('javascript_error'); });
    meter = new PageRequests(page, config.baseURL);
    stage = 'navigation';
    await page.goto(state.url, { waitUntil: 'domcontentloaded' });
    stage = 'initial_content'; await ready(page, meter, row.locale, row.theme);
    if (session !== undefined && ['/login', '/setup'].includes(new URL(page.url()).pathname)) throw new Error('The signed-in route returned to sign-in.');
    if (session !== undefined) {
      stage = 'session_mode';
      const mode = page.locator('[data-screen-titlebar]:visible [role="switch"][data-as-admin]');
      const count = await mode.count();
      if (count > 1) throw new Error('The page shows more than one session mode.');
      if (count === 1) {
        const value = await mode.getAttribute('data-as-admin');
        if (value !== 'true' && value !== 'false') throw new Error('The page did not report its session mode.');
        row.observed_as_admin = value === 'true';
        if (row.observed_as_admin !== row.as_admin) throw new Error('The page changed the requested session mode.');
      }
    }
    stage = 'open_state';
    if (state.container !== undefined || state.view !== undefined || state.clicks !== undefined) meter.begin('opening');
    await openState(page, state, row.locale);
    stage = 'state_content'; await ready(page, meter, row.locale, row.theme);
    if (new URL(page.url()).origin !== config.baseURL) throw new Error('The state left the given world.');
    row.ready = true;
  } catch {
    // Playwright errors can include input values or one-use URLs. Persist the stage, never those messages.
    row.issues.push(`${stage}_failed`);
  }
  try {
    if (page !== undefined && !page.isClosed() && meter !== undefined) {
      // Even a failed opening gets its real 30-second idle record; it is labelled incomplete, not omitted.
      stage = 'idle'; meter.begin('idle'); idleStart = performance.now();
      let stopped: number | undefined;
      const stop = () => { stopped ??= performance.now(); };
      page.once('close', stop); page.once('crash', stop);
      try { await new Promise<void>((done) => setTimeout(done, IDLE_MS)); }
      finally { page.off('close', stop); page.off('crash', stop); }
      observed = (stopped ?? performance.now()) - idleStart; idleStart = undefined; meter.begin('after_idle');
      if (stopped !== undefined) { row.ready = false; row.issues.push('idle_window_interrupted'); }
      stage = 'final_content';
      if (row.ready) {
        try { await ready(page, meter, row.locale, row.theme); }
        catch { row.ready = false; row.issues.push('final_content_failed'); }
      }
      row.landed = safeEndpoint(page.url());
      stage = 'screenshot';
      const image = `screenshots/${String(row.number).padStart(5, '0')}-${slug(row.actor)}-${row.viewport.width}x${row.viewport.height}-${row.theme}-${row.locale}-${slug(state.id)}-${slug(state.label)}.png`;
      await page.screenshot({ path: join(directory, image), fullPage: false, animations: 'disabled', timeout: READY_MS });
      row.screenshot = image;
    }
  } catch { row.issues.push(`${stage}_failed`); }
  finally {
    if (idleStart !== undefined) observed = performance.now() - idleStart;
    if (meter !== undefined) { row.requests = meter.receipt(observed); meter.stop(); }
    if (context !== undefined) {
      try { if (session !== undefined) await session.keep(context); }
      catch { row.issues.push('session_snapshot_failed'); }
      finally {
        try { await context.close(); }
        catch { row.issues.push('browser_context_close_failed'); }
      }
    }
  }
}

export function screenCount(config: CaptureConfig): number {
  const states = captureManifest(config);
  return (ACTORS.length * states.filter((row) => row.signedIn).length + states.filter((row) => !row.signedIn).length) * VIEWPORTS.length * THEMES.length * config.locales.length;
}

/** The caller's test deadline is a sum of finite navigation/action/readiness/idle bounds, not timeout:0. */
export function runTimeout(config: CaptureConfig): number {
  const states = captureManifest(config);
  const mostClicks = Math.max(0, ...states.map((state) => state.clicks?.length ?? 0));
  const perScreen = IDLE_MS + READY_MS * 12 + ACTION_MS * (mostClicks + 12);
  const setupAndCleanup = READY_MS * (16 + config.locales.length * THEMES.length * 4);
  // Node timers use a signed 32-bit millisecond interval; a second locale must not overflow into a 1 ms deadline.
  return Math.min(screenCount(config) * perScreen + ACTORS.length * setupAndCleanup, 2_147_483_647);
}

export async function runCapture(browser: Browser, config: CaptureConfig): Promise<{ directory: string; run: CaptureRun }> {
  const started = new Date().toISOString();
  const directory = join(config.output, `${started.replace(/[:.]/g, '-')}-${randomUUID().slice(0, 8)}`);
  await mkdir(join(directory, 'screenshots'), { recursive: true, mode: 0o700 });
  const expected = screenCount(config);
  const run: CaptureRun = { format: 1, candidate: config.candidate, world: config.baseURL, started, finished: null, expected_screens: expected,
    minimum_idle_minutes: expected * IDLE_MS / 60_000, policy: { first_load_max: 8, idle_ms: 30000, idle_max: 0, network_quiet_ms: 500, workers: 1 },
    session_issues: [], artifact_issues: [], screens: [], sheets: [] };
  const states = captureManifest(config);
  await writeReceipt(directory, run);
  process.stdout.write(`Capture run: ${relative(process.cwd(), directory)}\n${expected} planned screens; a complete walk needs ${run.minimum_idle_minutes} minutes of idle observation, plus loading/captures.\n`);
  const walk = async (actor: Actor, session?: CaptureSession, unavailable = false): Promise<void> => {
    for (const locale of config.locales) for (const theme of THEMES) {
      let prepared = !unavailable;
      if (session !== undefined) {
        try { await session.prepare(locale, theme); }
        catch { prepared = false; run.session_issues.push(`${actor}/${locale}/${theme}: appearance_setup_failed`); }
      }
      for (const viewport of VIEWPORTS) for (const state of states.filter((row) => row.signedIn === (actor !== 'signed-out'))) {
        const row = newScreen(run, state, actor, viewport, locale, theme);
        if (prepared) await capture(browser, config, directory, row, state, session);
        else row.issues.push('sign_in_or_appearance_unavailable');
        run.screens.push(row);
        await writeReceipt(directory, run);
        // The integrator can review complete sixteen-screen sheets while the rest of a long walk continues.
        if (row.number % 16 === 0) await writeSheets(browser, directory, run, false);
        process.stdout.write(`${row.number}/${expected} ${actor} ${viewport.width}px ${theme} ${locale} ${state.id}: ${row.ready && row.issues.length === 0 ? 'captured' : 'INCOMPLETE'}; ${row.requests.first_load.count}/${row.requests.idle.count} requests\n`);
      }
    }
  };
  try {
    for (const actor of ACTORS) {
      let session: CaptureSession | undefined;
      try { session = await CaptureSession.signIn(browser, config, actor); }
      catch { run.session_issues.push(`${actor}: sign_in_failed`); }
      try { await walk(actor, session, session === undefined); }
      finally {
        if (session !== undefined) {
          try { await session.close(); }
          catch { run.session_issues.push(`${actor}: preference_restore_or_sign_out_failed`); }
        }
      }
    }
    await walk('signed-out');
  } finally {
    run.finished = new Date().toISOString();
    await writeReceipt(directory, run);
    // Keep all captured pages and failure placeholders, even if the walk was interrupted by a browser fault.
    await writeSheets(browser, directory, run);
    await writeReceipt(directory, run);
  }
  return { directory, run };
}
