// @vitest-environment node
/** What a thread would be bound to, worked out from what the server says and what the person chose: what is shown as chosen is what is sent, what is missing is told, and nothing is put in place of a way that cannot be had. */
import { describe, expect, it } from 'vitest';

import { type Mode, NEW_ENVIRONMENT, NO_PICKS, type Picks, resolve, settingsToKeep } from '@/features/start/choices';
import type { Arrangement } from '@/generated/api-types';

import { agent, arrangement, environment, model, options, provider } from './start-fixtures';

const starter: Mode = { suggest: false };
const dialog: Mode = { suggest: true };
const picked = (more: Partial<Picks>): Picks => ({ ...NO_PICKS, ...more });

describe('failed environments are never a default or an implicit replacement', () => {
  const failed = () => { const row = environment('failed', { isDefault: true }); row.environment.status = 'failed'; return row; };
  it('rejects a stale usable/default flag on a failed environment', () => {
    const resolved = resolve(options({ environments: [failed()] }), NO_PICKS, dialog);
    expect(resolved.environment).toBeNull();
    expect(resolved.environments).toEqual([]);
    expect(resolved.fresh).toBe(true);
  });
  it('does not silently make another environment after an explicit location fails', () => {
    const resolved = resolve(options({ environments: [failed()] }), picked({ environment: 'failed' }), dialog);
    expect(resolved.environment).toBeNull();
    expect(resolved.fresh).toBe(false);
    expect(resolved.missing).toContainEqual({ kind: 'environment' });
  });
  it('does not reuse a failed remembered location or a different topic’s directory when creation is forbidden', () => {
    const resolved = resolve(options({ environments: [failed(), environment('elsewhere')], can_create: { allowed: false, missing: [] } }), NO_PICKS, { suggest: true, respectDefaults: true }, picked({ environment: 'failed' }));
    expect(resolved.environment).toBeNull();
    expect(resolved.fresh).toBe(false);
    expect(resolved.missing).toContainEqual({ kind: 'environment' });
  });
});

describe('defaults and the person’s last successful setup', () => {
  const quick: Mode = { suggest: true, respectDefaults: true };
  const last = picked({ provider: 'p2', agent: 'a2', model: 'm2', environment: 'e2', arrangement: 'key_separated' });
  const bare = () => options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false } });

  it('starts from the valid place defaults before considering the previous setup', () => {
    const answer = resolve(options(), NO_PICKS, quick, last);
    expect([answer.provider?.id, answer.agent?.id, answer.model?.model_name, answer.environment?.environment.id]).toEqual(['p1', 'a1', 'm1', 'e1']);
    expect(answer.arrangement).toBe('separated');
  });

  it('fills the primary choices together while respecting the current native placement preference', () => {
    const answer = resolve(bare(), NO_PICKS, quick, last);
    expect([answer.provider?.id, answer.agent?.id, answer.model?.model_name]).toEqual(['p2', 'a2', 'm2']);
    expect(answer.arrangement).toBe('separated');
    expect(answer.missing).toEqual([]);
    const changed = bare();
    changed.providers = [provider('p2', { agents: [agent('a2', { arrangements: [arrangement('separated', 1), arrangement('key_separated', 2, false, 'arrangement_not_allowed')] })] })];
    expect(resolve(changed, NO_PICKS, quick, last).arrangement).toBe('separated');
  });

  it('discards inaccessible providers, unreported agents, unlisted models and unsupported effort', () => {
    const answer = bare();
    answer.providers = [provider('p1'), provider('p2', { usable: false })];
    const resolved = resolve(answer, NO_PICKS, quick, { ...last, effort: 'not-offered' });
    expect([resolved.provider?.id, resolved.agent?.id, resolved.model?.model_name, resolved.effort]).toEqual(['p1', 'a1', 'm1', 'low']);
    answer.providers = [provider('p2', { agents: [agent('a2', { reason: 'harness_unreported' })], models: [{ ...model('m2'), allowed: false }] })];
    expect(resolve(answer, NO_PICKS, quick, last).missing.map((row) => row.kind)).toEqual(['agent', 'model']);
  });

  it('takes an explicit choice first and never replaces the place’s fresh-environment rule with an old directory', () => {
    const answer = resolve({ ...bare(), environment_default: 'fresh' }, picked({ provider: 'p1', agent: 'a1', model: 'm1' }), quick, last);
    expect(answer.provider?.id).toBe('p1');
    expect(answer.fresh).toBe(true);
    expect(answer.environment).toBeNull();
  });

  it('keeps an unavailable configured default visible as a refusal', () => {
    const answer = resolve(options({ providers: [provider('p1', { usable: false }), provider('p2')] }), NO_PICKS, quick);
    expect(answer.provider).toBeNull();
    expect(answer.missing[0]).toMatchObject({ kind: 'provider', defaultSet: true });
  });

  it('does not silently start in another topic’s directory when it cannot make its own', () => {
    const answer = resolve(options({ environments: [environment('another-topic')], can_create: { allowed: false, missing: [] } }), NO_PICKS, quick);
    expect(answer.environment).toBeNull();
    expect(answer.missing).toContainEqual({ kind: 'environment' });
  });
});

describe('what is chosen where the defaults of the place are whole', () => {
  it('is what the place defaults to, and nothing is missing', () => {
    const resolved = resolve(options(), NO_PICKS, starter);
    expect(resolved.provider?.id).toBe('p1');
    expect(resolved.agent?.id).toBe('a1');
    expect(resolved.model?.model_name).toBe('m1');
    expect(resolved.environment?.environment.id).toBe('e1');
    expect(resolved.missing).toEqual([]);
  });

  it('takes the effort the place defaults to where the model has it, and otherwise the one the model starts with', () => {
    const whole = options({ defaults: { provider_id: 'p1', agent_id: 'a1', model_name: 'm1', effort: 'high', locked: false } });
    expect(resolve(whole, NO_PICKS, starter).effort).toBe('high');
    expect(resolve(options(), NO_PICKS, starter).effort).toBe('low');
    expect(resolve(options(), picked({ model: 'm2' }), starter).effort).toBeNull();
  });

  it('puts a pick of the person above the default, and drops a pick that cannot be had', () => {
    expect(resolve(options(), picked({ provider: 'p2' }), starter).provider?.id).toBe('p2');
    const closed = options({ providers: [provider('p1'), provider('p2', { status: 'exhausted' })] });
    expect(resolve(closed, picked({ provider: 'p2' }), starter).provider?.id).toBe('p1');
  });
});

describe('what the starter asks for and a dialog suggests', () => {
  const bare = options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false } });

  it('asks for the provider first where none is set, and takes nothing the person did not choose', () => {
    const resolved = resolve(bare, NO_PICKS, starter);
    expect(resolved.provider).toBeNull();
    expect(resolved.missing).toEqual([{ kind: 'provider', unusable: null, defaultSet: false, none: false }]);
  });

  it('asks next for the profile and the model, one at a time, as each is chosen', () => {
    const afterProvider = resolve(bare, picked({ provider: 'p1' }), starter);
    expect(afterProvider.missing.map((entry) => entry.kind)).toEqual(['agent', 'model']);
    const afterAgent = resolve(bare, picked({ provider: 'p1', agent: 'a2' }), starter);
    expect(afterAgent.missing.map((entry) => entry.kind)).toEqual(['model']);
    const whole = resolve(bare, picked({ provider: 'p1', agent: 'a2', model: 'm2' }), starter);
    expect(whole.missing).toEqual([]);
  });

  it('has nothing to ask where a provider offers one profile and one model', () => {
    const single = options({
      defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false },
      providers: [provider('p1', { agents: [agent('only')], models: [model('one')] })],
    });
    const resolved = resolve(single, picked({ provider: 'p1' }), starter);
    expect(resolved.agent?.id).toBe('only');
    expect(resolved.model?.model_name).toBe('one');
    expect(resolved.missing).toEqual([]);
  });

  it('is suggested the first of each by a dialog, where the place says nothing', () => {
    const resolved = resolve(bare, NO_PICKS, dialog);
    expect([resolved.provider?.id, resolved.agent?.id, resolved.model?.model_name]).toEqual(['p1', 'a1', 'm1']);
    expect(resolved.missing).toEqual([]);
  });

  it('is told that a default provider that may not be used is not taken, and which it is', () => {
    const refused = options({ providers: [provider('p1', { usable: false, reason: 'no_use' }), provider('p2')] });
    const resolved = resolve(refused, NO_PICKS, starter);
    expect(resolved.provider).toBeNull();
    const [first] = resolved.missing;
    expect(first).toMatchObject({ kind: 'provider', defaultSet: true });
    expect(first?.kind === 'provider' ? first.unusable?.id : null).toBe('p1');
  });

  it('is told where there is no provider at all that the person could choose', () => {
    const none = options({ providers: [provider('p1', { usable: false, reason: 'no_use' })] });
    expect(resolve(none, NO_PICKS, dialog).missing).toEqual([{ kind: 'provider', unusable: expect.objectContaining({ id: 'p1' }) as unknown, defaultSet: true, none: true }]);
  });
});

describe('a harness that has not reported', () => {
  const waiting = options({ providers: [provider('p1', { agents: [agent('a1', { reason: 'harness_unreported' }), agent('a2', { reason: 'harness_unreported' })] })] });

  it('is waited for, and nothing that cannot run is offered', () => {
    const resolved = resolve(waiting, NO_PICKS, starter);
    expect(resolved.agent).toBeNull();
    expect(resolved.waiting).toBe(true);
    const [first] = resolved.missing;
    expect(first).toMatchObject({ kind: 'agent', unreported: true });
    expect(resolved.agents.filter((row) => row.startable)).toEqual([]);
    expect(resolve(waiting, NO_PICKS, dialog).agent).toBeNull();
  });

  it('is waited for even where another profile could be chosen meanwhile, which is not taken in its place and is not hidden', () => {
    const some = options({ providers: [provider('p1', { agents: [agent('a1', { reason: 'harness_unreported' }), agent('a2')] })] });
    const resolved = resolve(some, NO_PICKS, starter);
    expect(resolved.waiting).toBe(true);
    expect(resolved.agent).toBeNull();
    expect(resolved.agents.filter((row) => row.startable).map((row) => row.id)).toEqual(['a2']);
    // The person may choose it themself; a dialog suggests it, in view.
    expect(resolve(some, picked({ agent: 'a2' }), starter).agent?.id).toBe('a2');
    expect(resolve(some, NO_PICKS, dialog).agent?.id).toBe('a2');
  });

  it('is no reason to wait where the place names no profile and the one that has reported is the only one', () => {
    const only = options({
      defaults: { provider_id: 'p1', agent_id: null, model_name: 'm1', effort: null, locked: false },
      providers: [provider('p1', { agents: [agent('a1', { reason: 'harness_unreported' }), agent('a2')] })],
    });
    const resolved = resolve(only, NO_PICKS, starter);
    expect(resolved.agent?.id).toBe('a2');
    expect(resolved.waiting).toBe(false);
  });

  it('comes alive when the answer says it has reported', () => {
    const reported = options({ providers: [provider('p1')] });
    const resolved = resolve(reported, NO_PICKS, starter);
    expect(resolved.waiting).toBe(false);
    expect(resolved.agent?.id).toBe('a1');
    expect(resolved.missing).toEqual([]);
  });
});

describe('the arrangement', () => {
  const ranked = (...rows: Arrangement[]) => options({ providers: [provider('p1', { agents: [agent('a1', { arrangements: rows }), agent('a2')] }), provider('p2')] });

  it('that is ticked is the one the server ranks first among those that can be had, shown as the default', () => {
    const resolved = resolve(ranked(arrangement('key_separated', 1), arrangement('separated', 2)), NO_PICKS, starter);
    expect(resolved.defaultArrangement).toBe('key_separated');
    expect(resolved.arrangement).toBe('key_separated');
    expect(resolved.arrangements.map((row) => row.type)).toEqual(['key_separated', 'separated']);
  });

  it('is listed in the order the server ranked it, the ones that can be had first, whatever order they came in', () => {
    const resolved = resolve(ranked(arrangement('credential_shared', 3, false, 'arrangement_not_allowed'), arrangement('separated', 2), arrangement('key_separated', 1)), NO_PICKS, starter);
    expect(resolved.arrangements.map((row) => [row.type, row.open])).toEqual([
      ['key_separated', true],
      ['separated', true],
      ['credential_shared', false],
    ]);
  });

  it('that the person ticks is the one that is sent, the first row as much as another', () => {
    const rows = ranked(arrangement('separated', 1), arrangement('key_separated', 2));
    expect(resolve(rows, picked({ arrangement: 'key_separated' }), starter).arrangement).toBe('key_separated');
    expect(resolve(rows, picked({ arrangement: 'separated' }), starter).arrangement).toBe('separated');
  });

  it('that cannot be had is never put in the place of the person’s own: it stays closed with the server’s reason, and the default shows', () => {
    const rows = ranked(arrangement('separated', 1), arrangement('key_separated', 2), arrangement('credential_shared', 3, false, 'arrangement_not_allowed'));
    const resolved = resolve(rows, picked({ arrangement: 'credential_shared' }), starter);
    expect(resolved.arrangement).toBe('separated');
    const closed = resolved.arrangements.find((row) => row.type === 'credential_shared');
    expect(closed?.open).toBe(false);
    expect(closed?.entry.detail).toBe('arrangement_not_allowed detail');
  });

  it('does not offer a default profile where none of its arrangements can run, retaining its native reason', () => {
    const rows = ranked(arrangement('separated', 1, false, 'executor_missing'), arrangement('key_separated', 2, false, 'arrangement_not_allowed'));
    const resolved = resolve(rows, NO_PICKS, starter);
    expect(resolved.arrangement).toBeNull();
    expect(resolved.agent).toBeNull();
    expect(resolved.missing).toContainEqual(expect.objectContaining({ kind: 'agent' }));
    const closed = resolved.agents.find((row) => row.id === 'a1');
    expect(closed?.startable).toBe(false);
    expect(closed?.entry.arrangements[0]?.reason).toBe('executor_missing');
  });
});

describe('the environment', () => {
  it('is the default one of the place where the person may use it', () => {
    expect(resolve(options(), NO_PICKS, starter).environment?.environment.id).toBe('e1');
  });

  it('is a new one, made from the defaults of the place, where there is no default and the person may make one', () => {
    const none = options({ environments: [environment('e2')] });
    const resolved = resolve(none, NO_PICKS, starter);
    expect(resolved.fresh).toBe(true);
    expect(resolved.environment).toBeNull();
    expect(resolved.missing).toEqual([]);
  });

  it('is asked for where none can be used or made, and a dialog suggests the first that can be used', () => {
    const closed = options({ environments: [environment('e1', { isDefault: true, usable: false }), environment('e2')], can_create: { allowed: false, missing: [] } });
    expect(resolve(closed, NO_PICKS, starter).missing).toEqual([{ kind: 'environment' }]);
    expect(resolve(closed, NO_PICKS, dialog).environment?.environment.id).toBe('e2');
  });

  it('follows the person’s pick: one that exists, or a new one', () => {
    expect(resolve(options(), picked({ environment: 'e2' }), starter).environment?.environment.id).toBe('e2');
    const made = resolve(options(), picked({ environment: NEW_ENVIRONMENT }), starter);
    expect(made.fresh).toBe(true);
    expect(made.environment).toBeNull();
  });

  it('is waited for where the person chose one that was made a moment ago and the answer does not list it yet', () => {
    const resolved = resolve(options(), picked({ environment: 'just-made' }), dialog);
    expect(resolved.settling).toBe(true);
    expect(resolved.fresh).toBe(false);
    expect(resolved.environment).toBeNull();
    expect(resolved.missing.map((entry) => entry.kind)).not.toContain('environment');
  });

  it('is a new one for every thread where the place says each gets its own', () => {
    const each = options({ environment_default: 'fresh' });
    expect(resolve(each, NO_PICKS, starter).fresh).toBe(true);
  });
});

describe('what is kept as the default of a place', () => {
  it('is what the person chose and the place does not have already', () => {
    const bare = options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false } });
    const resolved = resolve(bare, picked({ provider: 'p1', agent: 'a2', model: 'm1' }), starter);
    expect(settingsToKeep(bare, resolved)).toEqual([
      { key: 'agent.default_provider_id', value: 'p1' },
      { key: 'agent.default_agent_id', value: 'a2' },
      { key: 'agent.model', value: 'm1' },
      { key: 'agent.effort', value: 'low' },
    ]);
  });

  it('is only the model where the place has the rest', () => {
    const noModel = options({ defaults: { provider_id: 'p1', agent_id: 'a1', model_name: null, effort: null, locked: false } });
    const resolved = resolve(noModel, picked({ model: 'm2' }), starter);
    expect(settingsToKeep(noModel, resolved)).toEqual([{ key: 'agent.model', value: 'm2' }]);
  });
});
