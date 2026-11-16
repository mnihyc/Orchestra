/** The page names the files its entry would only find once it had run, so that they are fetched with it. */
import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { resolve } from 'node:path';
import { ESLint } from 'eslint';

import { type BuiltChunk, firstWaveFiles, firstWavePlugin } from '../../build/first-wave';
import { vocabularyPlan } from '../../build/vocabulary';

const chunk = (fileName: string, rest: Partial<BuiltChunk> = {}): BuiltChunk => ({ type: 'chunk', fileName, ...rest });
// Loading the TypeScript lint configuration is fixture setup, outside the program's lint case.
const starterLinter = new ESLint({ cwd: process.cwd() });
await starterLinter.calculateConfigForFile('src/shell/startup.inline.js');

it('checks the verbatim HTML request starter with the shipped JavaScript lint rules', async () => {
  const results = await starterLinter.lintFiles(['src/shell/startup.inline.js']);
  expect(results.flatMap((result) => result.messages.filter((message) => message.severity === 2))).toEqual([]);
});

it('names the complete frame dependency wave in the HTML instead of discovering shared scripts after execution', () => {
  const shell = { ...chunk('assets/shell.js', { name: 'shell' }), imports: ['assets/controls.js', 'assets/rows.js'] };
  const controls = { ...chunk('assets/controls.js'), imports: ['assets/runtime.js'] };
  const rows = { ...chunk('assets/rows.js'), imports: ['assets/runtime.js', 'assets/controls.js'] };
  const bundle = { shell, controls, rows, runtime: chunk('assets/runtime.js'), late: chunk('assets/editor.js') };
  expect(firstWaveFiles(bundle)).toEqual(['assets/controls.js', 'assets/rows.js', 'assets/runtime.js', 'assets/shell.js']);
  // Four preloaded scripts occupy one of the six-connection profile's exchanges; no dependency waits for execution.
  expect(Math.ceil(firstWaveFiles(bundle).length / 6)).toBe(1);
});

it('visits a shared dependency once even when chunks import each other', () => {
  const shell = { ...chunk('assets/shell.js', { name: 'shell' }), imports: ['assets/controls.js'] };
  const controls = { ...chunk('assets/controls.js'), imports: ['assets/shell.js'] };
  expect(firstWaveFiles({ shell, controls })).toEqual(['assets/controls.js', 'assets/shell.js']);
});

it('names the actual frame entry and page manifest beside their shared group before either import executes', () => {
  const bundle = {
    shell: chunk('assets/shell-shared.js', { name: 'shell' }),
    facade: chunk('assets/Shell-entry.js', { facadeModuleId: '/work/frontend/src/shell/Shell.tsx', imports: ['assets/shell-shared.js'] }),
    pages: chunk('assets/pages.js', { facadeModuleId: '/work/frontend/src/features/workbench/pages.ts', imports: ['assets/router.js'] }),
    router: chunk('assets/router.js'),
    editor: chunk('assets/editor.js', { facadeModuleId: '/work/frontend/src/ui/CodeView.tsx' }),
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/Shell-entry.js', 'assets/pages.js', 'assets/router.js', 'assets/shell-shared.js']);
});

it('starts the session from the HTML before any script chunk and starts one bootstrap at the first answer', async () => {
  const plugin = firstWavePlugin(), hook = plugin.transformIndexHtml;
  if (typeof hook !== 'object' || !('handler' in hook)) throw new Error('The production HTML transform is required.');
  const html = '<!doctype html><html><head><meta charset="utf-8"><script type="module" src="/assets/initial.js"></script></head><body></body></html>';
  const bundle = {
    initial: { ...chunk('assets/initial.js'), isEntry: true, imports: ['assets/runtime.js'] },
    runtime: chunk('assets/runtime.js'),
    shell: chunk('assets/shell.js', { name: 'shell' }),
    thread: chunk('assets/thread.js', { facadeModuleId: '/work/frontend/src/routes/thread/$threadId/index.tsx?tsr-split=component' }),
    editor: chunk('assets/editor.js'),
  };
  const transformed = await hook.handler.call({} as ThisParameterType<typeof hook.handler>, html, { path: '/thread/one', filename: 'index.html', bundle } as unknown as Parameters<typeof hook.handler>[1]);
  const built = typeof transformed === 'object' && 'html' in transformed ? transformed.html : html;
  const document = new DOMParser().parseFromString(built, 'text/html');
  const scripts = [...document.querySelectorAll('script')];
  expect(scripts[0]?.hasAttribute('data-orch-startup')).toBe(true);
  expect(document.querySelector('script[type="module"][src]')).toBeNull();
  const source = scripts[0]?.textContent;
  if (source === undefined) throw new Error('The actual inline startup program is required.');
  let answer!: (response: Response) => void;
  const fetch = vi.fn<(path: string, init?: RequestInit) => Promise<Response>>().mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }))
    .mockResolvedValueOnce(Response.json({ person: { id: 'own-reader' }, personal_revision: { personal: 0, inbox: 0, meta: 0, authority: 0 } }));
  const window = { fetch, document, location: new URL('https://example.invalid/thread/one'), navigator: { language: 'en' }, AbortController, Intl, Date };
  runInNewContext(source, { window, URL, Request, Response, Headers, AbortController, Promise, Date, Intl, setTimeout, clearTimeout });
  expect(fetch.mock.calls.map(([path, init]) => [path, init?.method])).toEqual([['/api/v1/auth/session', 'POST']]);
  expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: 'same-origin', headers: { 'x-orch-csrf': '1' } });
  const preloaded = () => [...document.querySelectorAll('link[rel="modulepreload"]')].map((link) => link.getAttribute('href'));
  expect(preloaded()).toEqual(['/assets/initial.js', '/assets/runtime.js']);
  answer(Response.json({ user: { id: 'own-reader', locale: 'en' }, access_token: 'synthetic-html-token', access_expires_at: null }));
  await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  expect(fetch.mock.calls[1]?.[0]).toBe('/api/v1/me/bootstrap');
  expect(fetch.mock.calls[1]?.[1]?.headers).toMatchObject({ Authorization: 'Bearer synthetic-html-token' });
  expect(preloaded()).toEqual(['/assets/initial.js', '/assets/runtime.js', '/assets/shell.js', '/assets/thread.js']);
  expect(preloaded()).not.toContain('/assets/editor.js');
});

it('permits only the response nonce for the inline request starter at the stock edge', () => {
  const policy = readFileSync(resolve(process.cwd(), '../deploy/nginx.conf'), 'utf8');
  expect(policy).toContain("'nonce-$request_id'");
  expect(policy).toContain("sub_filter '__ORCH_CSP_NONCE__' $request_id;");
  const script = /script-src ([^;]+)/.exec(policy)?.[1];
  expect(script).not.toContain("'unsafe-inline'");
});

it('preloads the frame without transferring another reader\'s whole English catalogue', () => {
  const bundle = {
    'assets/initial-aaa.js': chunk('assets/initial-aaa.js', { name: 'initial' }),
    'assets/shell-bbb.js': chunk('assets/shell-bbb.js', { name: 'shell' }),
    'assets/en-ccc.js': chunk('assets/en-ccc.js', { name: 'en', facadeModuleId: '/work/frontend/src/i18n/en.json' }),
    'assets/zh-CN-ddd.js': chunk('assets/zh-CN-ddd.js', { name: 'zh-CN', facadeModuleId: '/work/frontend/src/i18n/zh-CN.json' }),
    'assets/Thread-eee.js': chunk('assets/Thread-eee.js', { name: 'Thread', facadeModuleId: '/work/frontend/src/features/thread/Thread.tsx' }),
    'assets/inter-fff.woff2': { type: 'asset', fileName: 'assets/inter-fff.woff2' },
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/shell-bbb.js']);
});

it('keeps feature words outside the first vocabulary and gives both languages the same parts', { timeout: 120_000 }, async () => {
  const plan = await vocabularyPlan(process.cwd());
  const initial = plan.parts.get('initial');
  expect(initial).toBeDefined();
  expect(initial?.keys.has('common.cancel')).toBe(true);
  expect(initial?.keys.has('settings.interface.reading.title')).toBe(false);
  const feature = plan.moduleParts.get('routes/settings/interface/index.tsx');
  expect(feature).toBeDefined();
  expect(plan.parts.get(feature ?? '')?.keys.has('settings.interface.reading.title')).toBe(true);
  for (const part of plan.parts.values()) expect(Object.keys(part.words.en)).toEqual(Object.keys(part.words['zh-CN']));
});
