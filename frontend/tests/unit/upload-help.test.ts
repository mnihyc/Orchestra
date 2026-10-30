// @vitest-environment node
/** Upload questions land on existing authored headings in both languages. */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';
import { marked } from 'marked';
import { readArticle } from '@/help/guide';
import { QUEUE_HELP, UPLOAD_HELP, uploadHelpHeading } from '@/help/uploads';

it.each(['en', 'zh-CN'] as const)('links every upload question and steering question to an authored heading in %s', (language) => {
  const directory = resolve(import.meta.dirname, '../../../docs/user', language === 'en' ? '.' : language);
  const article = readArticle('paste-and-upload', readFileSync(resolve(directory, 'paste-and-upload.md'), 'utf8'));
  expect(article.screens).toEqual(['/thread/$threadId', '/chat/$chatId', '/place-chat/$nodeId', '/topic/$topicId', '/project/$projectId', '/org/$slug', '/doc/$documentId', '/workbench', '/resources/environments/$id']);
  for (const topic of Object.keys(UPLOAD_HELP) as (keyof typeof UPLOAD_HELP)[]) {
    expect(article.headings.map((heading) => heading.id)).toContain(uploadHelpHeading(topic));
  }
  const queue = readArticle('message-queue', readFileSync(resolve(directory, 'message-queue.md'), 'utf8'));
  expect(queue.headings.map((heading) => heading.id)).toContain(QUEUE_HELP.heading);
  expect(marked.parse(article.body, { async: false })).not.toContain('**');
  expect(marked.parse(queue.body, { async: false })).not.toContain('**');
});
