/** Native search choices come from the same wire declarations as outgoing request construction. */
import declarations from '../../../../shared/protocol-options.json';

interface Declaration { modes: Record<string, unknown> }
interface Registry {
  protocols: Record<string, { web_search?: Declaration }>;
  harnesses: Record<string, { web_search?: string[] }>;
}
const registry: Registry = declarations;

export function searchModesFor(provider: { protocols?: readonly string[]; harness_kind: string | null }): string[] {
  const native = provider.harness_kind === null ? undefined : registry.harnesses[provider.harness_kind]?.web_search;
  const modes = native ?? (provider.protocols ?? []).flatMap((protocol) => Object.keys(registry.protocols[protocol]?.web_search?.modes ?? {}));
  return [...new Set(modes)];
}
