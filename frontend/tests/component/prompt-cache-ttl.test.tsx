/** Cache lifetime keeps the native default and names declared unsupported programs on the same row. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { ResolvedSetting } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { settingChoice } from '@/i18n/choices';
import { show } from './support';

vi.mock('@/features/chat/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useOpenPath: () => vi.fn() }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const locale=currentLocale();
afterEach(async()=>{await setLocale(locale);});
const key='agents.prompt_cache_ttl';
function definition():KeyDef {
  return {key,title:i18n.t('settingRegistry.agents.prompt_cache_ttl.title'),description:i18n.t('settingRegistry.agents.prompt_cache_ttl.description'),
    group:'agent',class:'parameter',scopes:['system','org','project','topic','thread'],merge:'replace',
    schema:{anyOf:[{type:'string',enum:['5m','1h']},{type:'null'}]},default:null,example:null,text_key:null,keywords:[]};
}
function row(value:unknown,program:string|null):ResolvedSetting & {not_applicable_program?:string} {
  return {key,value,own:null,source:{scope:'default',node:null},locked_by:null,writable:false,parts:[],
    ...(program===null?{}:{not_applicable_reason:'prompt_cache_ttl_unsupported',not_applicable_program:program})};
}
it.each(['en','zh-CN'] as const)('shows the program default and unsupported facts without hiding the stored value in %s',async(language)=>{
  await setLocale(language);
  const def=definition();
  const node=(setting:ResolvedSetting)=><SettingRow def={def} setting={setting} target={{kind:'node',id:'thread'}}
    place={{name:'Conversation',kind:'thread',owner:null,access:{permissions:['thread.view']}}}/>;
  const view=show(node(row(null,null)));
  expect(screen.getByText(language==='en'?"Program's own default":'程序自身的默认值')).toBeVisible();
  for(const program of ['Codex','DeepSeek Harness']) {
    view.rerender(node(row('1h',program)));
    expect(screen.getByText(language==='en'?`Not used by ${program}.`:`${program} 不使用此设置。`)).toBeVisible();
    expect(screen.getByText(language==='en'?'1 hour':'1 小时')).toBeVisible();
  }
  expect(settingChoice(i18n.t,key,'5m')?.name).toBe(language==='en'?'5 minutes':'5 分钟');
});
