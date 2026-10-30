/** People and agents use the same bounded file preview, viewer and saved presentation in both widths and languages. */
import { expect, test, type Page } from '@playwright/test';
import type { ContentPart, DocumentFile } from '../../src/generated/api-types';
import { userMessage, toolCall, turn, agentMessage } from '../doubles/thread-rows';
import { ADA } from '../doubles/thread-scenes';
import { id as fixtureId } from '../doubles/fixtures';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { FILE_GUIDE } from '../../src/help/files';
import { begin, MAIN } from './inspector-support';
import { emit, PEOPLE, signIn } from './support';
import { block, DOCS, held, waitUntilSaved } from './documents-support';

const PIXEL = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==';
const PHOTO = '1'.repeat(64);
const GENERATED = '2'.repeat(64);
const FILE = '3'.repeat(64);
const MOTION = '4'.repeat(64);
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function fileRoutes(page: Page, reads: string[]) {
  await page.route('**/api/v1/**/blobs/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() !== 'GET') { await route.continue(); return; }
    reads.push(`${url.pathname}${url.search}`);
    const hash = url.pathname.split('/').at(-1) ?? PHOTO;
    if (url.searchParams.get('preview') === 'true') {
      await route.fulfill({ json: { state: 'ready', hash, size: 70, media_type: hash === FILE ? 'text/plain' : hash === MOTION ? 'image/gif' : 'image/png', is_text: hash === FILE,
        image: hash === FILE ? null : `data:image/png;base64,${PIXEL}`, text: hash === FILE ? 'First line\nSecond line' : null,
        width: hash === FILE ? null : 1, height: hash === FILE ? null : 1, duration_seconds: null, entry_count: null, viewer_bytes: 64 * 1024 * 1024 } });
    } else {
      await route.fulfill({ body: hash === FILE ? 'First line\nSecond line' : Buffer.from(PIXEL, 'base64'), contentType: hash === FILE ? 'text/plain' : 'image/png', headers: { 'X-Content-Type-Options': 'nosniff' } });
    }
  });
}

test('reduced motion keeps the static preview until Play animation', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await begin();
  const reads: string[] = [];
  await fileRoutes(page, reads);
  await emit(MAIN, { items: [userMessage('motion-user', '', { author: ADA, parts: [
    { type: 'image', blob: MOTION, name: 'motion.gif', media_type: 'image/gif' },
  ] })] });
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await page.getByRole('button', { name: 'Open motion.gif', exact: true }).click();
  await expect(page.locator('[data-file-overlay] img')).toHaveAttribute('src', `data:image/png;base64,${PIXEL}`);
  expect(reads.filter((url) => !url.includes('preview=true'))).toHaveLength(0);
  await page.getByRole('button', { name: 'Play animation', exact: true }).click();
  await expect(page.locator('[data-file-overlay] img')).toHaveAttribute('src', /^blob:/);
  expect(reads.filter((url) => !url.includes('preview=true'))).toHaveLength(1);
});

async function appearance(theme: 'light' | 'dark', locale: 'en' | 'zh-CN') {
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ login: PEOPLE.ada.login, password: PEOPLE.ada.password }) });
  expect(login.ok).toBe(true);
  const { access_token: token } = await login.json() as { access_token: string };
  const saved = await fetch(`${DOUBLE}/api/v1/me`, { method: 'PATCH', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` }, body: JSON.stringify({ theme, locale }) });
  expect(saved.ok).toBe(true);
}

for (const locale of ['en', 'zh-CN'] as const) test(`file hints open their authored heading in ${locale}`, async ({ page }, info) => {
  await page.setViewportSize({ width: locale === 'en' ? 1440 : 400, height: 1000 });
  await begin();
  await appearance(locale === 'en' ? 'light' : 'dark', locale);
  const reads: string[] = [];
  await fileRoutes(page, reads);
  await emit(MAIN, { items: [userMessage('guide-file', '', { author: ADA, parts: [
    { type: 'image', blob: PHOTO, name: 'photo.png', media_type: 'image/png' },
  ] })] });
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await page.getByRole('button', { name: 'photo.png', exact: true }).click();
  const hint = page.locator('[data-file-card] button[data-help-article="files-and-attachments"]');
  await expect(hint).toHaveAttribute('data-help-heading', FILE_GUIDE.sections.previews[locale]);
  await hint.click();
  await page.getByRole('button', { name: (locale === 'en' ? english : chinese).help.hint.more, exact: true }).click();
  await expect(page.getByRole('heading', { name: locale === 'en' ? 'Previews and chip sizes' : '预览和标签大小', exact: true })).toBeVisible();
  await expect(page.getByText(locale === 'en' ? 'Everyone sees that saved choice.' : '其他人也会看到它。', { exact: false })).toBeVisible();
  expect(reads.every((url) => url.includes('preview=true'))).toBe(true);
  await page.screenshot({ path: info.outputPath(`file-help-${locale}.png`), animations: 'disabled' });
});

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`file previews at ${width}px in ${theme} and ${locale}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await begin();
    await appearance(theme, locale);
    const reads: string[] = [];
    await fileRoutes(page, reads);
    const at = '2026-10-25T00:00:00Z';
    const parts: ContentPart[] = [
      { type: 'text', text: 'Files for this task' },
      { type: 'image', blob: PHOTO, name: 'photo.png', media_type: 'image/png' },
      { type: 'file', blob: FILE, name: 'notes.txt', path: '.orch/uploads/notes.txt', size: 23 },
    ];
    await emit(MAIN, { items: [
      userMessage('file-user', '', { author: ADA, turn: 'file-turn', inputId: 'file-input', at, parts }),
      toolCall('file-image', { kind: 'image', blob: GENERATED, name: 'generated.png', media_type: 'image/png', caption: 'An image made by the agent' }, { title: 'Generated image', turn: 'file-turn', at }),
      agentMessage('file-answer', 'The files are ready.', { turn: 'file-turn', at }),
    ], turns: [turn('file-turn', { ordinal: 1, state: 'completed', inputId: 'file-input', startedAt: at, completedAt: at })] });
    await signIn(page);
    await page.goto(`/thread/${MAIN}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const openName = locale === 'en' ? 'Open photo.png' : '打开 photo.png';
    const thumb = page.getByRole('button', { name: openName, exact: true });
    await thumb.scrollIntoViewIfNeeded();
    await expect(thumb.getByRole('img', { name: 'photo.png' })).toBeVisible();
    const before = [...reads];
    expect(before.every((url) => url.includes('preview=true'))).toBe(true);
    expect(before.filter((url) => url.includes(PHOTO))).toHaveLength(1);
    const bounds = await thumb.boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds?.width).toBeLessThanOrEqual(192);
    expect(bounds?.height).toBeLessThanOrEqual(112);
    await page.screenshot({ path: info.outputPath(`files-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await thumb.click();
    const viewer = page.locator('[data-file-overlay] [data-viewer="image"]');
    await expect(viewer.getByRole('img', { name: 'photo.png' })).toBeVisible();
    expect(reads.filter((url) => !url.includes('preview=true'))).toHaveLength(1);
    await page.screenshot({ path: info.outputPath(`viewer-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(thumb).toBeFocused();
    const step = page.locator('[data-step="file-image"] [data-tool-row] button[aria-expanded]');
    await step.scrollIntoViewIfNeeded();
    if (await step.getAttribute('aria-expanded') !== 'true') await step.click();
    const generated = page.getByRole('button', { name: locale === 'en' ? 'Open generated.png' : '打开 generated.png', exact: true });
    await expect(generated.getByRole('img', { name: 'generated.png' })).toBeVisible();
    await page.screenshot({ path: info.outputPath(`agent-${width}-${theme}-${locale}.png`), animations: 'disabled' });
    await generated.click();
    await expect(page.locator('[data-file-overlay] [data-viewer="image"] img')).toHaveAttribute('alt', 'generated.png');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`a document stores Small, Medium and Large at ${width}px in ${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await begin();
    await appearance(theme, 'en');
    const file: DocumentFile = { id: fixtureId(10, 1), name: 'diagram.png', media_type: 'image/png', size: 70, is_text: false, added_by: ADA, added_at: '2026-10-25T00:00:00Z' };
    const response = await fetch(`${DOUBLE}/__double/documents/files`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ doc: DOCS.plan, files: [file] }) });
    expect(response.status).toBe(204);
    // The existing block's fixture id is reused, so this is an ordinary file block's saved choice.
    const state = await held(DOCS.plan);
    const image = state.blocks.find((one) => one.id === block(24));
    expect(image?.props.file_id).toBe(file.id);
    await page.route('**/api/v1/documents/*/files/*', async (route) => {
      if (new URL(route.request().url()).searchParams.get('preview') === 'true') await route.fulfill({ json: { state: 'ready', hash: PHOTO, size: 70, media_type: 'image/png', is_text: false,
        image: `data:image/png;base64,${PIXEL}`, text: null, width: 1, height: 1, duration_seconds: null, entry_count: null, viewer_bytes: 64 * 1024 * 1024 } });
      else await route.fulfill({ body: Buffer.from(PIXEL, 'base64'), contentType: 'image/png' });
    });
    let external = 0;
    await page.route('https://media.example.org/**', async (route) => { external++; await route.abort(); });
    await signIn(page);
    await page.goto(`/doc/${DOCS.plan}`);
    const externalLabel = page.getByText('media.example.org · demo.mp4', { exact: true });
    await expect(externalLabel).toHaveAttribute('title', 'https://media.example.org/demo.mp4');
    await expect(externalLabel).toHaveCSS('white-space', 'nowrap');
    await expect(externalLabel).toHaveCSS('text-overflow', 'ellipsis');
    await expect(page.getByRole('button', { name: 'Play from media.example.org', exact: true }).first()).toBeVisible();
    const figure = page.locator('[data-ot="image"]').first();
    await figure.scrollIntoViewIfNeeded();
    await expect(figure.locator('[data-chip-size="large"]')).toBeVisible();
    await expect(figure.getByRole('img', { name: 'diagram.png' })).toBeVisible();
    for (const display of ['small', 'medium', 'large'] as const) {
      await figure.locator('[data-chip-size]').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Show as', exact: true }).hover();
      await page.getByRole('menuitemradio', { name: display.charAt(0).toUpperCase() + display.slice(1), exact: true }).click();
      await waitUntilSaved(page);
      await expect.poll(async () => (await held(DOCS.plan)).blocks.find((one) => one.id === block(24))?.props.display).toBe(display);
      await page.reload();
      await figure.scrollIntoViewIfNeeded();
      await expect(figure.locator(`[data-chip-size="${display}"]`)).toBeVisible();
      await page.screenshot({ path: info.outputPath(`document-${width}-${display}.png`), animations: 'disabled' });
    }
    expect(external).toBe(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
