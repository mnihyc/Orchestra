// @vitest-environment node
/** A new declaration must be documented, including nested fields and both authored languages. */
import { expect, it } from 'vitest';
import { readHelpContent } from '../../build/help-content';
import { checkReferenceCoverage, documentedReferences, requiredHelpReferences, schemaFieldPaths } from '@/help/referenceCoverage';
import { readArticle, type Guide } from '@/help/guide';

const content = readHelpContent();
it('explains stopping and session reopening facts in both shipped languages', () => {
  const references = [
    'state:StopState.stopping', 'state:StopState.stopped', 'state:BlockedReason.source_failed',
    'state:MarkerKind.session_reopened',
    ...['unknown', 'instructions', 'limits', 'configuration', 'mcp', 'tools', 'skills', 'program_reloads', 'side_history']
      .map((reason) => `state:SessionReopenReason.${reason}`),
    'option:ThreadInterruptBody.turn_id', 'option:ThreadStop.state', 'option:ThreadStop.requested_at',
    'option:ThreadStop.turn', 'option:Thread.stopping_since', 'option:Turn.interrupted_at',
    'option:MarkerSessionReopened.reason',
  ];
  for (const language of ['en', 'zh-CN'] as const) {
    const entries = content.guide.articles.filter((article) => content.userArticles.includes(article.id))
      .flatMap((article) => documentedReferences(language === 'en' ? article.body : article.translations?.[language]?.body ?? ''));
    for (const reference of references) {
      const entry = entries.find((entry) => entry.id === reference);
      expect(entry, `${language}:${reference}`).toBeDefined();
      expect(entry?.explanation.trim().length).toBeGreaterThan(0);
      if (language === 'zh-CN') expect(entry?.explanation).toMatch(/\p{Script=Han}/u);
    }
  }
});

it('covers every currently declared setting, nested option, state and API error in the shipped guide', () => {
  expect(content.references).toContain('setting:threads.offline_deadline');
  expect(content.references).toContain('option:env.after_up.*.command');
  expect(content.references).toContain('state:JobState.lost');
  expect(content.references).toContain('error:tunnel.gateway_unreachable');
  expect(content.references).toContain('setting:ask.max_tools');
  expect(content.references).toContain('state:AskState.closed');
  expect(content.references).toContain('state:UploadInventoryState.partial');
  expect(content.references).toContain('option:ModelGroupOptions.request_options');
  expect(content.references).toContain('option:usage.rates.*.web_fetch_per_call');
  expect(checkReferenceCoverage(content.guide, content.userArticles, content.references)).toEqual([]);
});

it.each(['setting', 'option', 'state', 'error'])('refuses one removed %s entry without reducing the inventory', (kind) => {
  const id = content.references.find((id) => id.startsWith(`${kind}:`));
  if (id === undefined) throw new Error(`Missing native ${kind} inventory`);
  const guide: Guide = { articles: content.guide.articles.map((article) => ({ ...article,
    body: article.body.split('\n').filter((line) => !line.startsWith(`| \`${id}\``)).join('\n') })) };
  expect(checkReferenceCoverage(guide, content.userArticles, content.references)).toContain(`reference.missing: en:${id}`);
});

it('rejects empty explanations, untranslated Chinese and rows hidden in code or comments', () => {
  const body = '# Reference\n\n| Reference | Meaning |\n| --- | --- |\n| `state:JobState.running` | |\n';
  const article = readArticle('reference', body);
  const guide: Guide = { articles: [{ ...article, translations: { 'zh-CN': { ...article, body: body.replace('| |', '| Running |') } } }] };
  const failures = checkReferenceCoverage(guide, ['reference'], ['state:JobState.running']);
  expect(failures).toContain('reference.explanation_missing: en:state:JobState.running');
  expect(failures).toContain('reference.explanation_missing: zh-CN:state:JobState.running');
  expect(documentedReferences(`\`\`\`md\n${body}\n\`\`\`\n<!--\n${body}\n-->`)).toEqual([]);
});

it('discovers additions and union/map fields from declarations rather than a count or vendor list', () => {
  const schema = { anyOf: [{ properties: { chosen: { properties: { nested: { type: 'string' } } } } }, { additionalProperties: { properties: { extra: { type: 'boolean' } } } }] };
  expect(schemaFieldPaths(schema)).toEqual(['chosen', 'chosen.nested', '*.extra']);
  const required = requiredHelpReferences([{ key: 'new.setting', schema }], { components: { schemas: {
    ChangeOptionsBody: { properties: { choice: { type: 'boolean' } } },
    NewState: { oneOf: [{ const: 'new_state' }, { type: 'object', properties: { needs_key: { type: 'object' } }, required: ['needs_key'] }] }, ErrorCode: { enum: ['new.error'] },
  } } }, { interfaces: { extension: { service: { options_schema: { properties: { new_field: {} } } } } } });
  expect(required).toEqual(expect.arrayContaining(['setting:new.setting', 'option:new.setting.chosen.nested', 'option:new.setting.*.extra',
    'option:ChangeOptionsBody.choice', 'state:NewState.new_state', 'state:NewState.needs_key', 'error:new.error', 'option:interfaces.extension.service.new_field']));
  expect(checkReferenceCoverage({ articles: [] }, [], required)).toContain('reference.missing: zh-CN:state:NewState.new_state');
});


it('does not count a comment as visible explanatory prose', () => {
  expect(documentedReferences('| Reference | Meaning |\n| --- | --- |\n| `state:JobState.running` | <!-- 隐藏说明 --> |\n'))
    .toEqual([{ id: 'state:JobState.running', explanation: '' }]);
});

it('discovers option declarations inside an extension array by their declared identity', () => {
  expect(requiredHelpReferences([], {}, { extensions: [{ id: 'extra-service', options_schema: { properties: { threshold: {} } } }] }))
    .toEqual(['option:extensions.extra-service.threshold']);
});


it('refuses a retired setting advertised as a current control', () => {
  for (const language of ['en', 'zh-CN'] as const) {
    const explanation = language === 'en' ? 'A removed control.' : '已移除的设置。';
    const table = `\n\n| Reference | Meaning |\n| --- | --- |\n| \`setting:retired.setting\` | ${explanation} |\n`;
    expect(documentedReferences(table)).toEqual([{ id: 'setting:retired.setting', explanation }]);
    const guide: Guide = { articles: content.guide.articles.map((article) => {
      if (article.id !== 'settings-reference') return article;
      if (language === 'en') return { ...article, body: article.body + table };
      const chinese = article.translations?.['zh-CN'];
      if (chinese === undefined) throw new Error('The Chinese settings reference is required.');
      return { ...article, translations: { ...article.translations, 'zh-CN': { ...chinese, body: chinese.body + table } } };
    }) };
    expect(checkReferenceCoverage(guide, content.userArticles, content.references))
      .toContain(`reference.setting_unknown: ${language}:setting:retired.setting`);
  }
});


it('documents view-opening choices and topic thread search in both shipped languages', () => {
  const article = content.guide.articles.find((entry) => entry.id === 'option-reference');
  if (article === undefined) throw new Error('The shipped option reference is required');
  for (const locale of ['en', 'zh-CN'] as const) {
    const body = locale === 'en' ? article.body : article.translations?.[locale]?.body;
    if (body === undefined) throw new Error('The translated option reference is required');
    expect(documentedReferences(body).map((entry) => entry.id)).toEqual(expect.arrayContaining([
      'option:PersonalState.sidebar.open_links', 'option:TopicSearch.q',
    ]));
  }
});
