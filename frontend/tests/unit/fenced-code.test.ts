/** Quoted hunks stay data, including literal fences and many backtick runs. */
import { expect, it } from 'vitest';
import { fencedCode } from '@/lib/fencedCode';
import { reviewMessage } from '@/features/source/words';
import { i18n } from '@/i18n';

it('uses a strictly longer fence without dropping any quoted code', () => {
  expect(fencedCode('plain', 'diff')).toBe('```diff\nplain\n```');
  expect(fencedCode('context\n ```\n+````')).toBe('`````\ncontext\n ```\n+````\n`````');
  const many = '` x '.repeat(200_000);
  expect(fencedCode(many)).toBe(`\`\`\`\n${many}\n\`\`\``);
});

it('keeps a recorded closing fence inside the review’s hunk, before the person’s actual words', () => {
  const hunk = '@@ -1 +1 @@\n ```\n+changed';
  const text = reviewMessage(i18n.t, [{ id: 'review', path: 'file.ts', side: 'new', from_line: 1, to_line: 1, hunk, diff_hash: 'recorded', item_id: null, text: 'My actual request' }]);
  expect(text).toContain(`\`\`\`\`diff\n${hunk}\n\`\`\`\`\n\nMy actual request`);
});
