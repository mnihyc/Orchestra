import { expect, it } from 'vitest';
import { documentedHeaders, documentedOptions, suggestedRule } from '@/features/models/optionCatalogue';
import { candidateProtocol, completionFallback } from '@/features/models/candidateProtocol';

it('keeps request spellings and lifetime lists specific to the documented format', () => {
  expect(documentedOptions('openai-chat').find((field) => field.id === 'verbosity')?.path).toEqual(['verbosity']);
  expect(documentedOptions('openai-responses').find((field) => field.id === 'verbosity')?.path).toEqual(['text', 'verbosity']);
  expect(documentedOptions('anthropic-messages').find((field) => field.id === 'tier')?.values).toEqual(['auto', 'standard_only']);
  expect(documentedOptions('anthropic-messages').find((field) => field.id === 'cacheLifetime')?.values).toEqual(['5m', '1h']);
  expect(documentedOptions('a-custom-format')).toEqual([]);
  expect(documentedHeaders('anthropic-messages')).toContain('anthropic-beta');
  expect(documentedHeaders('a-custom-format')).toEqual([]);
});
it('never enables a paid start or writes identifying values through catalogue defaults', () => {
  for (const protocol of ['openai-chat', 'openai-responses', 'anthropic-messages']) for (const option of documentedOptions(protocol)) {
    const rule = suggestedRule(option);
    expect(rule).not.toHaveProperty('default'); expect(rule).not.toHaveProperty('const');
  }
});
it('retains a supported original completion protocol and uses the native fallback order otherwise', () => {
  const target = ['anthropic-messages', 'openai-responses'];
  expect(candidateProtocol(target, ['openai-responses'], 'summary')).toBe('openai-responses');
  expect(candidateProtocol(target, ['openai-chat'], 'summary')).toBe('anthropic-messages');
  expect(completionFallback(['custom-format', 'openai-chat', 'anthropic-messages'])).toBe('openai-chat');
  expect(candidateProtocol(target, undefined, 'agent')).toBe('native');
  expect(candidateProtocol(['openai-chat'], undefined, undefined)).toBeUndefined();
  expect(candidateProtocol(target, undefined, 'embedding')).toBeUndefined();
});
