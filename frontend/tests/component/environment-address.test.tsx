/** Both languages show the server's short address unchanged and say it can be used in the finder. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';

import { EnvironmentAddress } from '@/features/resources/environments/Address';
import { setLocale } from '@/i18n';

import { show } from './support';

afterEach(async () => { await setLocale('en'); });

for (const language of ['en','zh-CN'] as const) it(`keeps the canonical environment address and names its use (${language})`,async () => {
  await setLocale(language);
  const address='acme.lab.ada.acme-workspace';
  show(<EnvironmentAddress address={address} />);
  const hint=language==='en' ? `Short address: ${address}. Use it to find this environment.` : `短地址：${address}。可用它查找此环境。`;
  expect(screen.getByText(address)).toHaveAttribute('title',hint);
  expect(screen.queryByText('acme.lab.ada.Acme workspace')).not.toBeInTheDocument();
});
