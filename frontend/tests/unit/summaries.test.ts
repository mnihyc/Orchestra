/** A handoff summary is offered only when this conversation has a chosen producer. */
import { describe, expect, it } from 'vitest';
import { summaryConfigured } from '@/features/change/summary';

describe('choosing a summary handoff', () => {
  it('offers none while settings are loading, unset, cleared or malformed', () => {
    expect(summaryConfigured(undefined)).toBe(false);
    for (const value of [null, {}, { kind: '' }, ['model']]) expect(summaryConfigured([{ key: 'auto.summarizer', value }])).toBe(false);
  });
  it('allows a configured built-in or installed producer without assuming a model is the only kind', () => {
    expect(summaryConfigured([{ key: 'auto.summarizer', value: { kind: 'model', provider_id: 'provider', model_name: 'small' } }])).toBe(true);
    expect(summaryConfigured([{ key: 'auto.summarizer', value: { kind: 'rules' } }])).toBe(true);
  });
});
