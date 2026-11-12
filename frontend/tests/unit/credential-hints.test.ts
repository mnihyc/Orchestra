import { describe, expect, it } from 'vitest';
import { credentialProposal } from '@/features/resources/providers/credentialHints';
import type { ProviderCredentialHint, ProviderPreset } from '@/generated/api-types';

const preset = (name: string, hints: ProviderCredentialHint[] = []): ProviderPreset => ({
  name, note: null, docs_url: null, upstreams: [{ protocol: 'fixture-wire', base_url: 'https://model.example.test', options: {} }], credential_hints: hints,
});
const first = preset('First', [{ prefix: 'fixture-', role: 'model' }, { prefix: 'fixture-admin-', role: 'admin', field: 'admin_key' }]);

describe('declared credential-format proposals', () => {
  it('does not infer from an old preset or an unknown key and returns no key material', () => {
    expect(credentialProposal([preset('Old')], 'fixture-private-value')).toEqual({ kind: 'none' });
    expect(credentialProposal([first], 'unknown-private-value')).toEqual({ kind: 'none' });
    expect(credentialProposal([first], '  ')).toEqual({ kind: 'none' });
    const proposal = credentialProposal([first], 'fixture-private-value');
    expect(proposal.kind).toBe('proposed');
    expect(JSON.stringify(proposal)).not.toContain('private-value');
  });

  it('uses the most specific declared format without turning the role into a verification', () => {
    const result = credentialProposal([first], '  fixture-admin-private-value  ');
    expect(result).toEqual({ kind: 'proposed', preset: first, hint: first.credential_hints?.[1] });
    expect(result).not.toHaveProperty('verified');
    expect(result).not.toHaveProperty('capabilities');
  });

  it('leaves contradictory ties unchosen and ignores empty catch-all hints', () => {
    const second = preset('Second', [{ prefix: 'fixture-', role: 'model' }]);
    const contradictory = preset('First', [{ prefix: 'fixture-', role: 'admin', field: 'admin_key' }]);
    const empty = preset('Catch-all', [{ prefix: '', role: 'model' }]);
    expect(credentialProposal([first, second], 'fixture-private-value')).toEqual({ kind: 'ambiguous' });
    expect(credentialProposal([first, contradictory], 'fixture-private-value')).toEqual({ kind: 'ambiguous' });
    expect(credentialProposal([empty], 'unknown')).toEqual({ kind: 'none' });
  });

  it('accepts new declaration roles and a new provider without rewriting existing preset data', () => {
    const before = JSON.stringify(first);
    const added = preset('New extension', [{ prefix: 'new-format-', role: 'future-role', future: { kept: true } }]);
    expect(credentialProposal([first, added], 'fixture-private-value')).toEqual(credentialProposal([first], 'fixture-private-value'));
    expect(credentialProposal([first, added], 'new-format-private-value')).toMatchObject({ kind: 'proposed', hint: { role: 'future-role', future: { kept: true } } });
    expect(JSON.stringify(first)).toBe(before);
  });
});
