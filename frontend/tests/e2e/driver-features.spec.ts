/** A cluster's unavailable features remain readable on the shipping host page at both widths and themes. */
import { expect, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id as fixtureId, KIND } from '../doubles/fixtures';
import { scenario, signIn } from './support';

const reasons = {
  host_paths: 'Node paths are not shared with this pool', build: 'This pool has no builder', archive: 'Image archives cannot be loaded',
  kept_files: 'Images are immutable', private_views: 'Storage classes have no private views', suspend: 'Units cannot be suspended',
  host_ports: 'The pool has no node-local listeners', live_mounts: 'Mounts change on replacement', kept_file_transfer: 'Installed files are not transferred',
};

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a cluster declares features without another request (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    // Change the double's native projection: reads, inventories and live refreshes all agree.
    const installed = await fetch(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/resources/host-driver-facts`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: fixtureId(KIND.host, 1),
        capacity: { cpu_millis: 0, memory_mb: 0, disk_gb: 0 },
        drivers: [{ name: 'cluster-test', title: 'Cluster test pool', creates: ['environment', 'provider'], runtime_classes: [],
          availability: { ...Object.fromEntries(Object.entries(reasons).map(([feature, reason]) => [feature, { supported: false, reason }])),
            tools: { supported: true, reason: null }, gateway: { supported: true, reason: null } } }],
        storages: [{ name: 'durable', default: true, capacity_gb: 900, free_gb: 600, private_views: false }, { name: 'unmeasured', default: false, capacity_gb: 0, free_gb: 0, private_views: false }] }),
    });
    expect(installed.status).toBe(200);
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    const words = locale === 'en' ? english : chinese;
    const requests: string[] = [];
    page.on('request', (request) => { if (new URL(request.url()).pathname.startsWith('/api/v1/')) requests.push(`${request.method()} ${new URL(request.url()).pathname}`); });
    await page.goto(`/resources/hosts/${fixtureId(KIND.host, 1)}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.locator('summary').filter({ hasText: words.common.details }).click();
    await expect(page.getByRole('region', { name: words.hosts.capacity.title, exact: true }).getByText(words.hosts.capacity.unknown, { exact: true })).toHaveCount(3);
    for (const label of [words.hosts.capacity.cpu, words.hosts.capacity.memory, words.hosts.capacity.disk]) {
      const fact = page.locator('dt').filter({ hasText: label }).locator('..').locator('dd');
      await expect(fact).toContainText('–');
    }
    const button = page.getByRole('table', { name: words.hosts.drivers.title, exact: true }).getByRole('button', { name: words.hosts.driverFeatures.title, exact: true });
    await expect(button).toBeVisible();
    await button.scrollIntoViewIfNeeded();
    // Finish the shell's native reads before measuring the feature control's requests.
    await page.waitForLoadState('networkidle');
    const before = requests.length;
    await button.focus();
    await page.keyboard.press('Enter');
    const card = page.getByRole('dialog', { name: words.hosts.driverFeatures.title, exact: true });
    await expect(card).toBeVisible();
    for (const [feature, reason] of Object.entries(reasons)) {
      await expect(card.getByText(words.hosts.driverFeatures.names[feature as keyof typeof reasons], { exact: true })).toBeVisible();
      await expect(card.getByText(reason, { exact: true })).toBeVisible();
    }
    expect(requests.slice(before)).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const bounds = await card.boundingBox();
    expect(bounds).not.toBeNull();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    expect(bounds!.height).toBeLessThanOrEqual(900);
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(900);
    const screenshot = info.outputPath(`cluster-features-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, animations: 'disabled' });
    await info.attach('host features', { path: screenshot, contentType: 'image/png' });
    const requestLog = info.outputPath('requests.json');
    await writeFile(requestLog, JSON.stringify({ before, opening: requests.slice(before), requests }, null, 2));
    await info.attach('requests', { path: requestLog, contentType: 'application/json' });
    await page.keyboard.press('Escape');
    await expect(card).toHaveCount(0);
    await expect(button).toBeFocused();
    await button.click();
    await expect(card).toBeVisible();
    await page.keyboard.press('Escape');
    if (width === 1440) {
      await page.getByRole('tab', { name: words.resourceDetails.layout.storage, exact: true }).click();
      const row = page.getByRole('row').filter({ hasText: 'unmeasured' });
      await expect(row.getByRole('cell').nth(2)).toHaveText('–');
      await expect(row.getByRole('cell').nth(3)).toHaveText('–');
      await page.getByRole('tab', { name: words.hosts.tabs.overview, exact: true }).click();
      await page.locator('summary').filter({ hasText: words.common.details }).click();
    }
    await page.getByRole('region', { name: words.hosts.drivers.title, exact: true }).locator('[data-help-article="host-driver-features"]').click();
    const help = page.getByRole('dialog', { name: words.hosts.driverFeatures.title, exact: true });
    await help.getByRole('button', { name: words.help.hint.more, exact: true }).click();
    const guide = page.getByRole('region', { name: locale === 'en' ? 'Read a host’s driver features' : '查看主机驱动的功能', exact: true });
    await expect(guide.getByRole('heading', { level: 1 })).toHaveText(locale === 'en' ? 'Read a host’s driver features' : '查看主机驱动的功能');
    await expect(guide.getByRole('heading', { level: 2 }).first()).toHaveText(locale === 'en' ? 'Read the available operations' : '查看可用操作');
  });
}
