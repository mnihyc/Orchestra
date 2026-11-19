/** A real browser typesets a two-page document with local fonts, math, an image and a diagram, then reads/searches that PDF through the workspace viewer with bounded canvases and released workers. */
import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';

import { zipFilesOf } from '../doubles/export.ts';
import { DOCS, openDocument } from './documents-support.ts';
import { begin, openThread, row, showView, workspace } from './inspector-support.ts';
import { signIn } from './support.ts';

test.use({ viewport: { width: 1600, height: 1000 } });

test('PDF typesetting and viewing use local assets, preserve the content, and release the worker on close', async ({ page, baseURL }) => {
  // Cold compiler/Chinese font downloads and the first WASM compile get two minutes.
  test.setTimeout(120_000);
  await begin();
  await page.addInitScript(() => {
    const workers = new Set<Worker>();
    const Native = window.Worker;
    window.Worker = class extends Native {
      constructor(url: string | URL, options?: WorkerOptions) { super(url, options); workers.add(this); }
      override terminate() { workers.delete(this); super.terminate(); }
    };
    Object.defineProperty(window, '__orchPdfWorkers', { value: () => workers.size });
  });
  await signIn(page);
  const origin = new URL(baseURL ?? page.url()).origin;
  const external: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (/^https?:$/.test(url.protocol) && url.origin !== origin) external.push(url.href);
  });
  await page.context().route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (/^https?:$/.test(url.protocol) && url.origin !== origin) await route.abort();
    else await route.continue();
  });
  const source = `#set document(title: "Offline 你好")
= Offline 你好
The words of Ada and a local image.
#table(columns: (1fr, 1fr), [Owner], [Action], [Ada], [Ship])
$x^2 + y^2 = z^2$
#image("images/pixel.png", width: 24pt)
// orch:diagram 00000000-0000-4000-8000-000000000001
#figure(raw(block: true, lang: "mermaid", "graph TD\\n  A[Begin]-->B[End]"), caption: [Diagram, as its Mermaid source])
#pagebreak()
= Second page
Needle only appears on this page.
`;
  const pixel = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==', 'base64');
  const zip = zipFilesOf([['main.typ', Buffer.from(source)], ['images/pixel.png', pixel]], true);
  await page.route(new RegExp(`/api/v1/documents/${DOCS.plan}/export`), async (route) => {
    expect(new URL(route.request().url()).searchParams.get('format')).toBe('typst');
    await route.fulfill({ contentType: 'application/zip', body: Buffer.from(zip), headers: { 'Content-Disposition': 'attachment; filename="Offline.zip"' } });
  });
  await openDocument(page, DOCS.plan);
  // Ordinary editing never initializes the PDF compiler or PDF.js worker.
  const workerCount = () => page.evaluate(() => (window as unknown as { __orchPdfWorkers: () => number }).__orchPdfWorkers());
  expect(await workerCount()).toBe(0);
  await page.locator('[data-screen="document"]').getByRole('button', { name: /^Actions for / }).click();
  await page.getByRole('menuitem', { name: 'Export…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Export Rollout plan' });
  await dialog.getByRole('radio', { name: /^PDF/ }).check();
  const [download] = await Promise.all([page.waitForEvent('download'), dialog.getByRole('button', { name: 'Export', exact: true }).click()]);
  expect(download.suggestedFilename()).toBe('Offline.pdf');
  const path = await download.path();
  const bytes = await readFile(path);
  expect(bytes.subarray(0, 5).toString()).toBe('%PDF-');
  await expect.poll(workerCount).toBe(0);

  await workspace('/put', { files: { 'web/offline.pdf': { base64: bytes.toString('base64') } } });
  await openThread(page);
  await showView(page, /^Files/);
  await row(page, /^offline\.pdf/).click();
  const viewer = page.locator('[data-viewer="pdf"]');
  await expect(viewer.locator('canvas')).toHaveCount(1);
  await expect(viewer.locator('.textLayer')).toContainText('Offline 你好');
  await expect(viewer.locator('.textLayer')).toContainText('Owner');
  await expect(viewer.getByText('Page 1 of 2', { exact: true })).toBeVisible();
  await viewer.getByRole('searchbox', { name: 'Find in PDF' }).fill('Needle');
  await viewer.getByRole('searchbox', { name: 'Find in PDF' }).press('Enter');
  await expect(viewer.getByRole('spinbutton', { name: 'Page number' })).toHaveValue('2');
  await expect(viewer.locator('.textLayer')).toContainText('Needle');
  await expect(viewer.getByRole('status')).toContainText('1 match');
  await viewer.getByRole('button', { name: 'Zoom in' }).click();
  await expect(viewer.locator('canvas')).toHaveCount(1);
  await expect.poll(() => viewer.locator('canvas').evaluate((canvas: HTMLCanvasElement) => canvas.width * canvas.height)).toBeGreaterThan(0);
  expect(await viewer.locator('canvas').evaluate((canvas: HTMLCanvasElement) => canvas.width * canvas.height)).toBeLessThanOrEqual(8_000_000);
  await page.getByRole('button', { name: 'Close offline.pdf', exact: true }).click();
  await expect(viewer).toHaveCount(0);
  await expect.poll(workerCount).toBe(0);
  expect(external).toEqual([]);
});
