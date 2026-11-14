/** Every thread-bound new placement sends its lifetime through the existing fork/switch builder. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as ForksApi from '@/api/forks';
import { initialChoices, requestOf } from '@/features/change/choices';
import { Where } from '@/features/change/Where';
import { i18n, setLocale } from '@/i18n';
import { creationHost } from '../unit/start-fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(), placesQuery: (id: string) => ({ queryKey: ['temporary-places', id], queryFn: () => Promise.resolve([creationHost()]) }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
afterEach(async () => { cleanup(); await setLocale('en'); });
function Placement({ result }: { result: 'new' | 'replace' }) {
  const [choices, setChoices] = useState(initialChoices(result));
  return <><Where thread={snapshot().thread} choices={choices} onChange={(patch) => setChoices((before) => ({ ...before, ...patch }))} fork={result === 'new'} />
    <output aria-label="Change request">{JSON.stringify(requestOf(choices))}</output></>;
}
for (const locale of ['en', 'zh-CN'] as const) for (const result of ['new', 'replace'] as const) it('offers Temporary only for a new environment and carries thread lifetime ('+locale+', '+result+')', async () => {
  await setLocale(locale); const user = userEvent.setup(); show(<Placement result={result} />);
  expect(screen.queryByRole('switch', { name: i18n.t('temporary.name') })).not.toBeInTheDocument();
  await user.click(screen.getByRole('combobox', { name: i18n.t('change.where.label') }));
  await user.click(screen.getByRole('option', { name: i18n.t('changeCompact.destination.new') }));
  const temporary = screen.getByRole('switch', { name: i18n.t('temporary.name') });
  expect(temporary).not.toBeChecked(); expect(temporary).toHaveAccessibleDescription(i18n.t('temporary.description'));
  await user.click(temporary);
  expect(screen.getByLabelText('Change request')).toHaveTextContent('"config":{"env.lifetime":"thread"}');
  await user.unhover(temporary); await user.hover(temporary);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('temporary.consent'));
  await user.click(temporary);
  expect(screen.getByLabelText('Change request')).not.toHaveTextContent('env.lifetime');
});
