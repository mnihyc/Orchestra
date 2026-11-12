/** Parameter formats follow the actual recovery kind; completion calls retain supported originals. */
const completions = new Set(['openai-responses', 'anthropic-messages', 'openai-chat']);
export function completionFallback(protocols: readonly string[]): string | undefined {
  return protocols.find((protocol) => completions.has(protocol)) ?? protocols[0];
}
export function candidateProtocol(target: readonly string[], source: readonly string[] | undefined, purpose: string | undefined): string | undefined {
  if (purpose === 'agent') return 'native';
  if (purpose === undefined) return undefined;
  if (purpose === 'embedding') return target.includes('openai-embeddings') ? 'openai-embeddings' : undefined;
  if (target.length === 1) return target[0];
  const original = source?.[0];
  return original === undefined ? undefined : target.includes(original) ? original : completionFallback(target);
}
