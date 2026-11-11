/** Thread wire options use the existing schema controls, acknowledgement, locks and revision refusal. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { ParameterRow } from '@/features/inspector/Parameters';
import type { ParameterView, Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const key = 'provider_options:fixture-wire';
const access = { thread: { id: 'option-thread', owner: userRef(me(1, 'ada', 'Ada')) } as Pick<Thread, 'id' | 'owner'> };
function parameter(patch: Partial<ParameterView> = {}): ParameterView {
  return {
    key, apply: 'next_turn', value: { mode: 'a' }, source: { scope: 'thread', node: null }, locked_by: null, writable: true,
    def: { title: 'Fixture interface', type: 'object', group: 'provider', provider_interface: 'fixture-wire', provider_constraint: {
      type: 'object', additionalProperties: false, properties: {
        mode: { type: 'string', title: 'Mode', enum: ['a', 'b'] },
        region: { title: 'Region', const: 'local', 'x-orch-source': 'provider' },
      },
    } }, ...patch,
  };
}
function writer() {
  return { ready: true, write: vi.fn<(key: string, value: unknown) => Promise<void>>().mockResolvedValue(undefined),
    reset: vi.fn<(key: string) => Promise<void>>().mockResolvedValue(undefined), refuse: vi.fn<(error: unknown, key?: string) => Promise<void>>().mockResolvedValue(undefined) };
}
let locale = currentLocale();
let appearance = useAppearance.getState();
beforeEach(() => { locale = currentLocale(); appearance = useAppearance.getState(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale(locale); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('thread options %s/%s', (language, theme) => {
  beforeEach(async () => { await setLocale(language); useAppearance.setState({ locale: language, theme, resolvedTheme: theme }); });

  it('uses a list for an opened enum and a fixed field for the provider const', async () => {
    const runs = writer();
    show(<ul><ParameterRow parameter={parameter()} access={access} writable runs={runs} /></ul>);
    expect(screen.getByRole('textbox', { name: 'Region' })).toHaveValue('local');
    expect(screen.getByRole('textbox', { name: 'Region' })).toBeDisabled();
    expect(screen.getByText(i18n.t('providerOptions.fixedByProvider'))).toBeVisible();
    const user = userEvent.setup();
    await user.click(screen.getByRole('combobox', { name: 'Mode' }));
    await user.click(screen.getByRole('option', { name: 'b' }));
    await waitFor(() => expect(runs.write).toHaveBeenCalledExactlyOnceWith(key, { mode: 'b' }));
    await screen.findByText(i18n.t('inspector.parameters.saved'));
  });

  it('retains the chosen value while pending and announces success only after acknowledgement', async () => {
    const runs = writer();
    let complete: () => void = () => { throw new Error('the option write has not started'); };
    runs.write.mockImplementation(() => new Promise<void>((resolve) => { complete = resolve; }));
    show(<ul><ParameterRow parameter={parameter()} access={access} writable runs={runs} /></ul>);
    const user = userEvent.setup();
    await user.click(screen.getByRole('combobox', { name: 'Mode' }));
    await user.click(screen.getByRole('option', { name: 'b' }));
    const choice = screen.getByRole('combobox', { name: 'Mode' });
    await waitFor(() => expect(choice).toBeDisabled());
    expect(choice).toHaveValue('b');
    expect(screen.queryByText(i18n.t('inspector.parameters.saved'))).not.toBeInTheDocument();
    await user.click(choice);
    expect(runs.write).toHaveBeenCalledTimes(1);
    complete();
    await screen.findByText(i18n.t('inspector.parameters.saved'));
    expect(choice).toHaveValue('b');
  });

  it('keeps the schema draft after a real stale-revision refusal and a refreshed old value', async () => {
    const runs = writer();
    const error = new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Changed',
      detail: 'The setting changed.', status: 409, request_id: 'thread-option-stale' }, null);
    runs.write.mockRejectedValue(error);
    const draw = () => <ul><ParameterRow parameter={parameter()} access={access} writable runs={runs} /></ul>;
    const rendered = show(draw());
    const user = userEvent.setup();
    await user.click(screen.getByRole('combobox', { name: 'Mode' }));
    await user.click(screen.getByRole('option', { name: 'b' }));
    await waitFor(() => expect(runs.refuse).toHaveBeenCalledWith(error, key));
    await screen.findByText(i18n.t('errors.conflict.stale_revision'));
    rendered.rerender(draw());
    expect(screen.getByRole('combobox', { name: 'Mode' })).toHaveValue('b');
    expect(runs.write).toHaveBeenCalledTimes(1);
  });

  it('resets only this interface through the existing parameter reset', async () => {
    const runs = writer();
    show(<ul><ParameterRow parameter={parameter()} access={access} writable runs={runs} /></ul>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('inspector.parameters.reset', { name: 'Fixture interface' }) }));
    await waitFor(() => expect(runs.reset).toHaveBeenCalledExactlyOnceWith(key));
    expect(runs.write).not.toHaveBeenCalled();
  });

  it.each(['reader', 'locked'] as const)('keeps %s controls disabled without a write', async (kind) => {
    const runs = writer();
    const locked = kind === 'locked' ? { scope: 'topic', node: { id: 'topic-1', kind: 'topic', name: 'Plans', redacted: false } } as ParameterView['locked_by'] : null;
    show(<ul><ParameterRow parameter={parameter({ locked_by: locked })} access={access} writable={kind !== 'reader'} runs={runs} /></ul>);
    const choice = screen.getByRole('combobox', { name: 'Mode' });
    expect(choice).toBeDisabled();
    await userEvent.click(choice);
    expect(runs.write).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: i18n.t('inspector.parameters.reset', { name: 'Fixture interface' }) })).not.toBeInTheDocument();
  });
});
