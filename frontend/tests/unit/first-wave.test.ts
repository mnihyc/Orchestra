/** The page names the files its entry would only find once it had run, so that they are fetched with it. */
import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { resolve } from 'node:path';
import { ESLint } from 'eslint';
import type { ChunkingContext, ModuleInfo } from 'rolldown';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';

import { type BuiltChunk, firstScreenPlan, firstWaveFiles, firstWavePlugin, threadShellWorkerPlugin } from '../../build/first-wave';
import { staticChunkGroup } from '../../build/frame-chunks';
import { vocabularyPlan } from '../../build/vocabulary';
import type { HtmlReads } from '@/api/htmlReads';
import { assetFileName, inlineAsset } from '../../build/fonts';
import { applicationInitialization } from '@/shell/applicationInitialization';

const chunk = (fileName: string, rest: Partial<BuiltChunk> = {}): BuiltChunk => ({ type: 'chunk', fileName, ...rest });
// Loading the TypeScript lint configuration is fixture setup, outside the program's lint case.
const starterLinter = new ESLint({ cwd: process.cwd() });
await starterLinter.calculateConfigForFile('src/shell/startup.inline.js');

it('packs every static frame widget once while leaving optional viewers for their own opening', () => {
  const module = (id: string, importedIds: string[], dynamicallyImportedIds: string[] = []): ModuleInfo => ({
    id, importedIds, dynamicallyImportedIds, importers: [], dynamicImporters: [], exports: [],
    isEntry: false, inputFormat: 'es', ast: null, code: '', meta: {}, moduleSideEffects: true,
  });
  const modules = [module('frame', ['widget', 'controls'], ['editor']), module('widget', ['controls']),
    module('controls', ['widget']), module('other-page', [], ['widget']), module('editor', ['editor-state']), module('editor-state', [])];
  const graph = new Map(modules.map((item) => [item.id, item]));
  const getModuleInfo = vi.fn((id: string) => graph.get(id) ?? null);
  const context: ChunkingContext = { getModuleInfo };
  const group = staticChunkGroup('frame', 'frame', 1);
  const name = group.name;
  if (typeof name !== 'function') throw new Error('The frame group must match the actual module graph.');
  expect(modules.map(({ id }) => [id, name(id, context)])).toEqual([
    ['frame', 'frame'], ['widget', 'frame'], ['controls', 'frame'], ['other-page', null], ['editor', null], ['editor-state', null],
  ]);
  expect(getModuleInfo).toHaveBeenCalledTimes(3);
  expect(group.includeDependenciesRecursively).toBe(false);
});

it('checks the verbatim HTML request starter with the shipped JavaScript lint rules', async () => {
  const results = await starterLinter.lintFiles(['src/shell/startup.inline.js']);
  expect(results.flatMap((result) => result.messages.filter((message) => message.severity === 2))).toEqual([]);
});

it('refuses an entry whose compressed shell wave would take the long line to draw', async () => {
  const plugin = firstWavePlugin(), generate = plugin.generateBundle;
  if (typeof generate !== 'function') throw new Error('The actual build must check the emitted shell wave.');
  const code = `export const values=${JSON.stringify(Array.from({ length: 12_000 }, (_, index) => createHash('sha256').update(String(index)).digest('hex')))};`;
  const bundle = { initial: chunk('assets/initial.js', { isEntry: true, code }) };
  const context = { warn: vi.fn(), error: (message: string): never => { throw new Error(message); } } as unknown as ThisParameterType<typeof generate>;
  await expect(Promise.resolve().then(() => generate.call(context, {} as Parameters<typeof generate>[0],
    bundle as unknown as Parameters<typeof generate>[1], false))).rejects.toThrow(/compressed|KiB|first.*wave/i);
});

it('counts blocking style bytes as well as JavaScript in the first-shell transfer budget', async () => {
  const plugin = firstWavePlugin(), generate = plugin.generateBundle;
  if (typeof generate !== 'function') throw new Error('The actual build must check the emitted shell wave.');
  const source = Array.from({ length: 12_000 }, (_, index) => `--colour-${index}:#${createHash('sha256').update(String(index)).digest('hex')};`).join('');
  const bundle = { initial: chunk('assets/initial.js', { isEntry: true, code: '', viteMetadata: { importedCss: new Set(['assets/initial.css']) } }),
    css: { type: 'asset', fileName: 'assets/initial.css', source } };
  const context = { warn: vi.fn(), error: (message: string): never => { throw new Error(message); } } as unknown as ThisParameterType<typeof generate>;
  await expect(Promise.resolve().then(() => generate.call(context, {} as Parameters<typeof generate>[0],
    bundle as unknown as Parameters<typeof generate>[1], false))).rejects.toThrow(/compressed|KiB|first.*wave/i);
});

it('counts the distinct code and blocking styles for the addressed conversation against its two-flight target', async () => {
  const generate = firstWavePlugin().generateBundle;
  if (typeof generate !== 'function') throw new Error('The production build must measure the addressed conversation.');
  const style = 'p{color:green}';
  const wrapperStyle = 'article{display:grid}';
  const codes = ['entry()', 'frame()', 'thread()', 'application()'] as const;
  const css = { importedCss: new Set(['style.css']) };
  const bundle = {
    entry: chunk('entry.js', { isEntry: true, code: codes[0], viteMetadata: css }),
    wrapper: chunk('wrapper.js', { facadeModuleId: '/src/application.tsx', imports: ['entry.js', 'application.js'],
      code: 'import "./entry.js";import "./application.js";', viteMetadata: { importedCss: new Set(['wrapper.css']) } }),
    application: chunk('application.js', { code: codes[3], imports: ['entry.js'] }),
    wrapperStyle: { type: 'asset', fileName: 'wrapper.css', source: wrapperStyle },
    frame: chunk('frame.js', { facadeModuleId: '/src/shell/Shell.tsx', imports: ['entry.js'], code: codes[1], viteMetadata: css }),
    thread: chunk('thread.js', { facadeModuleId: '/src/routes/thread/$threadId/index.tsx', imports: ['frame.js'], code: codes[2], viteMetadata: css }),
    help: chunk('help.js', { facadeModuleId: '/src/routes/help/index.tsx', code: 'help()' }),
    style: { type: 'asset', fileName: 'style.css', source: style },
  };
  const warn = vi.fn();
  await generate.call({ warn } as unknown as ThisParameterType<typeof generate>, {} as Parameters<typeof generate>[0],
    bundle as unknown as Parameters<typeof generate>[1], false);
  const bytes = [...codes, style, wrapperStyle].reduce((total, code) => total + gzipSync(code, { level: 1 }).byteLength, 0);
  expect(warn).toHaveBeenCalledWith(expect.stringContaining(`Addressed thread compressed transfer: ${bytes} bytes (scripts: 4, stylesheets: 2); two-flight target 196608 bytes`));
});

it('preloads both normal text fonts from their own origin under the unchanged font policy', async () => {
  const plugin = firstWavePlugin(), hook = plugin.transformIndexHtml;
  if (typeof hook !== 'object' || !('handler' in hook)) throw new Error('The production HTML transform is required.');
  const bundle = {
    initial: chunk('assets/initial.js', { isEntry: true }),
    inter: { type: 'asset', fileName: 'fonts/inter-latin-wght-normal-hash.woff2' },
    mono: { type: 'asset', fileName: 'fonts/jetbrains-mono-latin-wght-normal-hash.woff2' },
    italic: { type: 'asset', fileName: 'fonts/inter-latin-wght-italic-hash.woff2' },
  };
  const html = '<html><head><meta charset="utf-8"></head><body></body></html>';
  const result = await hook.handler.call({} as ThisParameterType<typeof hook.handler>, html,
    { path: '/', filename: 'index.html', bundle } as unknown as Parameters<typeof hook.handler>[1]);
  const built = typeof result === 'object' && 'html' in result ? result.html : html;
  const document = new DOMParser().parseFromString(built, 'text/html');
  expect([...document.querySelectorAll('link[rel="preload"][as="font"]')].map((link) => [link.getAttribute('href'), link.getAttribute('crossorigin')])).toEqual([
    ['/fonts/inter-latin-wght-normal-hash.woff2', 'anonymous'], ['/fonts/jetbrains-mono-latin-wght-normal-hash.woff2', 'anonymous'],
  ]);
  expect([...document.querySelectorAll('link[as="font"]')].every((link) => link.getAttribute('media') === 'not all')).toBe(true);
  const policy = readFileSync(resolve(process.cwd(), '../deploy/nginx.conf'), 'utf8');
  expect(policy).toContain("font-src 'self';");
  expect(policy).not.toContain("font-src 'self' data:");
});

it('starts the compiled application directly when its wrapper only repeats the implementation imports', () => {
  const bundle = {
    initial: chunk('assets/initial.js', { isEntry: true }),
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: 'import "./initial.js";import "./application.js";', imports: ['assets/initial.js', 'assets/application.js'] }),
    application: chunk('assets/application.js', { code: 'startApplication();', imports: ['assets/initial.js'], viteMetadata: { importedCss: new Set(['assets/application.css']) } }),
    css: { type: 'asset', fileName: 'assets/application.css', source: 'body{color:black}' },
  };
  const plan = firstScreenPlan(bundle);
  expect(plan).toMatchObject({ application: { file: 'assets/application.js', styles: ['assets/application.css'] } });
  expect(plan.common).toEqual(['assets/application.js', 'assets/initial.js']);
});

it('keeps an imported application initializer and its order when the wrapper only starts the compiled factories', () => {
  const bundle = {
    initial: chunk('assets/initial.js', { isEntry: true }),
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: 'import "./initial.js";import { a as first, b as second } from "./application.js";first();second();', imports: ['assets/initial.js', 'assets/application.js'] }),
    application: chunk('assets/application.js', { imports: ['assets/initial.js'] }),
  };
  const plan = firstScreenPlan(bundle);
  expect(plan).toMatchObject({ application: { file: 'assets/application.js', initializers: ['a', 'b'] } });
  expect(plan.common).not.toContain('assets/application-entry.js');
});

it('starts the admitted application once and in order when concurrent readers join its module', async () => {
  let finish!: (module: unknown) => void;
  const load = vi.fn<(href: string) => Promise<unknown>>(() => new Promise((resolve) => { finish = resolve; }));
  const start = applicationInitialization(load);
  const order: string[] = [];
  const module = { a: vi.fn(() => { order.push('a'); }), b: vi.fn(() => { order.push('b'); }) };
  const first = start('/assets/app.js', ['a', 'b']), joined = start('/assets/app.js', ['a', 'b']);
  expect(joined).toBe(first); expect(order).toEqual([]);
  finish(module); await first;
  expect(order).toEqual(['a', 'b']);
  expect(await start('/assets/app.js', ['a', 'b'])).toBe(module);
  expect(load).toHaveBeenCalledOnce(); expect(module.a).toHaveBeenCalledOnce(); expect(module.b).toHaveBeenCalledOnce();
});

it('retries a failed application transfer without repeating a completed initializer', async () => {
  const initialize = vi.fn();
  const load = vi.fn<(href: string) => Promise<unknown>>().mockRejectedValueOnce(new Error('Synthetic transfer failure')).mockResolvedValueOnce({ initialize });
  const start = applicationInitialization(load);
  await expect(start('/assets/app.js', ['initialize'])).rejects.toThrow('Synthetic transfer failure');
  await start('/assets/app.js', ['initialize']); await start('/assets/app.js', ['initialize']);
  expect(load).toHaveBeenCalledTimes(2); expect(initialize).toHaveBeenCalledOnce();
});

it('keeps an application initialization failure rather than replaying partially applied effects', async () => {
  const initialize = vi.fn(() => { throw new Error('Synthetic initialization failure'); });
  const load = vi.fn<(href: string) => Promise<unknown>>().mockResolvedValue({ initialize });
  const start = applicationInitialization(load);
  await expect(start('/assets/app.js', ['initialize'])).rejects.toThrow('Synthetic initialization failure');
  await expect(start('/assets/app.js', ['initialize'])).rejects.toThrow('Synthetic initialization failure');
  expect(load).toHaveBeenCalledOnce(); expect(initialize).toHaveBeenCalledOnce();
});

it.each(['export {};', ';', '"use strict";'])('does not transfer an extra application wrapper for a module marker %s', (marker) => {
  const bundle = {
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: `import "./application.js";${marker}`, imports: ['assets/application.js'] }),
    application: chunk('assets/application.js'),
  };
  expect(firstScreenPlan(bundle)).toMatchObject({ application: { file: 'assets/application.js', initializers: [] } });
});

it('retains an application wrapper whose call has an argument rather than changing its initialization', () => {
  const bundle = {
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: 'import { a as start } from "./application.js";start(1);', imports: ['assets/application.js'] }),
    application: chunk('assets/application.js'),
  };
  expect(firstScreenPlan(bundle)).toMatchObject({ application: { file: 'assets/application-entry.js', initializers: [] } });
});

it('keeps application entry code whenever the implementation cannot supply all of its effects', () => {
  const bundle = {
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: 'import "./application.js";registerApplication();', imports: ['assets/application.js'] }),
    application: chunk('assets/application.js'),
  };
  expect(firstScreenPlan(bundle)).toMatchObject({ application: { file: 'assets/application-entry.js' } });
});

it('keeps a separate application entry when another imported effect is outside the implementation', () => {
  const bundle = {
    wrapper: chunk('assets/application-entry.js', { facadeModuleId: '/work/frontend/src/application.tsx',
      code: 'import "./application.js";import "./registration.js";', imports: ['assets/application.js', 'assets/registration.js'] }),
    application: chunk('assets/application.js'),
    registration: chunk('assets/registration.js'),
  };
  const plan = firstScreenPlan(bundle);
  expect(plan).toMatchObject({ application: { file: 'assets/application-entry.js' } });
  expect(plan.common).toContain('assets/registration.js');
});

it('keeps every font face external while leaving other small assets to their normal build policy', () => {
  for (const file of ['inter.woff2', 'italic.woff', 'font.ttf', 'font.otf']) {
    expect(inlineAsset(file)).toBe(false);
    expect(assetFileName({ names: [file] })).toBe('fonts/[name]-[hash][extname]');
  }
  expect(inlineAsset('mark.svg')).toBeUndefined();
  expect(assetFileName({ names: ['mark.svg'] })).toBe('assets/[name]-[hash][extname]');
});

it('names the complete frame dependency wave in the HTML instead of discovering shared scripts after execution', () => {
  const shell = { ...chunk('assets/shell.js', { name: 'shell', facadeModuleId: '/work/frontend/src/shell/Shell.tsx' }), imports: ['assets/controls.js', 'assets/rows.js'] };
  const controls = { ...chunk('assets/controls.js'), imports: ['assets/runtime.js'] };
  const rows = { ...chunk('assets/rows.js'), imports: ['assets/runtime.js', 'assets/controls.js'] };
  const bundle = { shell, controls, rows, runtime: chunk('assets/runtime.js'), late: chunk('assets/editor.js') };
  expect(firstWaveFiles(bundle)).toEqual(['assets/controls.js', 'assets/rows.js', 'assets/runtime.js', 'assets/shell.js']);
  // Four preloaded scripts occupy one of the six-connection profile's exchanges; no dependency waits for execution.
  expect(Math.ceil(firstWaveFiles(bundle).length / 6)).toBe(1);
});

it('visits a shared dependency once even when chunks import each other', () => {
  const shell = { ...chunk('assets/shell.js', { name: 'shell', facadeModuleId: '/work/frontend/src/shell/Shell.tsx' }), imports: ['assets/controls.js'] };
  const controls = { ...chunk('assets/controls.js'), imports: ['assets/shell.js'] };
  expect(firstWaveFiles({ shell, controls })).toEqual(['assets/controls.js', 'assets/shell.js']);
});

it('does not preload an unrelated icon just because the bundler named it shell', () => {
  const bundle = {
    frame: chunk('assets/frame.js', { facadeModuleId: '/work/frontend/src/shell/Shell.tsx' }),
    icon: chunk('assets/icon.js', { name: 'shell', facadeModuleId: '/work/frontend/node_modules/icons/shell.js' }),
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/frame.js']);
});

it('preloads a manually grouped frame by its actual contained module when it has no facade', () => {
  const bundle = {
    frame: chunk('assets/frame.js', { facadeModuleId: null, moduleIds: ['/work/frontend/src/shell/Shell.tsx'], imports: ['assets/shared.js'] }),
    shared: chunk('assets/shared.js'),
    icon: chunk('assets/icon.js', { facadeModuleId: null, moduleIds: ['/work/frontend/node_modules/icons/shell.js'] }),
  };
  expect(firstWaveFiles(bundle)).toEqual(['assets/frame.js', 'assets/shared.js']);
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
    shell: chunk('assets/shell.js', { name: 'shell', facadeModuleId: '/work/frontend/src/shell/Shell.tsx' }),
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
  const window: { __orchStartupReads?: HtmlReads } & Record<string, unknown> = { fetch, document, location: new URL('https://example.invalid/thread/one'), navigator: { language: 'en' }, AbortController, Intl, Date };
  runInNewContext(source, { window, URL, Request, Response, Headers, AbortController, Promise, Date, Intl, setTimeout, clearTimeout });
  expect(fetch.mock.calls.map(([path, init]) => [path, init?.method])).toEqual([['/api/v1/auth/session', 'POST']]);
  expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: 'same-origin', headers: { 'x-orch-csrf': '1' } });
  const preloaded = () => [...document.querySelectorAll('link[rel="modulepreload"]')].map((link) => link.getAttribute('href'));
  expect(preloaded()).toEqual(['/assets/initial.js', '/assets/runtime.js']);
  answer(Response.json({ user: { id: 'own-reader', locale: 'en' }, access_token: 'synthetic-html-token', access_expires_at: null }));
  await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  expect(fetch.mock.calls[1]?.[0]).toBe('/api/v1/me/bootstrap');
  expect(fetch.mock.calls[1]?.[1]?.headers).toMatchObject({ Authorization: 'Bearer synthetic-html-token' });
  // Even a fast authenticated answer cannot fetch feature code before the small shell paints.
  expect(preloaded()).toEqual(['/assets/initial.js', '/assets/runtime.js']);
  window.__orchStartupReads?.painted();
  expect(preloaded()).toEqual(['/assets/initial.js', '/assets/runtime.js', '/assets/shell.js', '/assets/thread.js']);
  expect(preloaded()).not.toContain('/assets/editor.js');
});

const threadId = '01000000-0000-7000-8000-000000000001';
const openingPath = `/api/v1/threads/${threadId}/opening`;
const cookieOpeningPath = `/api/v1/auth/session/thread/${threadId}`;
const documentSession = { user: { id: 'reader' }, access_token: 'fixture-token' };
function addressedDocument(address: string, opening = Response.json({ session: documentSession, opening: { status: 'ready', data: { path: openingPath } } })) {
  const origin = 'https://example.invalid';
  const document = new DOMParser().parseFromString('<html><head></head><body></body></html>', 'text/html');
  const fetch = vi.fn<(path: string, init?: RequestInit) => Promise<Response>>().mockImplementation((path) => {
    if (path === cookieOpeningPath) return Promise.resolve(opening);
    if (path.endsWith('/auth/session')) return Promise.resolve(Response.json(documentSession));
    return Promise.resolve(Response.json({ path }));
  });
  const window: { __orchStartupReads?: HtmlReads } & Record<string, unknown> = { fetch, document, location: new URL(address, origin), AbortController, Intl, Date };
  const source = readFileSync(resolve(process.cwd(), 'src/shell/startup.inline.js'), 'utf8');
  runInNewContext(`${source}(window,plan);`, { window, plan: { entry: [], common: [], pages: [] }, URL, Request, Response, Headers, AbortController, Promise, Date, Intl });
  const request = (path: string, token = 'fixture-token') => new Request(origin + path, { headers: { Authorization: `Bearer ${token}` } });
  return { fetch, provider: window.__orchStartupReads!, request };
}

const takeDocumentSession = (provider: HtmlReads) => provider.take(new Request('https://example.invalid/api/v1/auth/session', { method: 'POST', headers: { 'x-orch-csrf': '1' } }));

it('opens the addressed thread and restores its browser session with one request before content', async () => {
  const { fetch, provider, request } = addressedDocument(`/thread/${threadId}/`);
  expect(await (await takeDocumentSession(provider))?.response.json()).toEqual(documentSession);
  expect(fetch.mock.calls.map(([path, init]) => [path, init?.method])).toEqual([[cookieOpeningPath, 'POST']]);
  expect(fetch.mock.calls[0]?.[1]).toMatchObject({ credentials: 'same-origin', headers: { 'x-orch-csrf': '1' } });
  expect(provider.take(request(openingPath + '?continuation_id=elsewhere'))).toBeNull();
  const reply = await provider.take(request(openingPath));
  expect(await reply?.response.json()).toEqual({ path: openingPath });
  expect(provider.take(request(openingPath))).toBeNull();
  expect(fetch).toHaveBeenCalledTimes(1);
  provider.contentShown?.();
  expect((await provider.take(request('/api/v1/me/bootstrap')))?.response.ok).toBe(true);
  expect(fetch.mock.calls.map(([path]) => path)).toEqual([cookieOpeningPath, '/api/v1/me/bootstrap']);
});

it('preserves the signed in session when the addressed opening reports a refusal', async () => {
  const problem = { status: 403, code: 'authz.denied' };
  const { provider, request } = addressedDocument(`/thread/${threadId}`, Response.json({ session: documentSession, opening: { status: 'failed', problem, retry_after: 4 } }));
  expect(await (await takeDocumentSession(provider))?.response.json()).toEqual(documentSession);
  expect(provider.afterThreadContent).toBeUndefined();
  const reply = await provider.take(request(openingPath));
  expect(reply?.response.status).toBe(403);
  expect(reply?.response.headers.get('retry-after')).toBe('4');
  expect(await reply?.response.json()).toEqual(problem);
});

it('releases a waiting shell when cookie discovery finds an anonymous reader', async () => {
  const { provider, fetch } = addressedDocument(`/thread/${threadId}`, Response.json(null));
  const painted = provider.afterThreadContent;
  expect(painted).toBeDefined();
  expect(await (await takeDocumentSession(provider))?.response.json()).toBeNull();
  await painted;
  expect(provider.afterThreadContent).toBeUndefined();
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('keeps the session request alive if its opening reader leaves and stops the transport when the session ends', async () => {
  const { fetch, provider, request } = addressedDocument(`/thread/${threadId}`);
  const controller = new AbortController();
  const pending = provider.take(new Request(request(openingPath), { signal: controller.signal }));
  controller.abort();
  expect(await pending).toBeNull();
  const signal = fetch.mock.calls[0]?.[1]?.signal;
  expect(signal?.aborted).toBe(false);
  expect(await (await takeDocumentSession(provider))?.response.json()).toEqual(documentSession);
  provider.cancel();
  expect(signal?.aborted).toBe(true);
});

it('keeps the addressed thread reply within the session that requested it', async () => {
  const { provider, request } = addressedDocument(`/thread/${threadId}`);
  expect(await provider.take(request(openingPath, 'other-reader'))).toBeNull();
  expect(await provider.take(request(openingPath))).toBeNull();
});

it('leaves a selected history address to the reader that knows its continuation', async () => {
  const { fetch, provider, request } = addressedDocument(`/thread/${threadId}?continuation=other`);
  expect((await provider.take(request('/api/v1/me/bootstrap')))?.response.ok).toBe(true);
  expect(fetch.mock.calls.map(([path]) => path)).toEqual(['/api/v1/auth/session', '/api/v1/me/bootstrap']);
  expect(provider.take(request(openingPath))).toBeNull();
});

it('permits only the response nonce for the inline request starter at the stock edge', () => {
  const policy = readFileSync(resolve(process.cwd(), '../deploy/nginx.conf'), 'utf8');
  expect(policy).toContain("'nonce-$request_id'");
  expect(policy).toContain("sub_filter '__ORCH_CSP_NONCE__' $request_id;");
  const script = /script-src ([^;]+)/.exec(policy)?.[1];
  expect(script).not.toContain("'unsafe-inline'");
});

it('pins the finished public document to the worker and changes both together on an update', async () => {
  const hook = threadShellWorkerPlugin().generateBundle;
  if (typeof hook !== 'object' || !('handler' in hook)) throw new Error('The finished document must be available.');
  const revisions: string[] = [];
  const starter = '<script nonce="__ORCH_CSP_NONCE__" data-orch-startup></script>';
  for (const revision of ['first release', 'second release']) {
    const document = `<html><head>${starter}</head><body>${revision}</body></html>`;
    const emitFile = vi.fn<(file: { fileName: string; source: string }) => string>(() => '');
    await hook.handler.call({ emitFile } as unknown as ThisParameterType<typeof hook.handler>, {} as Parameters<typeof hook.handler>[0],
      { 'index.html': { type: 'asset', source: document } } as unknown as Parameters<typeof hook.handler>[1], false);
    const emitted = new Map(emitFile.mock.calls.map(([file]) => [file.fileName, file]));
    const digest = createHash('sha256').update(JSON.stringify(document)).digest();
    const asset = emitted.get(`assets/thread-shell-${digest.toString('hex')}.json`);
    const worker = emitted.get('sw.js');
    if (!asset || !worker) throw new Error('The build must emit the public document and its worker.');
    const login = emitted.get('login.html')?.source;
    expect(login).toContain('<template data-orch-login-meta><!--# include virtual="/internal/login-meta"');
    expect(login).toContain('</template>' + starter);
    expect(asset).toMatchObject({ fileName: `assets/thread-shell-${digest.toString('hex')}.json`, source: JSON.stringify(document) });
    expect(worker.fileName).toBe('sw.js');
    expect(worker.source).toContain(`sha256-${digest.toString('base64')}`);
    expect(worker.source).toContain('/' + asset.fileName);
    revisions.push(worker.source);
  }
  expect(revisions[0]).not.toBe(revisions[1]);
});

function threadWorker() {
  const template = '<!doctype html><script nonce="__ORCH_CSP_NONCE__" data-orch-startup>start()</script>';
  const asset = '/assets/thread-shell-fixture.json';
  const nonce = '00112233445566778899aabbccddeeff';
  const policy = `default-src 'self'; script-src 'self' 'nonce-${nonce}'; frame-ancestors 'none'`;
  const fetch = vi.fn<(arg: string | Request, init?: RequestInit) => Promise<Response>>((arg) => {
    if (arg !== asset) return Promise.resolve(new Response('network document'));
    return Promise.resolve(Response.json(template, { headers: { 'content-security-policy': policy, 'content-encoding': 'gzip', 'x-frame-options': 'DENY' } }));
  });
  interface FetchEvent {
    request: { url: string; mode: string; method: string };
    respondWith: (response: Promise<Response>) => void;
  }
  const listeners = new Map<string, (event: FetchEvent) => void>();
  const self = { location: new URL('https://example.invalid/sw.js'),
    addEventListener: (name: string, listener: (event: FetchEvent) => void) => listeners.set(name, listener) };
  const script = readFileSync(resolve(process.cwd(), 'public/sw.js'), 'utf8');
  runInNewContext(script.replace('const threadShell = null;', `const threadShell = ${JSON.stringify({ url: asset, integrity: 'sha256-fixture' })};`),
    { self, fetch, URL, Headers, Response, crypto });
  const navigate = (path: string, method = 'GET', mode = 'navigate') => {
    let answer: Promise<Response> | undefined;
    listeners.get('fetch')?.({ request: { url: new URL(path, self.location.origin).href, mode, method }, respondWith: (value) => { answer = value; } });
    return answer;
  };
  return { navigate, fetch, asset, nonce };
}

it('opens a returning thread from the immutable public shell with a fresh document nonce', async () => {
  const { navigate, fetch, asset, nonce } = threadWorker();
  const nonces = new Set<string>();
  for (let visit = 0; visit < 2; visit++) {
    const answer = await navigate(`/thread/${threadId}`);
    expect(answer).toBeDefined();
    const policy = answer?.headers.get('content-security-policy') ?? '';
    const current = /'nonce-([a-f0-9]{32})'/.exec(policy)?.[1];
    expect(current).toBeDefined();
    expect(current).not.toBe(nonce);
    nonces.add(String(current));
    expect(await answer?.text()).toContain(`nonce="${current}"`);
    expect(answer?.headers.get('content-encoding')).toBeNull();
    expect(answer?.headers.get('x-frame-options')).toBe('DENY');
  }
  expect(nonces.size).toBe(2);
  expect(fetch.mock.calls.map(([url]) => url)).toEqual([asset, asset]);
  expect(fetch.mock.calls[0]?.[1]).toMatchObject({ cache: 'force-cache', credentials: 'omit', integrity: 'sha256-fixture' });
  for (const path of ['/login', '/api/v1/auth/session', 'https://other.invalid/thread/' + threadId]) expect(navigate(path)).toBeUndefined();
  expect(navigate(`/thread/${threadId}`, 'POST')).toBeUndefined();
  expect(navigate(`/thread/${threadId}`, 'GET', 'cors')).toBeUndefined();
});

it('uses normal navigation when the release shell or its edge policy is unavailable', async () => {
  const { navigate, fetch } = threadWorker();
  for (const asset of [Response.json('missing the document policy'), new Response('missing', { status: 404 })]) {
    fetch.mockResolvedValueOnce(asset);
    expect(await (await navigate(`/thread/${threadId}`))?.text()).toBe('network document');
  }
});

it('takes the public sign-in policy from its document once without lending it to a signed-in reader', async () => {
  const document = new DOMParser().parseFromString('<html><head><template data-orch-login-meta></template></head></html>', 'text/html');
  const template = document.querySelector('template')!;
  const policy = { name: 'A & B <team>', local_login: false, may_create_org: false, needs_setup: false };
  template.content.textContent = JSON.stringify(policy);
  const fetch = vi.fn<(path: string) => Promise<Response>>().mockResolvedValue(Response.json(null));
  const window: { __orchStartupReads?: HtmlReads } & Record<string, unknown> = {
    fetch, document, location: new URL('https://example.invalid/login/?redirect=%2Fsettings'), AbortController, Intl,
  };
  const source = readFileSync(resolve(process.cwd(), 'src/shell/startup.inline.js'), 'utf8');
  runInNewContext(`${source}(window,plan);`, { window, plan: { entry: [], common: [], pages: [] }, URL, Request, Response, Headers, AbortController, Promise, Date, Intl });
  const request = new Request('https://example.invalid/api/v1/meta');
  const provider = window.__orchStartupReads!;
  expect(await provider.take(new Request(request, { headers: { Authorization: 'Bearer fixture' } }))).toBeNull();
  expect(await (await provider.take(request))?.response.json()).toEqual(policy);
  expect(await provider.take(request)).toBeNull();
  expect(fetch.mock.calls.map(([path]) => path)).toEqual(['/api/v1/auth/session']);
});

it('starts loading the sign-in form and its styles with the document while the workbench stays unopened', async () => {
  const document = new DOMParser().parseFromString('<html><head></head><body></body></html>', 'text/html');
  const fetch = vi.fn().mockResolvedValue(Response.json({ code: 'auth.required' }, { status: 401 }));
  const window: { __orchStartupReads?: HtmlReads } & Record<string, unknown> = { fetch, document, location: new URL('https://example.invalid/login'), AbortController, Intl, Date };
  const source = readFileSync(resolve(process.cwd(), 'src/shell/startup.inline.js'), 'utf8');
  runInNewContext(`${source}(window,plan);`, { window, plan: { entry: ['assets/initial.js'], common: ['assets/Shell.js'],
    application: { file: 'assets/app.js', styles: ['assets/app.css'], initializers: [] },
    pages: [{ pattern: '^/login$', files: ['assets/login.js'] }] }, URL, Request, Response, Headers, AbortController, Promise, Date, Intl });
  expect(document.querySelector('link[rel=modulepreload][href="/assets/login.js"]')).not.toBeNull();
  expect(document.querySelector('link[rel=modulepreload][href="/assets/app.js"]')).not.toBeNull();
  expect(document.querySelector('link[rel=preload][as=style][href="/assets/app.css"]')).not.toBeNull();
  window.__orchStartupReads?.painted();
  await vi.waitFor(() => expect(document.querySelector('link[href="/assets/login.js"]')).not.toBeNull());
  expect(document.querySelector('link[href="/assets/Shell.js"]')).toBeNull();
  expect(fetch).toHaveBeenCalledTimes(1);
});

it('preloads the frame without transferring another reader\'s whole English catalogue', () => {
  const bundle = {
    'assets/initial-aaa.js': chunk('assets/initial-aaa.js', { name: 'initial' }),
    'assets/shell-bbb.js': chunk('assets/shell-bbb.js', { name: 'shell', facadeModuleId: '/work/frontend/src/shell/Shell.tsx' }),
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
  // The emitted language pair shares an immutable address; the shell can derive the URL without a loader per feature.
  expect(feature).toBe(createHash('sha256').update(JSON.stringify(plan.parts.get(feature ?? '')?.words)).digest('hex'));
  for (const part of plan.parts.values()) expect(Object.keys(part.words.en)).toEqual(Object.keys(part.words['zh-CN']));
});
