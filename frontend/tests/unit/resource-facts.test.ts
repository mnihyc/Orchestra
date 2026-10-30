/** Resource hints preserve permitted observations, configured thresholds, model groups and unknown measurements. */
import { describe, expect, it } from 'vitest';
import { environmentHint, hostHint, modelHint, providerHint, resourceHint } from '@/features/resource-facts/facts';
import type { CapStatus, ProviderChoiceUsage } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { detailEnvironment, detailHost } from '../component/resource-detail-fixtures';
import { planProvider } from './provider-plan-fixtures';
import { choiceCap } from './resource-facts-fixtures';

const now = Date.parse('2026-10-25T12:00:00Z');
const context = () => ({ t: i18n.t, locale: 'en-GB', timeZone: 'UTC', now });
const usage = (caps: CapStatus[]): ProviderChoiceUsage => ({ caps, performance: [], observed_at: new Date(now).toISOString() });

it('names an agent program from its descriptor without guessing from its kind', () => {
  const agent = { id: 'agent', name: 'Profile', harness_kind: 'research-agent', descriptor: { harness: { name: 'Research assistant' } } };
  expect(resourceHint({ kind: 'agent', agent }, context()).text).toBe('Research assistant');
  expect(resourceHint({ kind: 'agent', agent: { ...agent, descriptor: null } }, context()).text).toBe(i18n.t('resources.harnessNameUnknown'));
  const named = { ...agent, harness: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
  expect(resourceHint({ kind: 'agent', agent: named }, { ...context(), locale: 'zh-CN' }).text).toBe('研究助理');
  const translated = { ...agent, descriptor: { ...agent.descriptor, text: { 'zh-CN': { 'harness.name': '研究助理' } } } };
  expect(resourceHint({ kind: 'agent', agent: translated }, { ...context(), locale: 'zh-CN' }).text).toBe('研究助理');
});

describe('provider hints', () => {
  it('shows the binding window and never invents an alarm band', () => {
    const provider = planProvider();
    provider.facts.windows[1]!.resets_at = '2026-10-25T12:00:00Z';
    expect(providerHint(provider, context())).toEqual({ text: 'Weekly window (Fable) 81% · resets 12:00', warning: false });
    provider.facts.windows[1]!.used_percent = 123.4;
    expect(providerHint(provider, context())).toMatchObject({ text: 'Weekly window (Fable) 123.4% · resets 12:00', warning: false });
    provider.facts.windows[1]!.warning_percent = 90;
    expect(providerHint(provider, context()).warning).toBe(true);
  });
  it('shows a source refusal and its actual reset', () => {
    const provider = planProvider({ status: 'exhausted' });
    provider.facts.limit_reached = { type: 'window', resets_at: '2026-10-25T14:30:00Z' };
    expect(providerHint(provider, context())).toEqual({ text: 'At limit until 14:30', warning: true });
    provider.status = 'offline';
    expect(providerHint(provider, context())).toEqual({ text: 'Unavailable', warning: true });
  });
  it('uses balances and a binding running cap without inventing usage for private facts', () => {
    const provider = planProvider();
    provider.facts.windows = []; provider.facts_state.windows = 'empty';
    expect(providerHint(provider, context()).text).toBe('12.4 vendor credits left');
    provider.choice_usage = usage([choiceCap({ state: 'warning' })]);
    expect(providerHint(provider, context())).toEqual({ text: '3 running', warning: true });
    provider.access.permissions = ['provider.use'];
    expect(providerHint(provider, context())).toEqual({ text: 'Room not visible', warning: false });
  });
});

describe('model hints', () => {
  it('combines the context with existing performance and keeps a limit tied to its model group', () => {
    const cap = choiceCap({ state: 'reached', period_ends_at: '2026-10-25T14:30:00Z' });
    cap.cap.models = ['@frontier'];
    const subject = { kind: 'model' as const, model: { model_name: 'gateway/model-a', display_name: 'Model A', groups: ['frontier'], parameters: { context_window: 200_000 } }, providerId: 'provider', usage: usage([cap]), speed: 'fast' };
    expect(modelHint(subject, context())).toEqual({ text: 'At limit until 14:30', warning: true });
    expect(modelHint({ ...subject, model: { ...subject.model, groups: ['small'] } }, context())).toEqual({ text: '200K · fast', warning: false });
  });
  it('does not claim that an optional tier limit blocks an ordinary choice', () => {
    const cap = choiceCap({ state: 'reached' }); cap.cap.tiers = ['fast'];
    expect(modelHint({ kind: 'model', model: { model_name: 'm', display_name: 'M', parameters: {} }, providerId: 'p', usage: usage([cap]) }, context())).toEqual({ text: '', warning: false });
  });
});

describe('host and environment hints', () => {
  it('uses the server fullness decision and separates measured memory from declarations', () => {
    const host = { ...detailHost, usage: { cpu_percent_milli: 12_500, memory_used_mb: 4096, observed_at: new Date(now).toISOString() }, full: false };
    expect(hostHint({ kind: 'host', host }, context())).toEqual({ text: '1 running · 25% memory', warning: false });
    expect(hostHint({ kind: 'host', host: { ...host, full: true } }, context())).toEqual({ text: 'Full', warning: true });
    expect(hostHint({ kind: 'host', host: { ...host, machine: { ...host.machine, online: false }, offline_since: '2026-10-25T14:03:00Z' } }, context())).toEqual({ text: 'Offline since 14:03', warning: true });
  });
  it('shows activity, stopped idle time and actual failures without guessing a loss', () => {
    const environment = { ...detailEnvironment, activity: { running_threads: 2, idle_threads: 1, blocked_threads: 0, active_turns: 2, open_shells: 0 } };
    expect(environmentHint({ kind: 'environment', environment }, context())).toEqual({ text: '2 running · Build host', warning: false });
    expect(environmentHint({ kind: 'environment', environment: { ...environment, status: 'offline' } }, context())).toEqual({ text: 'Host offline', warning: true });
    const stopped = environmentHint({ kind: 'environment', environment: { ...environment, status: 'stopped', last_activity_at: '2026-10-25T09:00:00Z' } }, context());
    expect(stopped.warning).toBe(false); expect(stopped.text).toContain('Stopped'); expect(stopped.text).toContain('3');
    expect(environmentHint({ kind: 'environment', environment: { ...environment, status: 'failed' } }, context())).toEqual({ text: 'Failed', warning: true });
  });
});
