/** Public key-format hints propose existing presets; they neither verify nor retain the entered key. */
import type { ProviderCredentialHint, ProviderPreset } from '@/generated/api-types';

export type CredentialProposal = { kind: 'none' | 'ambiguous' }
  | { kind: 'proposed'; preset: ProviderPreset; hint: ProviderCredentialHint };

/** A longer declared prefix supplies more format information; a conflicting tie stays unchosen. */
export function credentialProposal(presets: readonly ProviderPreset[], entered: string): CredentialProposal {
  const key = entered.trim();
  if (key === '') return { kind: 'none' };
  let length = 0;
  const matches = new Map<string, { preset: ProviderPreset; hint: ProviderCredentialHint }>();
  for (const preset of presets) for (const hint of preset.credential_hints ?? []) {
    if (hint.prefix === '' || !key.startsWith(hint.prefix) || hint.prefix.length < length) continue;
    if (hint.prefix.length > length) { matches.clear(); length = hint.prefix.length; }
    matches.set(JSON.stringify([preset.name, hint.role, hint.field ?? null]), { preset, hint });
  }
  if (matches.size === 0) return { kind: 'none' };
  if (matches.size !== 1) return { kind: 'ambiguous' };
  const match = matches.values().next().value;
  return match === undefined ? { kind: 'none' } : { kind: 'proposed', ...match };
}
