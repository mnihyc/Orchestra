/** Allowed-value refusals and unknown context facts stay visible without inventing support. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ContextAvailability } from '@/features/composer/ContextAvailability';
import { statesOf, type SpeedOrAccess } from '@/features/composer/modelState';
import { AccessChoice, SpeedChoice } from '@/features/composer/SpeedAndAccess';
import type { ThreadParameters } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

function parameters(): ThreadParameters {
  const source = { scope: 'default' as const, node: null };
  return {
    provider: planProvider({ name: 'Readable provider' }),
    model: { value: 'sol', options: [{ ...MODEL_ESTIMATE, model_name: 'sol', display_name: 'Sol', allowed: true, reason: null,
      parameters: { service_tiers: [{ id: 'fast', label: 'Fast' }], default_service_tier: 'fast',
        access_programs: ['Daybreak Blue'], default_access_program: 'Daybreak Blue' } }], source, writable: true },
    effort: { value: null, source, supported: false, writable: true },
    parameters: [
      { key: 'service_tier', title: 'Speed', options: { from_model: 'service_tiers' }, resolved_options: [{ value: 'fast', label: 'Fast' }], provider_constraint: { enum: ['fast'] } },
      { key: 'cyber_access_program', title: 'Access', options: { from_model: 'access_programs' }, resolved_options: [{ value: 'Daybreak Blue', label: 'Daybreak Blue' }], provider_constraint: { enum: ['Daybreak Blue'] } },
    ].map((def) => ({ key: def.key, def: { ...def, allows_none: false, constraint_source: 'provider' }, value: null, source, locked_by: null, writable: true, apply: 'next_turn' })),
    approval_mode: 'bypass', approval_modes: ['ask', 'bypass'], approval_reviewer: null, subagent_limits: { limits: { at_once: 8, depth: 1 }, supported: [] },
  };
}
function choice(data: ThreadParameters, kind: SpeedOrAccess['kind']): SpeedOrAccess {
  const state = statesOf(data, {}).find((candidate): candidate is SpeedOrAccess => candidate.kind === kind);
  if (state === undefined) throw new Error('The fixture declares a usable speed or access choice');
  return state;
}
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) describe(`availability in ${locale}`, () => {
  const t = i18n.getFixedT(locale);
  beforeEach(async () => { await setLocale(locale); });
  it('keeps forbidden Standard and None disabled with the allowed-value source', async () => {
    const choose = vi.fn(); const data = parameters();
    const { unmount } = show(<SpeedChoice state={choice(data, 'speed')} writable chooser={null} rate={null} onChoose={choose} />);
    const standard = screen.getByRole('radio', { name: t('composer.model.speed.standard'), exact: true });
    expect(standard).toBeDisabled();
    await userEvent.hover(standard);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(t('composer.model.notAmongAllowedValues'));
    expect(standard).toHaveAccessibleDescription(expect.stringContaining(t('composer.model.notAmongAllowedValues')));
    expect(standard).toHaveAccessibleDescription(expect.stringContaining('Readable provider'));
    await userEvent.click(standard); expect(choose).not.toHaveBeenCalled(); unmount();
    show(<AccessChoice state={choice(data, 'access')} writable chooser={null} onChoose={choose} />);
    await userEvent.click(screen.getByRole('combobox'));
    const none = screen.getByRole('option', { name: t('composer.model.access.none'), exact: true });
    expect(none).toHaveAttribute('aria-disabled', 'true');
    expect(none).toHaveAccessibleDescription(expect.stringContaining(t('composer.model.notAmongAllowedValues')));
    expect(none).toHaveAccessibleDescription(expect.stringContaining('Readable provider'));
    await userEvent.click(none); expect(choose).not.toHaveBeenCalled();
  });
  it('does not invent a private restriction source and preserves a provider constant notice', async () => {
    const data = parameters(); data.provider = null;
    const state = choice(data, 'speed');
    expect(state.constraintProvider).toBeUndefined();
    const { unmount } = show(<SpeedChoice state={state} writable chooser={null} rate={null} onChoose={vi.fn()} />);
    const standard = screen.getByRole('radio', { name: t('composer.model.speed.standard'), exact: true });
    await userEvent.hover(standard);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(t('composer.model.notAmongAllowedValues'));
    expect(standard).toHaveAccessibleDescription(t('composer.model.notAmongAllowedValues'));
    expect(screen.queryByText('Readable provider')).not.toBeInTheDocument(); unmount();
    show(<SpeedChoice state={{ ...state, providerFixed: true }} writable chooser={null} rate={null} onChoose={vi.fn()} />);
    expect(screen.getByText(t('providerOptions.fixedByProvider'))).toBeVisible();
    expect(document.querySelector('[data-null-choice-restriction]')).toBeNull();
  });
  it('distinguishes missing context limits from no reported adjustment and restores a known range', () => {
    const data = parameters();
    const context = { key: 'context', def: { key: 'context', title: 'Context', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } } },
      value: null, source: { scope: 'default' as const, node: null }, locked_by: null, writable: true, apply: 'new_session' };
    data.parameters.push(context);
    const view = show(<ContextAvailability parameters={data} states={statesOf(data, {})} />);
    expect(screen.getByText(t('composer.model.context.title'))).toBeVisible();
    expect(screen.getByText(t('composer.model.context.unreportedLimits'))).toBeVisible();
    expect(within(view.container).queryByRole('spinbutton')).not.toBeInTheDocument();
    data.parameters = data.parameters.filter((parameter) => parameter.key !== 'context');
    view.rerender(<ContextAvailability parameters={data} states={statesOf(data, {})} />);
    expect(screen.getByText(t('composer.model.context.unreportedAdjustment'))).toBeVisible();
    data.parameters.push(context); data.model.options[0]!.parameters.context_max = 100_000;
    view.rerender(<ContextAvailability parameters={data} states={statesOf(data, {})} />);
    expect(document.querySelector('[data-context-availability]')).toBeNull();
    expect(statesOf(data, {}).find((state) => state.kind === 'context')).toMatchObject({ force: 100_000, largest: 100_000 });
  });
});
