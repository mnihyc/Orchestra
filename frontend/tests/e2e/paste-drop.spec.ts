/** Paste and drop use the built application at both widths and themes, with private bytes separated from workspace writes. */
import { createHash, randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { createInstance } from 'i18next';
import { expect, type APIRequestContext, type Page, test } from '@playwright/test';
import type { Draft, Job } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, filesPanel, MAIN, MAIN_ENVIRONMENT, openThread, showView } from './inspector-support';
import { PEOPLE } from './support';

interface Upload { name: string; size: number; media_type: string; bytes: Buffer }
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const fileRow = (page: Page, name: string | RegExp) => page.getByTestId('file-tree').getByRole('treeitem', { name });

async function language(request: APIRequestContext, locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } }); expect(login.ok()).toBe(true);
  const { access_token: token } = await login.json() as { access_token: string };
  const preference = await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } }); expect(preference.ok()).toBe(true);
  const i18n = createInstance(); await i18n.init({ lng: locale, interpolation: { escapeValue: false }, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
  return i18n.getFixedT(locale);
}

async function uploads(page: Page, holdNative = false) {
  const sessions = new Map<string, Upload>(), blobs = new Map<string, Upload>(), names = new Set<string>();
  let draft: Draft | null = null;
  const finished: unknown[] = [];
  let workspaceWrites = 0, agentInputs = 0, requests = 0;
  const native = new Map<string, { job: Job; status: number; result: unknown }>();
  let stopRequests = 0;
  const publish = async (job: Job) => {
    const response = await page.request.post(`${DOUBLE}/__double/job/snapshot`, { data: { job } });
    expect(response.ok()).toBe(true);
  };
  page.on('request', (request) => {
    if (request.url().includes('/api/v1/')) requests += 1;
    if (request.method() === 'POST' && /\/threads\/[^/]+\/inputs$/.test(new URL(request.url()).pathname)) agentInputs += 1;
  });
  await page.route('**/api/v1/uploads/**', async (route) => {
    const request = route.request(), url = new URL(request.url());
    if (request.method() === 'GET') {
      const hash = url.pathname.split('/').at(-1) ?? '', file = blobs.get(hash);
      if (file === undefined) throw new Error('The preview must name stored bytes');
      await route.fulfill({ contentType: file.media_type, body: file.bytes }); return;
    }
    if (request.method() === 'DELETE') { await route.fulfill({ status: 204 }); return; }
    if (url.pathname.endsWith('/sessions')) {
      const body = request.postDataJSON() as { name: string; size: number; media_type: string };
      const id = randomUUID(); sessions.set(id, { ...body, bytes: Buffer.alloc(0) });
      await route.fulfill({ json: { upload_id: id, chunk_bytes: 1024 * 1024, expires_at: '2099-01-01T00:00:00Z' } }); return;
    }
    const id = url.pathname.split('/')[5] ?? '', upload = sessions.get(id);
    if (upload === undefined) throw new Error('The selected upload must exist');
    if (request.method() === 'PUT') {
      expect(Number(url.searchParams.get('offset'))).toBe(upload.bytes.length);
      upload.bytes = Buffer.concat([upload.bytes, request.postDataBuffer() ?? Buffer.alloc(0)]);
      await route.fulfill({ json: { received: upload.bytes.length } }); return;
    }
    expect(upload.bytes.length).toBe(upload.size);
    const blob = createHash('sha256').update(upload.bytes).digest('hex'); blobs.set(blob, upload);
    await route.fulfill({ json: { blob, size: upload.size, name: upload.name, media_type: upload.media_type } });
  });
  await page.route(`**/api/v1/threads/${MAIN}/attachments`, async (route) => {
    const body = route.request().postDataJSON() as { blob: string; name: string; replace?: string };
    const file = blobs.get(body.blob); if (file === undefined) throw new Error('The attachment must name uploaded bytes');
    let name = body.replace?.split('/').at(-1) ?? body.name;
    if (body.replace === undefined) {
      const dot = name.lastIndexOf('.'), stem = dot > 0 ? name.slice(0, dot) : name, extension = dot > 0 ? name.slice(dot) : '';
      for (let suffix = 2; names.has(name); suffix += 1) name = `${stem} (${suffix})${extension}`;
    }
    names.add(name);
    await route.fulfill({ json: { path: `.orch/attachments/${name}`, name, size: file.size, blob: body.blob } });
  });
  await page.route(`**/api/v1/threads/${MAIN}/draft`, async (route) => {
    const request = route.request();
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    draft = request.method() === 'DELETE' ? null : await response.json() as Draft | null;
    await route.fulfill({ response });
  });
  await page.route(`**/api/v1/threads/${MAIN}/blobs/*`, async (route) => {
    const hash = new URL(route.request().url()).pathname.split('/').at(-1) ?? '';
    const file = blobs.get(hash);
    if (file === undefined) throw new Error('The preview must name stored bytes');
    await route.fulfill({ contentType: file.media_type, body: file.bytes });
  });
  await page.route('**/api/v1/environments/*/files/content?*', async (route) => {
    const request = route.request(), url = new URL(request.url()), hash = url.searchParams.get('blob');
    if (request.method() !== 'PUT' || hash === null) { await route.continue(); return; }
    const file = blobs.get(hash); if (file === undefined) throw new Error('A workspace write must name uploaded bytes');
    workspaceWrites += 1; url.searchParams.delete('blob');
    const response = await route.fetch({ url: url.toString(), postData: file.bytes, headers: { ...request.headers(), 'content-type': 'application/octet-stream' } });
    await route.fulfill({ response });
  });
  await page.route('**/api/v1/environments/*/files/uploads/*/write*', async (route) => {
    const request = route.request(), url = new URL(request.url()), id = url.pathname.split('/').at(-2) ?? '';
    const existing = native.get(id);
    if (request.method() === 'GET') {
      if (existing === undefined) throw new Error('The receipt belongs to an admitted write');
      await route.fulfill({ status: existing.status, json: existing.result }); return;
    }
    if (existing !== undefined) { await route.fulfill({ status: 202, json: existing.job }); return; }
    const body = request.postDataJSON() as { path: string; blob: string; expected_hash?: string };
    const file = blobs.get(body.blob); if (file === undefined) throw new Error('The write names completed uploaded bytes');
    const now = new Date(Date.now() - (holdNative ? 3_000 : 0)).toISOString();
    let status = 409, result: unknown = { type: 'orch:error:plan.blocked', code: 'plan.blocked', title: 'Stopped', status: 409, detail: 'No confirmed write', request_id: 'native-fixture' };
    if (!holdNative) {
      const target = new URL(request.url()); target.pathname = target.pathname.replace(/\/files\/uploads\/[^/]+\/write$/, '/files/content');
      target.searchParams.set('path', body.path); if (body.expected_hash !== undefined) target.searchParams.set('expected_hash', body.expected_hash);
      const headers: Record<string, string> = { ...request.headers(), 'content-type': 'application/octet-stream' }; delete headers['content-length'];
      const response = await route.fetch({ url: target.toString(), method: 'PUT', postData: file.bytes, headers });
      status = response.status(); result = await response.json();
      if (response.ok()) workspaceWrites += 1;
    }
    const state = holdNative ? 'running' : status < 400 ? 'succeeded' : 'failed';
    const job: Job = { id, kind: 'delivery', state, node_id: null, environment_id: MAIN_ENVIRONMENT, provider_id: null, batch_id: null,
      result: state === 'succeeded' ? result : null, error: state === 'failed' ? { code: 'conflict', phase: 'files', undone: true, message: 'The file changed' } : null,
      log: null, created_by: null, created_at: now, started_at: now, finished_at: holdNative ? null : now,
      progress: { phase: 'files', phases: [{ name: 'files', state: holdNative ? 'running' : 'done', weight: 100 }], percent: holdNative ? 49 : state === 'succeeded' ? 100 : 0,
        files_done: state === 'succeeded' ? 1 : 0, files_total: 1, bytes_done: holdNative ? Math.floor(file.size / 2) : state === 'succeeded' ? file.size : 0,
        bytes_total: file.size, rate_bps: null, eta_seconds: null, message: null,
        activity: { updated_at: new Date(Date.now() - 2_000).toISOString(), stall_after_seconds: 1, directories_done: null, stopping: false, waiting_for: null } } };
    native.set(id, { job, status, result });
    if (holdNative) await publish(job);
    await route.fulfill({ status: 202, json: job });
  });
  await page.route('**/api/v1/jobs/*/cancel', async (route) => {
    const id = new URL(route.request().url()).pathname.split('/').at(-2) ?? '', held = native.get(id);
    if (held === undefined) { await route.continue(); return; }
    stopRequests += 1;
    held.job = { ...held.job, progress: { ...held.job.progress, activity: { ...held.job.progress.activity!, stopping: true, waiting_for: { type: 'machine', name: 'upload-host' }, updated_at: new Date(Date.now() - 2_000).toISOString() } } };
    await publish(held.job); await route.fulfill({ json: held.job });
  });
  await page.route('**/api/v1/environments/*/files/uploads/*/finish*', async (route) => { finished.push(route.request().postDataJSON()); await route.fulfill({ status: 204 }); });
  return { get draft() { return draft; }, blobs, finished, counts: () => ({ workspaceWrites, agentInputs, requests, stopRequests }),
    acknowledgeStop: async () => { for (const held of native.values()) if (held.job.progress.activity?.stopping === true) {
      held.job = { ...held.job, state: 'cancelled', finished_at: new Date().toISOString(), progress: { ...held.job.progress, activity: { ...held.job.progress.activity, stopping: false } } };
      await publish(held.job);
    } } };
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`pasted drafts stay editable and private at ${width}px in ${theme}/${locale}`, async ({ page, request }, info) => {
    await begin(); const t = await language(request, locale, theme); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const held = await uploads(page); await openThread(page);
    const composer = page.getByRole('textbox', { name: t('composer.label') });
    const words = Array.from({ length: 35 }, (_, index) => `Line ${index + 1}: pasted words wrap in the actual composer`).join('\n');
    expect(words.length).toBeLessThan(8000);
    await composer.evaluate((element, text) => {
      const clipboardData = new DataTransfer(); clipboardData.setData('text/plain', text);
      element.dispatchEvent(new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData }));
    }, words);
    const chip = page.locator('[data-composer]').getByRole('button', { name: t('uploads.pastedText'), exact: true });
    await expect(chip).toBeVisible(); await expect(composer).toHaveValue('');
    await page.locator('[data-composer] input[type=file]').setInputFiles({ name: 'screenshot.png', mimeType: 'image/png', buffer: Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j9ZkAAAAASUVORK5CYII=', 'base64') });
    await expect(page.locator('[data-composer]').getByText(`→ .orch/attachments/screenshot.png · ${t('uploads.whenSent')}`)).toBeVisible();
    await chip.click();
    const editor = page.getByRole('textbox', { name: t('uploads.pastedText'), exact: true }); await expect(editor).toBeEditable();
    await editor.click(); await page.keyboard.press('Control+A'); await page.keyboard.type('Edited privately before sending');
    await expect.poll(() => {
      const paste = held.draft?.content.find((part) => part.type === 'pasted_text');
      return paste?.type === 'pasted_text' ? held.blobs.get(paste.blob)?.bytes.toString('utf8') : '';
    }).toBe('Edited privately before sending');
    await expect(editor).toBeVisible(); await expect(chip).toHaveAttribute('aria-expanded', 'true');
    expect(held.counts().workspaceWrites).toBe(0); expect(held.counts().agentInputs).toBe(0);
    await page.reload(); await expect(chip).toBeVisible(); await chip.click();
    await expect(editor).toContainText('Edited privately before sending');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    const labelBox = await chip.boundingBox();
    const removeBox = await page.locator('[data-composer]').getByRole('button', { name: t('uploads.remove', { name: t('uploads.pastedText') }), exact: true }).boundingBox();
    if (labelBox === null || removeBox === null) throw new Error('The paste label and its removal control must remain visible');
    expect(Math.min(labelBox.y + labelBox.height, removeBox.y + removeBox.height) - Math.max(labelBox.y, removeBox.y)).toBeGreaterThan(0);
    await page.screenshot({ path: info.outputPath(`paste-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    const helpOrigin = page.url();
    await page.locator('[data-composer] [data-help-control="upload-pasted-text"]').getByRole('button').click();
    await page.getByRole('button', { name: t('help.hint.more'), exact: true }).click();
    // The article's own title and section heading, as docs/user/paste-and-upload.md and its zh-CN edition write them.
    const authored = { en: { title: 'Paste and upload', heading: 'Pasted text' }, 'zh-CN': { title: '粘贴与上传', heading: '粘贴的文本' } }[locale];
    const helpArticle = page.getByRole('region', { name: t('views.help.title'), exact: true }).getByRole('region', { name: authored.title, exact: true });
    await expect(helpArticle.getByRole('heading', { name: authored.heading, exact: true })).toBeVisible();
    await expect(helpArticle).not.toContainText('**');
    await page.screenshot({ path: info.outputPath(`paste-help-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    const backToWork = page.locator('[data-help-reading]:visible').getByRole('button', { name: t('helpFlow.back'), exact: true });
    if (await backToWork.count() > 0) await backToWork.click();
    else if (page.url() !== helpOrigin) await page.goBack();
    await expect(composer).toBeVisible();
    await page.getByRole('button', { name: t('uploads.pasteAsText'), exact: true }).click();
    await expect(composer).toHaveValue('Edited privately before sending'); await expect(chip).toHaveCount(0);
    expect(held.counts().workspaceWrites).toBe(0); expect(held.counts().agentInputs).toBe(0);
    const counts = info.outputPath('request-counts.json');
    await writeFile(counts, JSON.stringify({ surface: 'paste', width, theme, locale, ...held.counts() }));
    await info.attach('request-counts', { path: counts, contentType: 'application/json' });
  });

  test(`folder drops name their count and viewers reject files at ${width}px in ${theme}/${locale}`, async ({ page, request }, info) => {
    await begin(); const t = await language(request, locale, theme); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const held = await uploads(page); await openThread(page); await showView(page, 'Files'); await fileRow(page, /^src/).click();
    const data = await page.evaluateHandle(() => {
      const data = new DataTransfer(); for (const name of ['screenshot.png', 'data.csv', 'notes.txt']) data.items.add(new File(['uploaded'], name)); return data;
    });
    await fileRow(page, /^src/).dispatchEvent('dragover', { dataTransfer: data });
    await expect(filesPanel(page).getByRole('status').filter({ hasText: t('uploads.target.folderCount', { count: 3, number: '3', path: 'web/src' }) })).toBeVisible();
    await page.screenshot({ path: info.outputPath(`drop-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await fileRow(page, /^src/).dispatchEvent('drop', { dataTransfer: data });
    await expect(fileRow(page, /^data\.csv/)).toBeVisible();
    await expect.poll(() => held.finished.length).toBe(1);
    expect(held.finished[0]).toMatchObject({ folder: 'web/src', outcome: 'complete' });
    expect(held.counts().workspaceWrites).toBe(3); expect(held.counts().agentInputs).toBe(0);
    await fileRow(page, /^app\.ts/).click();
    const code = page.getByRole('textbox', { name: /app\.ts/ }); await expect(code).toBeVisible();
    const before = await code.textContent(), url = page.url();
    await code.dispatchEvent('dragover', { dataTransfer: data }); await code.dispatchEvent('drop', { dataTransfer: data });
    await expect(code).toHaveText(before ?? ''); expect(page.url()).toBe(url); expect(held.counts().workspaceWrites).toBe(3);
    const reference = await page.evaluateHandle((environmentId) => {
      const data = new DataTransfer(); data.setData('application/x-orch-paths', JSON.stringify({ environmentId, paths: ['web/src/app.ts'] })); return data;
    }, MAIN_ENVIRONMENT);
    if (width === 400) {
      await page.getByRole('dialog', { name: 'web/src/app.ts', exact: true }).getByRole('button', { name: t('common.close'), exact: true }).click();
      await page.getByRole('button', { name: t('workbench.allTabs', { name: t('workbench.areas.main') }), exact: true }).click();
      await page.getByRole('option', { name: /Main thread/ }).click();
    } else await page.getByRole('tab').filter({ hasText: 'Main thread' }).click();
    await page.locator('[data-composer] textarea').dispatchEvent('drop', { dataTransfer: reference });
    await expect(page.getByRole('textbox', { name: t('composer.label') })).toHaveValue(new RegExp(`id:${MAIN_ENVIRONMENT}/web/src/app.ts`));
    expect(held.counts().workspaceWrites).toBe(3);
    const counts = info.outputPath('request-counts.json');
    await writeFile(counts, JSON.stringify({ surface: 'drop', width, theme, locale, ...held.counts() }));
    await info.attach('request-counts', { path: counts, contentType: 'application/json' });
  });

  test(`a dispatched Files copy waits for its stopped receipt at ${width}px in ${theme}/${locale}`, async ({ page, request }, info) => {
    await begin(); const t = await language(request, locale, theme);
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const held = await uploads(page, true);
    await openThread(page); await showView(page, 'Files'); await fileRow(page, /^src/).click();
    const data = await page.evaluateHandle(() => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([new Uint8Array(1024 * 1024)], 'native-copy.bin'));
      return transfer;
    });
    await fileRow(page, /^src/).dispatchEvent('drop', { dataTransfer: data });
    const selected = filesPanel(page).getByRole('list', { name: t('uploads.selected') });
    await expect(selected.getByText('native-copy.bin')).toBeVisible();
    await expect(selected.locator('[data-job-line]')).toContainText(t('liveWork.copying'));
    await expect(selected.locator('[data-job-stalled]')).toBeVisible();
    const nameBox = await selected.getByText('native-copy.bin').boundingBox();
    const cancelBox = await selected.getByRole('button', { name: t('uploads.cancel', { name: 'native-copy.bin' }) }).boundingBox();
    if (nameBox === null || cancelBox === null) throw new Error('The name and its stop control must remain visible');
    expect(Math.min(nameBox.y + nameBox.height, cancelBox.y + cancelBox.height) - Math.max(nameBox.y, cancelBox.y)).toBeGreaterThan(0);
    const copied = await selected.evaluate((element) => ({ client: element.clientWidth, scroll: element.scrollWidth }));
    expect(copied.scroll - copied.client).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath(`native-copy-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await selected.getByRole('button', { name: t('uploads.cancel', { name: 'native-copy.bin' }) }).click();
    await expect(selected.locator('[data-job-line]')).toHaveText(t('liveWork.stoppingOn', { machine: 'upload-host' }));
    expect(held.finished).toHaveLength(0);
    expect(held.counts().workspaceWrites).toBe(0);
    const stopping = await selected.evaluate((element) => ({ client: element.clientWidth, scroll: element.scrollWidth }));
    expect(stopping.scroll - stopping.client).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath(`native-stopping-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await held.acknowledgeStop();
    await expect(selected).toHaveCount(0);
    await expect.poll(() => held.finished.length).toBe(1);
    expect(held.finished[0]).toMatchObject({ outcome: 'cancelled' });
    expect(held.counts().stopRequests).toBe(1);
    expect(held.counts().workspaceWrites).toBe(0);
    await expect(fileRow(page, /^native-copy\.bin/)).toHaveCount(0);
    const counts = info.outputPath('request-counts.json');
    await writeFile(counts, JSON.stringify({ surface: 'native-upload-stop', width, theme, locale, ...held.counts() }));
    await info.attach('request-counts', { path: counts, contentType: 'application/json' });
  });
}
