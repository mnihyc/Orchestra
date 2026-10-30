/** Estimates follow the supplied model data and agree for text and stored paste counts. */
import { describe, expect, it } from 'vitest';
import { estimateTextTokens, estimateTokenCounts, modelTokenEstimate } from '@/lib/tokenEstimate';
import { ACME, MODEL_ESTIMATE } from '../doubles/fixtures';
import { World } from '../doubles/world';

const estimate = { ascii_prose_x100: 400, ascii_structured_x100: 200, other_x100: 200, image_tokens: 1000, measured: true };

describe('text token estimates', () => {
  it('uses the supplied ratios for each character class', () => {
    const text = 'abcd文😊';
    expect(estimateTextTokens(text, estimate)).toBe(2);
    expect(estimateTextTokens(text, { ...estimate, ascii_prose_x100: 200, other_x100: 100 })).toBe(4);
    expect(estimateTokenCounts(4, 2, estimate)).toBe(2);
    expect(modelTokenEstimate({ token_estimate: estimate })).toBe(estimate);
    expect(modelTokenEstimate({})).toBeNull();
  });

  it('keeps an estimate unavailable when model data or counts are invalid', () => {
    expect(estimateTextTokens('hello', null)).toBeNull();
    expect(estimateTextTokens('hello', { ...estimate, ascii_prose_x100: 0 })).toBeNull();
    expect(estimateTextTokens('hello', { ...estimate, other_x100: Number.NaN })).toBeNull();
    expect(estimateTokenCounts(-1, 0, estimate)).toBeNull();
    expect(estimateTokenCounts(1, 0.5, estimate)).toBeNull();
  });

  it('returns zero for empty text when the model supplies valid ratios', () => {
    expect(estimateTextTokens('', estimate)).toBe(0);
  });

  it('includes estimate data in the fake thread model options', () => {
    const world = new World(), person = world.people.get('ada');
    if (person === undefined) throw new Error('The model fixture needs a reader');
    const reply = world.threads.handle(person, 'GET', `/threads/${ACME.mainThread}/parameters`, new URLSearchParams(), {});
    expect(reply?.status).toBe(200);
    const options: unknown = expect.arrayContaining([expect.objectContaining(MODEL_ESTIMATE)]);
    expect(reply?.body).toMatchObject({ model: { options } });
  });
});
