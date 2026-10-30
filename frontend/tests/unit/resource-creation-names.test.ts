/** One creation act keeps its name at its list, shared entrance, dialog and final choice. */
import { afterEach, expect, it } from 'vitest';
import { i18n, setLocale } from '@/i18n';

const acts = [
  { en: 'Add provider', 'zh-CN': '添加提供方', keys: ['providers.list.add', 'resourceCreation.providers', 'providers.add.title', 'providers.add.submit', 'providerCreation.create'] },
  { en: 'Add machine', 'zh-CN': '添加机器', keys: ['machines.list.add', 'resourceCreation.machines', 'machines.add.title'] },
  { en: 'Add host', 'zh-CN': '添加主机', keys: ['hosts.list.add', 'resourceCreation.hosts', 'hosts.add.title'] },
  { en: 'Add agent profile', 'zh-CN': '添加智能体配置', keys: ['agents.list.add', 'resourceCreation.agents', 'agents.add.title', 'agents.add.submit'] },
  { en: 'Add environment', 'zh-CN': '添加环境', keys: ['environments.list.add', 'resourceCreation.environments', 'environments.make.title', 'environments.make.here', 'environments.make.submit'] },
  { en: 'Add template', 'zh-CN': '添加模板', keys: ['templates.list.add', 'resourceCreation.templates', 'templates.new.title', 'templates.new.submit'] },
  { en: 'Add MCP server', 'zh-CN': '添加 MCP 服务器', keys: ['resourceCreation.mcp', 'resourceDetails.mcp.add'] },
  { en: 'Add mount', 'zh-CN': '添加挂载', keys: ['resourceCreation.mounts', 'resourceDetails.mount.add'] },
] as const;

afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('names every shared creation act consistently in %s', async (locale) => {
  await setLocale(locale);
  for (const act of acts) for (const key of act.keys) expect(i18n.t(key), key).toBe(act[locale]);
  // Management's link opens the list; it does not perform the Add host act.
  expect(i18n.t('admin.hosts.addOpen')).toBe(locale === 'en' ? 'Open Hosts' : '打开主机列表');
});
