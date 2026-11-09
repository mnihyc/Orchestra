/** The runtime chooser consumes configured usable entries, including personal parameter overrides. */
import { cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AskModelChoice } from '@/features/ask/ModelChoice';
import { effortDisplayName } from '@/features/models/names';
import type { AskModelsReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { ACME } from '../doubles/fixtures';
import { show } from './support';
let reply: AskModelsReply;
vi.mock('@/api/ask', () => ({ askModelsQuery: (root: string) => ({ queryKey: ['ask-model-test', root], queryFn: () => Promise.resolve(reply) }) }));
beforeEach(() => { reply = askModels(); });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the resolved default first and only usable configured entries (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), change = vi.fn();
    show(<AskModelChoice root={ACME.org} value={null} disabled={false} onChange={change} />);
    fireEvent.click(screen.getByRole('button', { name: t('ask.model') }));
    const picker = await screen.findByRole('dialog', { name: t('ask.model') });
    const rows = await within(picker).findAllByRole('menuitemradio');
    expect(rows[0]).toHaveTextContent(t('ask.defaultModel'));
    expect(rows[0]?.closest('li')).toHaveTextContent('Readable API model');
    expect(within(picker).queryByRole('menuitemradio', { name: /Excluded API model/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('ask.model') })).toHaveTextContent(`${t('ask.defaultModel')} · Readable API model`);
  });
  it(`chooses another configured entry with its declared effort (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), change = vi.fn(), configured = reply.models[0];
    if (!configured?.model || configured.provider === null) throw new Error('The native fixture needs its model/provider.');
    const second = { ...configured, selection: { provider_id: configured.provider.id, model: 'second' }, is_default: false,
      model: { ...configured.model, model_name: 'second', display_name: 'Second API model', parameters: { efforts: ['low', 'high'], default_effort: 'low' } } };
    reply = { ...reply, models: [...reply.models, second] };
    show(<AskModelChoice root={ACME.org} value={null} disabled={false} onChange={change} />);
    fireEvent.click(screen.getByRole('button', { name: t('ask.model') }));
    const group = await screen.findByRole('group', { name: t('composer.model.efforts', { model: 'Second API model' }) });
    fireEvent.click(within(group).getByRole('menuitemradio', { name: effortDisplayName('high', t) }));
    expect(change).toHaveBeenCalledWith({ ...second.selection, params: { effort: 'high' } });
  });
  it(`does not invent effort for a configured model without it (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), change = vi.fn();
    show(<AskModelChoice root={ACME.org} value={null} disabled={false} onChange={change} />);
    fireEvent.click(screen.getByRole('button', { name: t('ask.model') }));
    const row = await screen.findByRole('menuitemradio', { name: 'Readable API model' });
    expect(screen.queryByRole('group', { name: t('composer.model.efforts', { model: 'Readable API model' }) })).not.toBeInTheDocument();
    fireEvent.click(row); expect(change).toHaveBeenCalledWith(reply.default_model);
  });
}
