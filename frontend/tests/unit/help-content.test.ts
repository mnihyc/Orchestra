// @vitest-environment node
/** Shipped guidance refuses dead links, missing real controls and guides detached from the product. */
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { readHelpContent } from '../../build/help-content';
import { checkHelpContent } from '@/help/contentCheck';
import { FILE_GUIDE } from '@/help/files';
import { guide as runtimeGuide } from '@/help/library';

describe('the complete shipped help', () => {
  const content = readHelpContent();
  const body = (id: string, language: 'en' | 'zh-CN') => {
    const article = content.guide.articles.find((entry) => entry.id === id);
    const text = language === 'en' ? article?.body : article?.translations?.[language]?.body;
    if (text === undefined) throw new Error(`The ${language} article ${id} must ship.`);
    return text;
  };

  it.each(['en', 'zh-CN'] as const)('explains a missing session file beside its screen label and gives sub-agent limits once in %s', (language) => {
    const article = body('state-reference', language);
    const settings = body('settings-and-usage', language);
    const english = language === 'en';
    expect(article).toContain(english
      ? '[Session files removed](#state:NativeHistoryState.removed) | The program confirmed that its session files are gone. You can still read the conversation in Orch.'
      : '[会话文件已移除](#state:NativeHistoryState.removed) | 程序已确认会话文件不在了。仍可在 Orch 中阅读对话。');
    const section = settings.split(english ? '## Sub-agents' : '## 子智能体')[1]?.split('\n## ')[0] ?? '';
    expect(section.match(english ? /defaults to 8/g : /默认是 8 个/g)).toHaveLength(1);
    expect(section.match(english ? /defaults to 1/g : /默认是 1 层/g)).toHaveLength(1);
    expect(section).not.toMatch(/agents\.subagent|1 to 64|1 到 64/u);
  });

  it('qualifies directory support and external requests on the installation entry page', () => {
    const readme = readFileSync(new URL('../../../README.md', import.meta.url), 'utf8');
    expect(readme).toContain('directory environments require a Linux connector');
    expect(readme).toContain('Logs and metrics are kept locally');
    expect(readme).toContain('Model requests and notifications go to the destinations you configure');
    expect(readme).not.toContain('Nothing is sent to any third party');
  });

  it('describes the canonical node policy without advertising a retired configuration import', () => {
    const deployment = readFileSync(new URL('../../../docs/operator/deployment.md', import.meta.url), 'utf8');
    const hooks = readFileSync(new URL('../../../docs/operator/node-hooks.md', import.meta.url), 'utf8');
    expect(deployment).toContain('no previous node configuration is imported');
    expect(hooks).toContain('imports no values from the deployment file');
    expect(deployment).not.toContain('imported once');
  });

  it.each(['en', 'zh-CN'] as const)('explains explicit updates with Auto off and minimum-version recovery in %s', (language) => {
    const updates = body('updates', language);
    for (const text of language === 'en'
      ? ['Explicit Update remains available while Auto is off', 'does not enable automatic updates', 'without a normal connection']
      : ['关闭自动更新时仍可选择立即更新', '不会开启自动更新', '无需先建立普通连接']) expect(updates).toContain(text);
    expect(updates).not.toContain(language === 'en' ? 'refuses a web request' : '拒绝网页请求');
    const operator = readFileSync(new URL('../../../docs/operator/upgrades.md', import.meta.url), 'utf8');
    expect(operator).toContain('Explicit web Update remains available with Auto off');
  });

  it.each(['en', 'zh-CN'] as const)('explains model-add choices, separate options and a missing context window in %s', (language) => {
    const provider = body('provider-details', language);
    const add = body('add-a-provider', language);
    for (const text of language === 'en'
      ? ['Web search, cache lifetimes, reasoning modes, capacity tiers and request options need separate configuration', 'Larger contexts remain a per-thread choice', 'enter a window before saving']
      : ['网页搜索、缓存时长、推理模式、容量档位和请求选项需要单独配置', '更大的上下文仍由每个线程选择', '保存前填写窗口大小']) expect(provider).toContain(text);
    expect(add).toContain(language === 'en' ? 'an explicitly declared build' : '明确指定的构建');
    expect(add).toContain(language === 'en' ? 'Managed provider containers have no local policy to configure' : '受管提供方容器没有可编辑的本机策略');
  });

  it.each(['en', 'zh-CN'] as const)('states that very small Ask contexts can leave no room for a question in %s', (language) => {
    const ask = body('ask', language);
    expect(ask).toContain(language === 'en' ? 'a very small context can leave no room for any question' : '很小的上下文可能让任何问题都无法放入');
    expect(ask).not.toContain(language === 'en' ? 'No pair of settings can make every question fail' : '任何一组设置都不会让所有问题都失败');
  });

  it.each(['en', 'zh-CN'] as const)('describes separate conversations, continued summaries and workspace files in visible terms in %s', (language) => {
    expect(body('running-thread-views', language)).toContain(language === 'en' ? 'A side thread has its own conversation' : '侧线程有自己的对话');
    expect(body('summaries', language)).toContain(language === 'en' ? 'Search reads existing summaries' : '搜索读取已有摘要');
    expect(body('settings-reference', language)).not.toContain(language === 'en' ? 'Working files storage' : '工作文件的存储');
  });
  it('ships cleanup and zero guidance with the real headings used in both languages', () => {
    const cleanup = content.guide.articles.find((article) => article.id === 'storage-cleanup');
    expect(cleanup?.headings.map((heading) => heading.id)).toEqual(['unused-caches','kept-data','unused-releases','uninstall','storage-report','clean-up-kept-items','server-files']);
    expect(cleanup?.translations?.['zh-CN']?.headings.map((heading) => heading.id)).toEqual(['unused-caches','kept-data','unused-releases','uninstall','storage-report','clean-up-kept-items','server-files']);
    const numbers = content.guide.articles.find((article) => article.id === 'numeric-settings');
    expect(numbers?.headings[0]?.id).toBe('zero-values');
    expect(numbers?.translations?.['zh-CN']?.headings[0]?.id).toBe('zero-values');
    expect(numbers?.translations?.['zh-CN']?.headings[0]?.text).toBe('零值');
    expect(cleanup?.translations?.['zh-CN']?.headings.map((heading)=>heading.text)).toEqual(['未使用缓存','保留的数据','未使用发布','卸载','存储报告','清理保留内容','服务器文件']);
  });
  it('has all targets, links, screen routes, both languages and control-linked decision guides', () => {
    const failures=checkHelpContent(content);
    expect(failures,failures.join('\n')).toEqual([]);
    expect(content.tutorials).toHaveLength(14);
  });
  it('ships search actions and model choices in both languages at the hints addresses',()=>{
    const search=content.guide.articles.find((article)=>article.id==='finding-things');
    const semantic=content.guide.articles.find((article)=>article.id==='semantic-search');
    for(const article of [search,semantic]) expect(article?.translations?.['zh-CN']?.body).toMatch(/语义/);
    for(const heading of ['choose-embeddings','local-embeddings','index-states','disk-limit','change-models']){
      expect(semantic?.headings.map((entry)=>entry.id)).toContain(heading);
      expect(semantic?.translations?.['zh-CN']?.headings.map((entry)=>entry.id)).toContain(heading);
    }
    expect(search?.translations?.['zh-CN']?.headings.map((entry)=>entry.id)).toContain('words-and-similar-meaning');
  });
  it('fails when a real control is removed while its tutorial remains', () => {
    const sources = { ...content.sources, 'features/ports/Grants.tsx': 'export {};' };
    expect(checkHelpContent({ ...content, sources })).toContain('target.missing: grants:features/ports/Grants.tsx');
  });
  it('requires both the policy key supplier and the shared row’s real tutorial marker',()=>{
    const sources={...content.sources,'ui/SettingRow.tsx':'export {};'};
    expect(checkHelpContent({...content,sources})).toContain('target.missing: policy:ui/SettingRow.tsx');
    const supplier={...content.sources,'features/settings/SettingRow.tsx':'export {};'};
    expect(checkHelpContent({...content,sources:supplier})).toContain('target.missing: policy:features/settings/SettingRow.tsx');
  });
  it('ships both actual agent adapter contract assets, and refuses either missing asset', () => {
    const article=content.guide.articles.find((one)=>one.id==='agent-adapters');
    expect(article?.section).toBe('build-your-own');
    for (const contract of ['agent', 'agent-programs']) {
      const href=`../../../crates/contracts/${contract}/README.md`;
      expect(article?.files?.[href]?.endsWith(`/crates/contracts/${contract}/README.md`)).toBe(true);
      expect(runtimeGuide.articles.find((one)=>one.id==='agent-adapters')?.files?.[href]).toEqual(expect.any(String));
      const files=Object.fromEntries(Object.entries(article?.files ?? {}).filter(([path])=>path!==href));
      const guide={articles:content.guide.articles.map((one)=>one.id==='agent-adapters'?{...one,files}:one)};
      expect(checkHelpContent({...content,guide})).toContain('article.missing: readme');
    }
  });
  it('fails on an absent article and on an absent heading', () => {
    const guide = { articles: content.guide.articles.map((article) => article.id === 'how-approvals-work' ? { ...article, body: `${article.body}\n[missing](missing-guide.md)\n[missing heading](let-agents-open-ports.md#missing-heading)` } : article) };
    expect(checkHelpContent({ ...content, guide })).toContain('article.missing: missing-guide');
    expect(checkHelpContent({ ...content, guide })).toContain('heading.missing: let-agents-open-ports#missing-heading');
  });
  it('accepts an authored translated heading alias while refusing an absent translated heading', () => {
    const guide = { articles: content.guide.articles.map((article) => {
      if (article.id !== 'connect-a-machine') return article;
      const chinese = article.translations?.['zh-CN'];
      if (chinese === undefined) throw new Error('The Chinese machine guide is missing');
      return { ...article, translations: { ...article.translations, 'zh-CN': { ...chinese,
        body: `${chinese.body}\n[commands](connect-a-machine.md#3-执行命令等待机器出现)\n[absent](connect-a-machine.md#没有这个标题)`,
      } } };
    }) };
    const failures = checkHelpContent({ ...content, guide });
    expect(failures).not.toContain('heading.missing: connect-a-machine#3-执行命令等待机器出现');
    expect(failures).toContain('heading.missing: connect-a-machine#没有这个标题');
  });
  it('refuses a command or checksum help link whose authored Chinese heading is missing', () => {
    for (const id of ['connect-a-machine', 'agent-program-builds']) {
      const guide = { articles: content.guide.articles.map((article) => {
        if (article.id !== id) return article;
        const chinese = article.translations?.['zh-CN'];
        if (chinese === undefined) throw new Error(`Missing Chinese article: ${id}`);
        return { ...article, translations: { ...article.translations, 'zh-CN': { ...chinese, headings: [] } } };
      }) };
      expect(checkHelpContent({ ...content, guide }).some((failure) => failure.startsWith(`heading.missing: ${id}#`))).toBe(true);
    }
  });
  it('fails when a guide exists but no product control links to it', () => {
    const sources = Object.fromEntries(Object.entries(content.sources).map(([path, source]) => [path, path.startsWith('features/') && !path.startsWith('features/help/') ? source.replaceAll('<GuideLink guide="who-can-see-and-do-what"', '<GuideLink guide="how-approvals-work"').replaceAll("'who-can-see-and-do-what'", "'how-approvals-work'") : source]));
    expect(checkHelpContent({ ...content, sources })).toContain('guide.unlinked: who-can-see-and-do-what');
  });
  it('fails when a tutorial has lost one language or its read destination', () => {
    expect(checkHelpContent({ ...content, catalogues: [content.catalogues[0], {}] })).toContain('text.missing: 1:helpRelease.walks.ports.one');
    expect(checkHelpContent({ ...content, routes: content.routes.filter((route) => route.replace(/\/$/, '') !== '/resources/machines') })).toContain('screen.missing: connect-a-computer:/resources/machines');
  });
  it('fails when a current registry key lacks its Chinese title or description', () => {
    const failures = checkHelpContent({ ...content, catalogues: [content.catalogues[0], {}] });
    expect(failures).toContain('text.missing: 1:settingRegistry.agent.params.description');
    expect(failures).toContain('text.missing: 1:settingRegistry.ask.max_tools.description');
  });
  it('requires the authored file headings in both languages and their screen hints', () => {
    const file = content.guide.articles.find((article) => article.id === FILE_GUIDE.id);
    expect(file?.translations?.['zh-CN']?.body).toContain('显示为');
    expect(file?.screens).toEqual(expect.arrayContaining(['/thread/$threadId', '/doc/$documentId', '/search', '/inbox', '/workbench']));
    const chinese = file?.translations?.['zh-CN'];
    if (chinese === undefined) throw new Error('The authored Chinese file guide is missing');
    const guide = { articles: content.guide.articles.map((article) => article.id === FILE_GUIDE.id ? { ...article,
      translations: { ...article.translations, 'zh-CN': { ...chinese, headings: [] } },
    } : article) };
    expect(checkHelpContent({ ...content, guide })).toContain(`heading.missing: ${FILE_GUIDE.id}#${FILE_GUIDE.sections.previews['zh-CN']}`);
    expect(checkHelpContent({ ...content, sources: { ...content.sources, 'features/files/FileDetails.tsx': 'export {};' } }))
      .toContain(`guide.unlinked: ${FILE_GUIDE.id}:features/files/FileDetails.tsx`);
  });
});
