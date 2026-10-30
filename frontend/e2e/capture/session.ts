/** Disposable browser sessions; only the two appearance fields we changed are restored on the server. */
import { expect, type Browser, type BrowserContext, type Page, type Response } from '@playwright/test';
import { ACTION_MS, READY_MS, type Actor, type CaptureConfig, type CaptureLocale, type CaptureTheme } from './config.ts';
import { bounded } from './deadline.ts';

type Storage = Awaited<ReturnType<BrowserContext['storageState']>>;
export function freshStorage(state: Storage): Storage {
  return { ...state, origins: state.origins.map((origin) => ({ ...origin,
    localStorage: origin.localStorage.filter((entry) => !/^orch\..*\.(workbench|choices)\./.test(entry.name)) })) };
}
interface Auth { token: string; theme: 'light' | 'dark' | 'system'; locale: CaptureLocale; admin: boolean }
function auth(value: unknown): Auth {
  if (typeof value !== 'object' || value === null || !('access_token' in value) || typeof value.access_token !== 'string' || !('user' in value)) throw new Error('The sign-in returned no usable session.');
  const user = value.user;
  if (typeof user !== 'object' || user === null || !('theme' in user) || !('locale' in user)
    || (user.theme !== 'light' && user.theme !== 'dark' && user.theme !== 'system') || (user.locale !== 'en' && user.locale !== 'zh-CN')) throw new Error('The sign-in returned no appearance preferences.');
  return { token: value.access_token, theme: user.theme, locale: user.locale, admin: 'is_system_admin' in user && user.is_system_admin === true };
}

export async function appearance(context: BrowserContext, locale: CaptureLocale, theme: CaptureTheme): Promise<void> {
  await context.addInitScript(({ locale, theme }) => {
    localStorage.setItem('orch.appearance', JSON.stringify({ locale, theme, density: 'comfortable' }));
  }, { locale, theme });
}

export class CaptureSession {
  private state: Storage;
  private current: Auth;
  private readonly original: Pick<Auth, 'theme' | 'locale'>;
  private lastChanged: Pick<Auth, 'theme' | 'locale'> | undefined;
  private updates: Promise<void> = Promise.resolve();
  private readonly browser: Browser;
  private readonly baseURL: string;

  private constructor(browser: Browser, baseURL: string, state: Storage, signedIn: Auth) {
    this.browser = browser; this.baseURL = baseURL;
    this.state = state; this.current = signedIn; this.original = { theme: signedIn.theme, locale: signedIn.locale };
  }
  static async signIn(browser: Browser, config: CaptureConfig, actor: Exclude<Actor, 'signed-out'>): Promise<CaptureSession> {
    const context = await browser.newContext({ baseURL: config.baseURL, viewport: { width: 1440, height: 900 }, locale: 'en-US', timezoneId: 'UTC' });
    let retained = false;
    try {
      await appearance(context, 'en', 'light');
      const page = await context.newPage();
      page.setDefaultTimeout(ACTION_MS); page.setDefaultNavigationTimeout(READY_MS);
      const person = config.people[actor === 'ada-admin' ? 'ada' : actor];
      await page.goto('/login');
      await page.getByLabel('Username or email address', { exact: true }).fill(person.login);
      await page.getByLabel('Password', { exact: true }).fill(person.password);
      // Subscribe before the click. Do not leave an orphaned response waiter when a control is missing.
      const [answer] = await Promise.all([
        page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/v1/auth/login', { timeout: READY_MS }),
        page.getByRole('button', { name: 'Sign in', exact: true }).click(),
      ]);
      if (!answer.ok()) throw new Error('A supplied person could not sign in with a password.');
      const signedIn = auth(await bounded<unknown>(answer.json()));
      await page.waitForURL((url) => url.pathname !== '/login', { timeout: READY_MS });
      const enabled = actor === 'ada-admin';
      if (enabled && !signedIn.admin) throw new Error('The supplied Ada has no system administrator mode.');
      if (signedIn.admin) {
        const mode = page.locator('[data-screen-titlebar]:visible [role="switch"][data-as-admin]');
        await expect(mode).toBeVisible({ timeout: READY_MS });
        if (await mode.getAttribute('data-as-admin') !== String(enabled)) await mode.click();
        await expect(mode).toHaveAttribute('data-as-admin', String(enabled));
        await expect(mode).toHaveAttribute('aria-checked', String(enabled));
      }
      const session = new CaptureSession(browser, config.baseURL, await bounded(context.storageState()), signedIn);
      retained = true;
      return session;
    } finally {
      try {
        if (!retained) await context.request.post('/api/v1/auth/logout', { headers: { 'X-Orch-Csrf': '1' }, timeout: READY_MS }).catch(() => undefined);
      } finally { await context.close(); }
    }
  }

  /** Every page starts with cold JS/query caches and a clean shipped workbench, but the same real session. */
  async context(viewport: { width: number; height: number }, locale: CaptureLocale, theme: CaptureTheme): Promise<BrowserContext> {
    const context = await this.browser.newContext({ baseURL: this.baseURL, storageState: freshStorage(this.state), viewport, colorScheme: theme, locale: locale === 'en' ? 'en-US' : locale, timezoneId: 'UTC', deviceScaleFactor: 1 });
    try { await appearance(context, locale, theme); return context; }
    catch (error) { await context.close(); throw error; }
  }
  track(page: Page): void {
    const update = (response: Response): void => {
      if (new URL(response.url()).pathname !== '/api/v1/auth/refresh' || !response.ok()) return;
      this.updates = this.updates.then(async () => { this.current = auth(await bounded<unknown>(response.json())); }).catch(() => { /* The bounded setup refresh recovers a response interrupted by page closure. */ });
    };
    page.on('response', update);
  }
  async keep(context: BrowserContext): Promise<void> {
    await this.updates;
    // Refresh cookies rotate. Reusing the original sign-in snapshot would expire the next page's session.
    this.state = await bounded(context.storageState());
  }
  async prepare(locale: CaptureLocale, theme: CaptureTheme): Promise<void> {
    await this.withSession(async (context) => {
      const reply = await context.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${this.current.token}` }, data: { locale, theme }, timeout: READY_MS });
      if (!reply.ok()) throw new Error('The capture appearance could not be saved.');
      this.lastChanged = { locale, theme };
    });
  }
  private async withSession(run: (context: BrowserContext) => Promise<void>): Promise<void> {
    const context = await this.browser.newContext({ baseURL: this.baseURL, storageState: this.state });
    try {
      const refreshed = await context.request.post('/api/v1/auth/refresh', { headers: { 'X-Orch-Csrf': '1' }, timeout: READY_MS });
      if (!refreshed.ok()) throw new Error('The capture session could not be renewed.');
      this.current = auth(await bounded<unknown>(refreshed.json()));
      await run(context);
    } finally { try { this.state = await bounded(context.storageState()); } finally { await context.close(); } }
  }
  async close(): Promise<void> {
    await this.withSession(async (context) => {
      const failures: string[] = [];
      try {
        const changed = this.lastChanged;
        if (changed !== undefined) {
          // A simultaneous human preference edit wins. Restore only fields still equal to our last write.
          const restore = { ...(this.current.theme === changed.theme ? { theme: this.original.theme } : {}), ...(this.current.locale === changed.locale ? { locale: this.original.locale } : {}) };
          if (Object.keys(restore).length > 0) {
            const restored = await context.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${this.current.token}` }, data: restore, timeout: READY_MS });
            if (!restored.ok()) throw new Error('The capture appearance could not be restored.');
          }
        }
      } catch {
        failures.push('The capture appearance could not be restored.');
      }
      // Signing out is still attempted after a failed restore; neither failure replaces the other.
      try {
        const ended = await context.request.post('/api/v1/auth/logout', { headers: { 'X-Orch-Csrf': '1' }, timeout: READY_MS });
        if (!ended.ok()) throw new Error('The capture session could not be signed out.');
      } catch {
        failures.push('The capture session could not be signed out.');
      }
      if (failures.length > 0) throw new Error(failures.join(' '));
    });
  }
}
