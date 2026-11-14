/** Ask edits the native selected-interface schema using its own projection; Use never requires provider metadata Read. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { AskModelChoice } from '@/features/ask/ModelChoice';
import { effortDisplayName } from '@/features/models/names';
import type { AskModel, AskModelsReply } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { askModels } from '../unit/ask-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ models: vi.fn<(root: string) => Promise<AskModelsReply>>(), provider: vi.fn(), providerModels: vi.fn() }));
vi.mock('@/api/ask', () => ({ askModelsQuery: (root: string) => ({ queryKey: ['ask-native-schema', root], queryFn: () => calls.models(root) }) }));
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof Resources>();
  return { ...actual,
    providerQuery: (...args: Parameters<typeof actual.providerQuery>) => { calls.provider(...args); return actual.providerQuery(...args); },
    providerModelsQuery: (...args: Parameters<typeof actual.providerModelsQuery>) => { calls.providerModels(...args); return actual.providerModelsQuery(...args); },
  };
});
const appearance = useAppearance.getState();
const schema = { type: 'object', additionalProperties: false, properties: {
  reasoning: { type: 'object', properties: { effort: { type: 'string', enum: ['low', 'high'], title: 'Effort' } } },
  temperature: { type: 'number', minimum: 0, maximum: 1, title: 'Temperature' },
} };
function nativeChoices() {
  const reply = askModels(), first = reply.models[0];
  if (!first?.model) throw new Error('A native configured model is required.');
  return { ...reply, models: [{ ...first, request_option_schema: schema,
    model: { ...first.model, parameters: { efforts: ['low', 'high'], default_effort: 'low' },
      // This other interface is deliberately different; the client must use the resolved field.
      option_constraints: { 'another-interface': { type: 'object', properties: { wrong: { type: 'string', title: 'Wrong interface' } } } } },
  }] };
}
function Choice({ changed, value: initial = null }: { changed: (value: AskModel | null) => void; value?: AskModel | null }) {
  const [value, setValue] = useState(initial);
  return <AskModelChoice root="root" value={value} disabled={false} onChange={(next) => { setValue(next); changed(next); }} />;
}
beforeEach(() => { calls.models.mockReset().mockResolvedValue(nativeChoices()); calls.provider.mockClear(); calls.providerModels.mockClear(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`edits effort and the resolved constrained field without ProviderGet or ProviderModelList (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); const changed = vi.fn();
    show(<Choice changed={changed} />);
    // The effort has a control of its own beside the model; the request options stay closed at the foot of the model list.
    fireEvent.click(await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.effort.title'), value: effortDisplayName('low') }) }));
    fireEvent.click(await screen.findByRole('menuitemradio', { name: effortDisplayName('high') }));
    await waitFor(() => expect(changed).toHaveBeenLastCalledWith({ ...nativeChoices().default_model, params: { effort: 'high' } }));
    fireEvent.click(screen.getByRole('button', { name: i18n.t('ask.model') }));
    const picker = await screen.findByRole('dialog', { name: i18n.t('ask.model') });
    const temperature = await within(picker).findByRole('spinbutton', { name: 'Temperature' });
    expect(temperature).toHaveAttribute('min', '0'); expect(temperature).toHaveAttribute('max', '1');
    expect(within(picker).queryByRole('textbox', { name: 'Wrong interface' })).not.toBeInTheDocument();
    expect(within(picker).queryByRole('combobox', { name: 'Effort' })).not.toBeInTheDocument();
    fireEvent.change(temperature, { target: { value: '0.5' } });
    const identity = nativeChoices().default_model;
    await waitFor(() => expect(changed).toHaveBeenLastCalledWith({ ...identity, params: { effort: 'high', temperature: 0.5 } }));
    expect(calls.provider).not.toHaveBeenCalled(); expect(calls.providerModels).not.toHaveBeenCalled(); expect(calls.models).toHaveBeenCalledWith('root');
  });

  it(`does not invent a JSON form or provider read for an absent resolved schema (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); calls.models.mockResolvedValue(askModels());
    show(<Choice changed={vi.fn()} />); fireEvent.click(screen.getByRole('button', { name: i18n.t('ask.model') }));
    await screen.findByRole('menuitemradio', { name: 'Readable API model' });
    expect(screen.queryByRole('spinbutton', { name: 'Temperature' })).not.toBeInTheDocument();
    expect(screen.queryByRole('textbox', { name: /JSON/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: new RegExp(`^${i18n.t('composer.effort.title')}`) })).not.toBeInTheDocument();
    expect(calls.provider).not.toHaveBeenCalled(); expect(calls.providerModels).not.toHaveBeenCalled();
  });
}
