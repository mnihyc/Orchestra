/** Pasted text uses the model's reported estimate and remaining-context boundary before and after draft sync. */
import { describe, expect, it } from 'vitest';
import type { Thread } from '@/generated/api-types';
import { estimatedPasteTokens, pasteContext, pasteFits, pastedPartFits, type PastedTextPart } from '@/lib/pastedText';

const usage = { context_window: 1000, context_used: 900 } as NonNullable<Thread['usage']>;
const paste: PastedTextPart = { type: 'pasted_text', blob: 'a'.repeat(64), name: 'pasted.txt', path: '.orch/attachments/pasted.txt', size: 100, characters: 100, ascii_characters: 100 };
const estimate = { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false };
const first = { ...estimate, ascii_prose_x100: 482, other_x100: 132, measured: true };
const second = { ...estimate, ascii_prose_x100: 331, other_x100: 86, measured: true };

describe('folded paste delivery', () => {
  it('treats both ends as defaults and lets each paste override either end', () => {
    expect(pastedPartFits({...paste,delivery:'inline'},null,'fake-1',0)).toBe(true);
    expect(pastedPartFits({...paste,delivery:'file'},usage,'fake-1',100)).toBe(false);
    expect(pastedPartFits({...paste,delivery:null},usage,'fake-1',100)).toBe(true);
    expect(pastedPartFits(paste,{...usage,context_used:1000},'fake-1',100)).toBe(true);
    expect(pasteFits('x'.repeat(100000),null,'fake-1',100)).toBe(true);
  });
  it('fits at the remaining-context boundary and becomes a file beyond it', () => {
    expect(estimatedPasteTokens('x'.repeat(100), estimate)).toBe(25);
    expect(pasteFits('x'.repeat(100), usage, estimate, 25)).toBe(true);
    expect(pasteFits('x'.repeat(101), usage, estimate, 25)).toBe(false);
    expect(pasteFits('x'.repeat(100), { ...usage, context_used: 901 }, estimate, 25)).toBe(false);
    expect(pasteFits('x', { ...usage, context_used: 1001 }, estimate, 25)).toBe(false);
  });
  it('does not invent a context size and uses the configured share', () => {
    expect(pasteFits('x', null, estimate, 25)).toBe(false);
    expect(pasteFits('x', usage, estimate, 0)).toBe(false);
    expect(pasteFits('', usage, estimate, 0)).toBe(false);
    expect(pastedPartFits({ ...paste, size: 0, characters: 0, ascii_characters: 0 }, usage, estimate, 0)).toBe(false);
    expect(pasteFits('x'.repeat(200), usage, estimate, 50)).toBe(true);
    expect(pasteFits('x', usage, null, 25)).toBe(false);
    expect(pastedPartFits(paste, usage, null, 25)).toBe(false);
  });
  it('uses the selected context fill instead of an older cumulative usage figure', () => {
    const current = pasteContext({ context_window: 1000, context_used: 100 }, { window: 1000, used: 901 });
    expect(pastedPartFits(paste, current, estimate, 25)).toBe(false);
    expect(pastedPartFits(paste, pasteContext(null, { window: 1000, used: 900 }), estimate, 25)).toBe(true);
    expect(pastedPartFits(paste, pasteContext(usage, null), estimate, 25)).toBe(true);
    expect(pastedPartFits(paste, null, estimate, 25, 1000)).toBe(false);
  });
  it('keeps the same decision from synchronized character counts without reading the file', () => {
    expect(pastedPartFits(paste, usage, estimate, 25)).toBe(true);
    expect(pastedPartFits(paste, { ...usage, context_used: 901 }, estimate, 25)).toBe(false);
    expect(pastedPartFits(paste, { ...usage, context_used: 901 }, estimate, 25, 200000)).toBe(false);
    expect(pastedPartFits(paste, usage, estimate, 25, 920)).toBe(false);
    expect(pastedPartFits(paste, usage, estimate, 25, null, 1)).toBe(false);
    const chinese = '文'.repeat(100);
    expect(pastedPartFits({ ...paste, size: 300, ascii_characters: 0 }, usage, first, 100)).toBe(pasteFits(chinese, usage, first, 100));
    const lower = estimatedPasteTokens(chinese, first), higher = estimatedPasteTokens(chinese, second);
    if (lower === null || higher === null) throw new Error('Both models have valid estimates');
    expect(higher).toBeGreaterThan(lower);
  });
});
