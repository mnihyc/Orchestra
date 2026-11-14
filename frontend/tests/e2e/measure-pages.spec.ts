/** The measurement command is exercised against held HTTP and real tab controls; shipped words use the production page. */
import { execFile } from 'node:child_process';
import { createServer, type Server } from 'node:http';
import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

const command = (args: string[]) => new Promise<{ code: string | number; out: string; error: string }>((resolve) => {
  execFile(process.execPath, ['tests/e2e/measure-pages.ts', '--runs', '1', '--timeout', '10000', ...args], (error, out, stderr) => {
    resolve({ code: error?.code ?? 0, out, error: stderr });
  });
});

async function fixture(): Promise<{ base: string; close: () => Promise<void> }> {
  const timers = new Set<ReturnType<typeof setTimeout>>();
  const server: Server = createServer((request, response) => {
    const path = new URL(request.url ?? '/', 'http://fixture.invalid').pathname;
    if (path === '/redirect') { response.writeHead(302, { Location: '/login' }); response.end(); return; }
    if (path.startsWith('/api/')) {
      const finish = () => { response.writeHead(200, { 'Content-Type': 'application/json' }); response.end(JSON.stringify({ rows: ['An answer held beyond visible content'] })); };
      if (path.endsWith('/slow')) { const timer = setTimeout(() => { timers.delete(timer); finish(); }, 1000); timers.add(timer); } else finish();
      return;
    }
    if (path === '/favicon.ico') { response.writeHead(204); response.end(); return; }
    response.writeHead(200, { 'Content-Type': 'text/html' });
    response.end(`<!doctype html><html><body><main><button role="tab" id="tab-one">One</button><button role="tab" id="tab-two">Two</button><p id="hidden" style="display:none">Hidden content</p></main><script>
      const roots = new Map();
      function show(path) {
        for (const root of roots.values()) root.style.display = 'none';
        let root = roots.get(path);
        if (!root) {
          root = document.createElement('div'); root.dataset.viewInstance = path.slice(1); root.dataset.pageAddress = path;
          root.textContent = path === '/error' ? 'A very long failure sentence that must never count as the requested successful page content.' : 'Ready';
          document.querySelector('main').append(root); roots.set(path, root);
          fetch(path === '/held' ? '/api/v1/slow' : '/api/v1/fast');
          requestAnimationFrame(() => { root.dataset.pageContent = path === '/error' ? 'error' : 'ready'; });
        }
        root.style.display = '';
      }
      for (const name of ['one', 'two']) document.getElementById('tab-' + name).onclick = () => { performance.mark('orch/switch/click', { detail: { tab: name } }); history.pushState({}, '', '/' + name); show('/' + name); };
      addEventListener('popstate', () => show(location.pathname)); show(location.pathname);
    </script></body></html>`);
  });
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('The measurement fixture needs a TCP address');
  return { base: `http://127.0.0.1:${address.port}`, close: async () => {
    for (const timer of timers) clearTimeout(timer);
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close((error) => error === undefined ? resolve() : reject(error)));
  } };
}

interface Report { rows: { calls: number; content: number; runs: { requests: { path: string; end: number; decoded: number }[] }[] }[] }

test('the command counts and finishes a response held past visible content', async () => {
  const info = test.info();
  const host = await fixture(), report = info.outputPath('held.json');
  try {
    const result = await command(['--base', host.base, '--report', report, '/held']);
    expect(result.code, result.error).toBe(0);
    const row = (JSON.parse(await readFile(report, 'utf8')) as Report).rows[0];
    expect(row?.calls).toBe(1);
    const held = row?.runs[0]?.requests.find((request) => request.path === '/api/v1/slow');
    expect(held?.decoded).toBeGreaterThan(0);
    expect(held?.end).toBeGreaterThan(row?.content ?? Infinity);
  } finally { await host.close(); }
});

test('the command refuses redirects, native failures and hidden supplied selectors', async () => {
  const host = await fixture();
  try {
    for (const address of ['/redirect', '/error']) {
      const result = await command(['--base', host.base, address]);
      expect(result.code).toBe(1); expect(result.error).toContain(address === '/redirect' ? 'redirect' : 'error');
      expect(result.out).not.toContain('| Content ms |');
    }
    const hidden = await command(['--base', host.base, '--timeout', '500', '--ready', '#hidden', '/one']);
    expect(hidden.code).toBe(1);
  } finally { await host.close(); }
});

test('the command clicks a retained tab and compares it with its accepted run', async () => {
  const info = test.info();
  const host = await fixture(), accepted = info.outputPath('accepted.json');
  try {
    const args = ['--base', host.base, '--scenario', 'switch', '--via', '/two'];
    const first = await command([...args, '--report', accepted, '/one']);
    expect(first.code, first.error).toBe(0);
    const second = await command([...args, '--baseline', accepted, '--allow-ms', '10000', '/one']);
    expect(second.code, second.error).toBe(0);
    expect(second.out).toContain('| switch | 0 |'); expect(second.out).not.toContain('uncompared');
  } finally { await host.close(); }
});

for (const locale of ['en', 'zh-CN'] as const) for (const width of [1440, 400]) test(`the shipped cold page fetches only its locale's first vocabulary at ${locale}/${width}`, async ({ page }, info) => {
  await scenario(); await page.setViewportSize({ width, height: 900 }); await signIn(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: 'Language', exact: true }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    // The verified reader outranks the language left by another person on this device.
    await page.evaluate(() => { const raw = JSON.parse(localStorage.getItem('orch.appearance') ?? '{}') as Record<string, unknown>; localStorage.setItem('orch.appearance', JSON.stringify({ ...raw, locale: 'en' })); });
  }
  const requests: string[] = [];
  page.on('request', (request) => requests.push(new URL(request.url()).pathname));
  const words = locale === 'en' ? english : chinese;
  await page.goto('/resources/providers');
  await expect(page.getByRole('heading', { name: words.screens.providers.title, exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
  expect(requests.filter((path) => /^\/assets\/(?:en|zh-CN)-[^/]+\.js$/.test(path))).toEqual([]);
  expect(requests.some((path) => path.startsWith(`/assets/words/${locale}-`))).toBe(true);
  if (locale === 'zh-CN') expect(requests.some((path) => path.startsWith('/assets/words/en-'))).toBe(false);
  await page.screenshot({ path: info.outputPath(`first-words-${locale}-${width}.png`), fullPage: true });
});
