/** Undo checks exact coordinates and bytes; it never fuzzes a patch over newer edits. */
import { expect, it } from 'vitest';
import { reverseDiff } from '@/lib/reverseDiff';

it('keeps later unrelated edits outside the recorded hunk', () => {
  const diff = '@@ -2 +2 @@\n-old\n+new\n';
  const inverse = reverseDiff('first\nnew\nlater edit\n', diff);
  expect(inverse?.text).toBe('first\nold\nlater edit\n');
  expect(inverse === null ? null : reverseDiff(inverse.text, inverse.diff)?.text).toBe('first\nnew\nlater edit\n');
  expect(reverseDiff('first\nchanged again\nlater edit\n', diff)).toBeNull();
});

it('applies multiple hunks backwards without shifting earlier coordinates', () => {
  const diff = '@@ -1,2 +1,3 @@\n a\n+x\n b\n@@ -4,2 +5,1 @@\n-d\n e\n';
  expect(reverseDiff('a\nx\nb\nc\ne\n', diff)?.text).toBe('a\nb\nc\nd\ne\n');
});

it('preserves no-newline markers and carriage returns exactly', () => {
  expect(reverseDiff('new', '@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n')?.text).toBe('old');
  expect(reverseDiff('new\r\nuntouched\r\n', '@@ -1 +1 @@\n-old\r\n+new\r\n')?.text).toBe('old\r\nuntouched\r\n');
  expect(reverseDiff('new', '@@ -1 +1 @@\n-old\n+new\n')).toBeNull();
});

it('handles zero-line sides without inventing a file operation', () => {
  expect(reverseDiff('', '@@ -1,1 +0,0 @@\n-old\n')?.text).toBe('old\n');
  expect(reverseDiff('new\n', '@@ -0,0 +1,1 @@\n+new\n')?.text).toBe('');
});

it.each([
  '@@ -1,2 +1,2 @@\n-old\n+new\n',
  '@@ -0,1 +1,1 @@\n-old\n+new\n',
  '@@ -2,1 +1,1 @@\n-old\n+new\n',
  '@@ -1 +1 @@\n old\n',
  'a cut or unavailable diff',
])('refuses incomplete, inconsistent and empty patches: %s', (diff) => {
  expect(reverseDiff('new\n', diff)).toBeNull();
});
