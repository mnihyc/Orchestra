/** A forwarded part is identified by its wire discriminator, while an incidental source identifier does not change a text part. */
import { describe, expect, it } from 'vitest';

import type { ContentPart } from '@/generated/api-types';
import { isForwarded, wordsOf } from '@/lib/content';

describe('content discriminators', () => {
  it('keeps a forwarded message distinct from the text beside it without guessing from identifiers', () => {
    const forwarded: ContentPart = { type: 'forwarded', from: { kind: 'thread', thread: { id: 'thread', kind: 'thread', name: 'Review', redacted: false } }, source_id: 'message', author: null, at: '2026-10-14T10:00:00.000Z', text: 'Ready', images: [], attachments: [] };
    expect(isForwarded(forwarded)).toBe(true);
    expect(isForwarded({ type: 'text', text: 'Ready', source_id: 'incidental' })).toBe(false);
    expect(wordsOf([{ type: 'text', text: 'Please review' }, forwarded])).toBe('Please review');
  });
});
