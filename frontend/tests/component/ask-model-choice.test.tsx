/** Default follows the native first-usable entry; there is no choose-a-model placeholder state. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AskModelChoice } from '@/features/ask/ModelChoice';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { show } from './support';
vi.mock('@/api/ask', () => ({ askModelsQuery: (root: string) => ({ queryKey: ['ask-default-test', root], queryFn: () => Promise.resolve(askModels(root)) }) }));
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`resets a personal choice to the resolved native default (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale), onChange = vi.fn(), reply = askModels('root');
  if (reply.default_model === null) throw new Error('The native fixture needs its first usable default.');
  show(<AskModelChoice root="root" value={{ ...reply.default_model, params: { effort: 'high' } }} disabled={false} onChange={onChange} />);
  fireEvent.click(screen.getByRole('button', { name: t('ask.model') }));
  const current = await screen.findByRole('menuitemradio', { name: t('ask.defaultModel') });
  await waitFor(() => expect(current.closest('li')).toHaveTextContent('Readable API model')); fireEvent.click(current);
  expect(onChange).toHaveBeenCalledWith(null);
});

for (const locale of ['en', 'zh-CN'] as const) it(`names the saved choice before opening a read-only permanent composer (${locale})`, async () => {
  await setLocale(locale); const reply = askModels('root');
  if (reply.default_model === null) throw new Error('The native fixture needs its first usable default.');
  show(<AskModelChoice root="root" value={reply.default_model} disabled onChange={vi.fn()} />);
  const trigger = screen.getByRole('button', { name: i18n.t('ask.model') });
  await waitFor(() => expect(trigger).toHaveTextContent('Readable API model'));
  expect(trigger).toBeDisabled(); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

for (const locale of ['en', 'zh-CN'] as const) it(`resolves the Default caption before opening without saving a personal choice (${locale})`, async () => {
  await setLocale(locale); const change = vi.fn();
  show(<AskModelChoice root="root" value={null} disabled={false} onChange={change} />);
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('ask.model') })).toHaveTextContent(`${i18n.t('ask.defaultModel')} · Readable API model`));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(change).not.toHaveBeenCalled();
});
