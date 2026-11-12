/** Native group editors preserve other groups, drafts, ordinary errors and the current membership default. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import type * as ResourcesApi from '@/api/resources';
import { GroupsField, GroupStrip } from '@/features/resources/providers/Groups';
import type { ProviderModel, ProviderWriteReply } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type * as Notices from '@/ui/Toast';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ save: vi.fn<typeof ResourcesApi.updateProvider>(), dismiss: [] as (() => void)[] }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(), updateProvider: calls.save }));
vi.mock('@/ui/Toast', async (load) => {
  const actual = await load<typeof Notices>();
  return { ...actual, toast: (options: Parameters<typeof actual.toast>[0]) => {
    const dismiss = actual.toast(options);
    calls.dismiss.push(dismiss);
    return dismiss;
  } };
});

const model: ProviderModel = { ...MODEL_ESTIMATE, id: 'layer-model', model_name: 'layer-model', display_name: 'Layer model',
  enabled: true, source: 'manual', groups: ['frontier'], parameters: {} };
const original = { frontier: { request_options: { wire: { type: 'object', properties: { region: { const: 'local' } } } } },
  other: { request_options: { wire: { type: 'object', properties: { choice: { const: 'kept' } } } } } };
const provider = planProvider({ models: [model], model_groups_max: 9, model_group_options: original });
const reply = { ...provider, warning: null } as ProviderWriteReply;
const selected = { wire: { type: 'object', properties: { region: { enum: ['local', 'nearby'] } } } };
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); calls.save.mockReset(); });
afterEach(async () => {
  cleanup();
  for (const dismiss of calls.dismiss.splice(0)) dismiss();
  useAppearance.setState(appearance); await setLocale(locale);
});

function GroupCount({ maximum, start }: { maximum: number; start: string[] }) {
  const [groups, setGroups] = useState(start);
  return <GroupsField groups={groups} offered={[]} maximum={maximum} disabled={false} onChange={setGroups} />;
}

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('option layers in %s/%s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });

  it('opens the real group editor and saves only that group while retaining the other declarations', async () => {
    calls.save.mockResolvedValue(reply);
    show(<GroupStrip provider={provider} models={provider.models} manage options={original} revision={provider.revision} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('models.groups.options') }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('models.groups.optionsTitle', { name: 'frontier' }) });
    const field = within(dialog).getByRole('textbox', { name: i18n.t('providerOptions.schemaTitle') });
    expect(field).toHaveValue(JSON.stringify(original.frontier.request_options, null, 2));
    const guide = within(dialog).getByRole('button', { name: i18n.t('models.groups.optionsGuide') });
    expect(guide).toHaveAttribute('data-help-heading', 'request-options');
    await user.clear(field); await user.paste(JSON.stringify(selected));
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledExactlyOnceWith(provider.id, {
      expected_revision: provider.revision, model_group_options: { ...original, frontier: { request_options: selected } },
    }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(screen.getByText(i18n.t('models.groups.optionsSaved', { name: 'frontier' }))).toBeVisible();
  });

  it('keeps typed JSON after refusal and refresh, preserves other groups and refuses malformed JSON locally', async () => {
    calls.save.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Changed',
      detail: 'This provider changed.', status: 409, request_id: 'group-option-stale' }, null));
    calls.save.mockResolvedValue(reply);
    const draw = (revision: number) => <GroupStrip provider={provider} models={provider.models} manage options={original} revision={revision} />;
    const rendered = show(draw(provider.revision));
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('models.groups.options') }));
    const dialog = screen.getByRole('dialog');
    const field = within(dialog).getByRole('textbox', { name: i18n.t('providerOptions.schemaTitle') });
    await user.clear(field); await user.paste('{broken');
    expect(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') })).toHaveAttribute('aria-disabled', 'true');
    expect(calls.save).not.toHaveBeenCalled();
    await user.clear(field); await user.paste(JSON.stringify(selected));
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    await screen.findByText(i18n.t('resources.edit.changed'));
    expect(field).toHaveValue(JSON.stringify(selected));
    rendered.rerender(draw(provider.revision + 1));
    expect(field).toHaveValue(JSON.stringify(selected));
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(calls.save).toHaveBeenLastCalledWith(provider.id, {
      expected_revision: provider.revision + 1, model_group_options: { ...original, frontier: { request_options: selected } },
    }));
  });

  it('uses a raised membership default and leaves current memberships removable when it becomes zero', async () => {
    const start = ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight'];
    const rendered = show(<GroupCount maximum={9} start={start} />);
    const user = userEvent.setup();
    const field = screen.getByLabelText(i18n.t('models.groups.add'));
    await user.type(field, 'nine{Enter}');
    expect(screen.getByRole('list', { name: i18n.t('models.groups.of') })).toHaveTextContent('nine');
    expect(field).toBeDisabled();
    rendered.rerender(<GroupCount maximum={0} start={start} />);
    expect(screen.getByText(i18n.t('models.groups.off'))).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('models.groups.remove', { name: 'one' }) }));
    expect(screen.getByRole('list', { name: i18n.t('models.groups.of') })).not.toHaveTextContent('one');
  });

  it('keeps a real pending save busy and accepts no second press before its acknowledgement', async () => {
    let complete: (value: ProviderWriteReply) => void = () => { throw new Error('the native save has not started'); };
    calls.save.mockImplementation(() => new Promise<ProviderWriteReply>((resolve) => { complete = resolve; }));
    show(<GroupStrip provider={provider} models={provider.models} manage options={original} revision={provider.revision} />);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('models.groups.options') }));
    const dialog = screen.getByRole('dialog');
    const field = within(dialog).getByRole('textbox', { name: i18n.t('providerOptions.schemaTitle') });
    await user.clear(field); await user.paste(JSON.stringify(selected));
    const save = within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') });
    await user.click(save);
    await waitFor(() => expect(save).toHaveAttribute('aria-busy', 'true'));
    expect(field).toBeDisabled();
    await user.click(save);
    expect(calls.save).toHaveBeenCalledTimes(1);
    expect(dialog).toBeVisible();
    complete(reply);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('offers no group editor to a reader', () => {
    show(<GroupStrip provider={provider} models={provider.models} manage={false} options={original} revision={provider.revision} />);
    expect(screen.queryByRole('button', { name: i18n.t('models.groups.options') })).not.toBeInTheDocument();
  });
});
