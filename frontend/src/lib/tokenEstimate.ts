/** Estimate prose from the model's reported token estimate. */

/** Counts stay numeric even when the text itself is no longer held. */
export interface CharacterCounts { ascii: number; other: number }

/** Count Unicode code points with constant auxiliary memory. */
export function countTextCharacters(text: string): CharacterCounts {
  let ascii = 0, other = 0;
  for (let at = 0; at < text.length; at += 1) {
    const code = text.charCodeAt(at);
    if (code < 128) ascii += 1;
    else {
      other += 1;
      const next = text.charCodeAt(at + 1);
      if (code >= 0xd800 && code <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) at += 1;
    }
  }
  return { ascii, other };
}

// A large pasted file yields between fixed-size chunks so cancel and editing remain responsive.
const TEXT_COUNT_CHUNK_CHARS = 32_768;

/** Count a file's text cooperatively, without a character array or an unbounded synchronous task. */
export async function countTextCharactersAsync(text: string, signal: AbortSignal): Promise<CharacterCounts> {
  let ascii = 0, other = 0;
  for (let at = 0; at < text.length;) {
    signal.throwIfAborted();
    let end = Math.min(text.length, at + TEXT_COUNT_CHUNK_CHARS);
    const last = text.charCodeAt(end - 1), next = text.charCodeAt(end);
    if (last >= 0xd800 && last <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) end += 1;
    const counts = countTextCharacters(text.slice(at, end));
    ascii += counts.ascii; other += counts.other; at = end;
    if (at < text.length) await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
  signal.throwIfAborted();
  return { ascii, other };
}

/** Read a model option's estimate without guessing from its routing name. */
export function modelTokenEstimate(model: unknown): unknown {
  return typeof model === 'object' && model !== null && 'token_estimate' in model ? model.token_estimate : null;
}

/** Estimate from saved character counts without reading the original text. */
export function estimateTokenCounts(ascii: number, other: number, estimate: unknown): number | null {
  if (!Number.isSafeInteger(ascii) || ascii < 0 || !Number.isSafeInteger(other) || other < 0
    || !Number.isSafeInteger(ascii * 100) || !Number.isSafeInteger(other * 100)
    || typeof estimate !== 'object' || estimate === null || !('ascii_prose_x100' in estimate) || !('other_x100' in estimate)) return null;
  const prose = estimate.ascii_prose_x100, rest = estimate.other_x100;
  if (typeof prose !== 'number' || !Number.isSafeInteger(prose) || prose <= 0
    || typeof rest !== 'number' || !Number.isSafeInteger(rest) || rest <= 0) return null;
  const value = Math.ceil(ascii * 100 / prose) + Math.ceil(other * 100 / rest);
  return Number.isSafeInteger(value) ? value : null;
}

/** Estimate from text, counting Unicode code points in the same way as stored paste counts. */
export function estimateTextTokens(text: string, estimate: unknown): number | null {
  const { ascii, other } = countTextCharacters(text);
  return estimateTokenCounts(ascii, other, estimate);
}
