/** The one-reader paste notice while reading, with a coded warning, and after a refusal, in both layouts and themes. */
import { expect, test } from '@playwright/test';

import { caretAtEndOf, DOCS, editorOf, openDocument, waitUntilSaved } from './documents-support';
import { scenario, signIn } from './support';

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`A paste keeps its reading, warning and refusal visible at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await openDocument(page, DOCS.plan);
    await caretAtEndOf(page, 'Do not enable before the migration');
    await page.keyboard.press('Enter');

    let release!: () => void;
    const reading = new Promise<void>((resolve) => { release = resolve; });
    let requests = 0;
    await page.route('**/api/v1/documents/convert', async (route) => {
      requests += 1;
      if (requests > 1) {
        await route.fulfill({ status: 403, contentType: 'application/problem+json', json: {
          type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
          title: 'Not permitted', detail: 'This document is no longer editable.', request_id: 'paste-refused',
        } });
        return;
      }
      await reading;
      const response = await route.fetch();
      const body = await response.json() as { warnings: unknown[] };
      await route.fulfill({ response, json: { ...body, warnings: [...body.warnings, {
        code: 'reader.image_missing', params: { path: 'missing.png' }, message: 'The English fallback must not replace a known catalogue sentence.',
      }] } });
    });
    const paste = (text: string) => page.evaluate((value) => {
      const clipboardData = new DataTransfer();
      clipboardData.setData('text/plain', value);
      document.querySelector('.doc-editor')?.dispatchEvent(new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true }));
    }, text);

    try {
      await paste('## Imported from the clipboard');
      const loading = page.getByRole('status').filter({ hasText: 'Reading what you pasted…' });
      await expect(loading).toBeVisible();
      await loading.scrollIntoViewIfNeeded();
      await page.screenshot({ path: info.outputPath(`formats-paste-reading-${width}-${theme}.png`), animations: 'disabled' });
    } finally {
      release();
    }
    const warning = page.getByRole('status').filter({ hasText: 'Picture not found: missing.png' });
    await expect(warning).toBeVisible();
    await expect(editorOf(page).locator('[data-ot="heading"]', { hasText: 'Imported from the clipboard' })).toBeVisible();
    await waitUntilSaved(page);
    await warning.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`formats-paste-warning-${width}-${theme}.png`), animations: 'disabled' });

    await caretAtEndOf(page, 'Ship the redirect fix');
    await paste('## This refused text must not be inserted');
    const refused = page.locator(`[data-document="${DOCS.plan}"]`).getByRole('alert');
    await expect(refused).toBeVisible();
    await expect(editorOf(page)).not.toContainText('This refused text must not be inserted');
    await refused.scrollIntoViewIfNeeded();
    await page.screenshot({ path: info.outputPath(`formats-paste-refused-${width}-${theme}.png`), animations: 'disabled' });
    expect(requests).toBe(2);
  });
}
