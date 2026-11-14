/** A vendor family is a local draft in the existing editor, with an explicit native save. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentProps } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { ModelGroupProposal } from '@/features/resources/providers/ModelGroupProposal';
import type { ModelSuggestion, ProviderModel } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ save: vi.fn<typeof ResourcesApi.updateModel>() }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(), updateModel: calls.save }));
vi.mock('@/features/resources/providers/Caps', async (original) => ({ ...await original<Record<string, unknown>>(), ModelLimits: () => null }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const model: ProviderModel = { ...MODEL_ESTIMATE, id: 'family-model', model_name: 'family-model', display_name: 'Family model', enabled: true, source: 'manual', groups: ['kept'], parameters: { context_window: 272_000 } };
const provider = planProvider({ models: [model], model_groups_max: 8 });
const observed = (line = 'vendor-family'): ModelSuggestion => ({ model_name: model.model_name, display_name: model.display_name, listed: true,
  parameters: { offered: { report: { line } } }, origins: { '/parameters/offered/report/line': 'vendor' } });
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); calls.save.mockReset(); calls.save.mockResolvedValue(model); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale(locale); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('vendor family proposal %s/%s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });

  it('fills the actual group editor without writing and Cancel preserves the stored memberships', async () => {
    show(<ModelGroupProposal provider={provider} model={model} observed={observed()} manage />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('providerFill.groupProposal', { name: 'vendor-family' }) }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('models.editor.title') });
    const groups = within(dialog).getByRole('list', { name: i18n.t('models.groups.of') });
    expect(groups).toHaveTextContent('kept'); expect(groups).toHaveTextContent('vendor-family');
    expect(calls.save).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(model.groups).toEqual(['kept']);
  });

  it('saves only after explicit confirmation and keeps the captured draft when source data refreshes', async () => {
    const draw = (line: string) => <ModelGroupProposal provider={provider} model={model} observed={observed(line)} manage />;
    const rendered = show(draw('vendor-family'));
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('providerFill.groupProposal', { name: 'vendor-family' }) }));
    rendered.rerender(draw('new-family'));
    const dialog = screen.getByRole('dialog', { name: i18n.t('models.editor.title') });
    expect(within(dialog).getByRole('list', { name: i18n.t('models.groups.of') })).toHaveTextContent('vendor-family');
    expect(within(dialog).getByRole('list', { name: i18n.t('models.groups.of') })).not.toHaveTextContent('new-family');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('providers.models.save') }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith(provider.id, model.id, { parameters: { context_window: 272_000 }, groups: ['kept', 'vendor-family'] }));
  });

  it('keeps a lowered zero limit, while allowing removal of the proposal and unchanged old membership', async () => {
    show(<ModelGroupProposal provider={{ ...provider, model_groups_max: 0 }} model={model} observed={observed()} manage />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('providerFill.groupProposal', { name: 'vendor-family' }) }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('models.editor.title') });
    const save = within(dialog).getByRole('button', { name: i18n.t('providers.models.save') });
    expect(save).toHaveAttribute('aria-disabled', 'true');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('models.groups.remove', { name: 'vendor-family' }) }));
    await waitFor(() => expect(save).not.toHaveAttribute('aria-disabled', 'true'));
    expect(calls.save).not.toHaveBeenCalled();
  });

  it('has no reader action or duplicate membership and never invents a slug', () => {
    const draw = (props: ComponentProps<typeof ModelGroupProposal>) => <div data-testid="family-proposal"><ModelGroupProposal {...props} /></div>;
    const rendered = show(draw({ provider, model, observed: observed(), manage: false }));
    const proposal = () => within(screen.getByTestId('family-proposal'));
    expect(proposal().queryByRole('button')).not.toBeInTheDocument();
    rendered.rerender(draw({ provider, model: { ...model, groups: ['vendor-family'] }, observed: observed(), manage: true }));
    expect(proposal().queryByRole('button')).not.toBeInTheDocument();
    rendered.rerender(draw({ provider, model, observed: observed('A Vendor Family'), manage: true }));
    expect(proposal().queryByRole('button')).not.toBeInTheDocument();
  });
});
