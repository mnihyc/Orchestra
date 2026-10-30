/** Editing and moving a waiting message preserves its attachments, quotes and stable identities. */
import { describe, expect, it } from 'vitest';
import { afterDropping, afterMoving, editableWords, nowDelivery, queuePreview, withEditedWords } from '@/features/composer/queue';
import type { ContentPart } from '@/generated/api-types';

const image: ContentPart = { type: 'image', blob: 'unchanged-image', name: 'screen.png', media_type: 'image/png' };
const quote: ContentPart = { type: 'forwarded', at: '2026-10-15T12:00:00Z', source_id: 'original', from: { kind: 'thread', thread: { kind: 'thread', id: 'source', name: 'Review', redacted: false } }, author: null,
  text: 'Quoted words\nAnother line', images: [{ blob: 'quote-image', media_type: 'image/png', name: 'quoted.png' }], attachments: [] };

describe('a waiting input', () => {
  it('edits its plain words once without flattening or dropping a quote, image or workspace file', () => {
    const file: ContentPart = { type: 'file', blob: null, name: 'context.ts', path: 'src/context.ts', size: 40 };
    const parts: ContentPart[] = [quote, { type: 'text', text: 'First' }, image, { type: 'text', text: 'Second' }, file];
    expect(editableWords(parts)).toBe('First\nSecond');
    expect(withEditedWords(parts, 'Replacement')).toEqual([quote, { type: 'text', text: 'Replacement' }, image, file]);
    expect(parts[1]).toEqual({ type: 'text', text: 'First' });
    expect(withEditedWords(parts, '')).toEqual([quote, image, file]);
    expect(withEditedWords([image], 'About this image')).toEqual([{ type: 'text', text: 'About this image' }, image]);
    expect(withEditedWords([{ type: 'text', text: 'Only words' }], '   ')).toEqual([]);
  });

  it('names attachment-only and forwarded inputs rather than leaving a blank or unexplained ellipsis', () => {
    expect(queuePreview([image])).toBe('screen.png');
    expect(queuePreview([quote])).toBe('Quoted words');
    expect(queuePreview([{ type: 'text', text: 'First line\nSecond line' }, image])).toBe('First line');
  });

  it('starts with meaningful words after blank lines or a script delimiter, retaining real shebangs and Markdown', () => {
    expect(queuePreview([{ type: 'text', text: '\n#!fake\r\n\r\nsay Check the redirect\nAnother line' }])).toBe('say Check the redirect');
    expect(queuePreview([{ ...quote, text: '\n#!fake\nForwarded question\nAnother line' }])).toBe('Forwarded question');
    expect(queuePreview([{ type: 'text', text: '#!fake\n  ' }, image])).toBe('screen.png');
    expect(queuePreview([{ type: 'text', text: '\n  ' }, { type: 'text', text: '\nNext part' }])).toBe('Next part');
    expect(queuePreview([{ type: 'text', text: '#!/bin/sh\nprintf hello' }])).toBe('#!/bin/sh');
    expect(queuePreview([{ type: 'text', text: '# Keep this heading\nDetails' }])).toBe('# Keep this heading');
  });

  it('targets the actual neighboring input ids for either direction, including the first and last slots', () => {
    const ids = ['first', 'middle', 'last'];
    expect(afterMoving(ids, 'middle', 0)).toBeNull();
    expect(afterMoving(ids, 'first', 1)).toBe('middle');
    expect(afterMoving(ids, 'first', 2)).toBe('last');
    expect(afterMoving(ids, 'last', 1)).toBe('first');
    for (const [id, target] of [['first', -1], ['last', 3], ['middle', 1], ['gone', 0]] as const) expect(afterMoving(ids, id, target)).toBeUndefined();
    expect(ids).toEqual(['first', 'middle', 'last']);
  });

  it('steers an active turn, offers interruption for known unsupported steering, and does not interrupt an idle thread', () => {
    expect(nowDelivery(true, false)).toBe('steer');
    expect(nowDelivery(true, true)).toBe('interrupt');
    expect(nowDelivery(false, true)).toBe('queue');
  });

  it('drops the picked input at the indicated line using the current ids, including rows that arrived during the drag', () => {
    const ids = ['beta', 'alpha', 'delta', 'gamma'];
    expect(afterDropping(ids, 'gamma', 'beta', 'before')).toBeNull();
    expect(afterDropping(ids, 'beta', 'gamma', 'after')).toBe('gamma');
    expect(afterDropping(ids, 'gamma', 'alpha', 'after')).toBe('alpha');
    expect(afterDropping(ids, 'gamma', 'delta', 'before')).toBe('alpha');
    expect(afterDropping(['new', ...ids], 'gamma', 'beta', 'before')).toBe('new');
    expect(ids).toEqual(['beta', 'alpha', 'delta', 'gamma']);
    for (const edge of ['before', 'after'] as const) {
      expect(afterDropping(ids, 'gone', 'beta', edge)).toBeUndefined();
      expect(afterDropping(ids, 'gamma', 'gone', edge)).toBeUndefined();
      expect(afterDropping(ids, 'gamma', 'gamma', edge)).toBeUndefined();
    }
    expect(afterDropping(ids, 'gamma', 'delta', 'after')).toBeUndefined();
    expect(afterDropping(ids, 'alpha', 'delta', 'before')).toBeUndefined();
  });
});
