/** Optional driver features remain ordinary unavailable choices with their reasons in both catalogues. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { DriverFeatures } from '@/features/resources/hosts/DriverFeatures';
import type { HostDriver } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const reasons = {
  host_paths: 'Node paths are not shared with this pool', build: 'This pool has no builder', archive: 'Image archives cannot be loaded',
  kept_files: 'Images are immutable', private_views: 'Storage classes have no private views', suspend: 'Units cannot be suspended',
  host_ports: 'The pool has no node-local listeners', live_mounts: 'Mounts change on replacement',
};
const driver: HostDriver = { name: 'cluster-test', title: 'Cluster test pool', creates: ['environment'], runtime_classes: [],
  availability: Object.fromEntries(Object.entries(reasons).map(([feature, reason]) => [feature, { supported: false, reason }])) };

afterEach(async () => { cleanup(); vi.restoreAllMocks(); document.documentElement.classList.remove('dark'); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows each missing cluster feature with its reason and closes on Escape (${locale}/${theme})`, async () => {
    await setLocale(locale); document.documentElement.classList.toggle('dark', theme === 'dark');
    const fetches = vi.spyOn(globalThis, 'fetch');
    const user = userEvent.setup(); show(<DriverFeatures driver={driver} />);
    const trigger = screen.getByRole('button', { name: i18n.t('hosts.driverFeatures.title') });
    trigger.focus(); await user.keyboard('{Enter}');
    for (const feature of Object.keys(reasons) as (keyof typeof reasons)[]) {
      const reason = reasons[feature];
      expect(screen.getByText(i18n.t(`hosts.driverFeatures.names.${feature}`))).toBeVisible();
      expect(screen.getByText(reason)).toBeVisible();
    }
    expect(screen.getAllByRole('img', { name: i18n.t('hosts.driverFeatures.unavailable') })).toHaveLength(Object.keys(reasons).length);
    expect(fetches).not.toHaveBeenCalled();
    await user.keyboard('{Escape}');
    expect(screen.queryByText(reasons.build)).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
}

it('shows an unknown observation as a dash and lets a touch-style click open available features', async () => {
  const { unmount } = show(<DriverFeatures driver={{ name: driver.name, title: driver.title, creates: driver.creates, runtime_classes: driver.runtime_classes }} />);
  expect(screen.getByText('–')).toBeVisible(); expect(screen.queryByRole('button')).not.toBeInTheDocument(); unmount();
  const user = userEvent.setup(); show(<DriverFeatures driver={{ ...driver, availability: { tools: { supported: true, reason: null } } }} />);
  await user.click(screen.getByRole('button', { name: i18n.t('hosts.driverFeatures.title') }));
  expect(screen.getByText(i18n.t('hosts.driverFeatures.names.tools'))).toBeVisible();
  expect(screen.getByText(i18n.t('hosts.driverFeatures.available'))).toBeVisible();
});
