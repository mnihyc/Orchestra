/** A destructive flow keeps the same familiar verb from entrance to confirmation and result. */
import {afterEach,expect,it} from 'vitest';
import {i18n,setLocale} from '@/i18n';

afterEach(async()=>{await setLocale('en');});
const deletions=[
  'resources.act.delete','resourceDetails.remove','resourceDetails.removeTitle',
  'resourceDetails.mount.removeLocation','resources.act.delete','providers.danger.deleteTitle','providers.danger.deleteConfirm',
  'providers.danger.delete','providers.danger.deleteTitle','providers.danger.deleteConfirm',
  'settings.channels.remove','settings.channels.removeTitle','settings.channels.removeConfirm',
  'settings.service.remove','settings.service.removeTitle','settings.service.removeConfirm',
  'settings.secrets.removeNamed','settings.secrets.removeTitle','settings.secrets.removeConfirm',
  'settings.destinations.removeTitle','settings.destinations.removeConfirm',
  'members.groups.remove','members.groups.removeTitle','members.groups.removeConfirm',
  'admin.idp.remove','admin.idp.removeTitle','admin.idp.removeConfirm',
  'admin.capabilities.remove','admin.capabilities.removeTitle','admin.capabilities.removeConfirm',
  'hosts.remove.menu','hosts.remove.title','hosts.remove.confirm','hosts.runtimes.remove','hosts.runtimes.removeTitle','hosts.runtimes.removeConfirm',
  'machinePrograms.remove','machinePrograms.removeTitle','trace.removeTitle','trace.removeConfirm',
  'export.list.remove','export.list.removeTitle','export.list.removeDo',
  'notifications.webhook.remove','notifications.webhook.removeTitle','notifications.webhook.removeConfirm',
];
it.each(['en','zh-CN'] as const)('uses Delete for owned things, Revoke for certificates and Remove for membership in %s',async(locale)=>{
  await setLocale(locale);
  for(const key of deletions){
    const value=i18n.t(key,{name:'Workshop',defaultValue:''});
    expect(value,key).not.toBe('');expect(value,key).toMatch(locale==='en'?/^Delete\b/:/^删除/);
  }
  // Imported and configured MCP presets share the resource page's Delete flow.
  expect(i18n.t('resourceDetails.remove')).toBe(locale==='en'?'Delete':'删除');
  expect(i18n.t('resourceDetails.removeTitle',{name:'Workshop'})).toBe(locale==='en'?'Delete Workshop?':'删除 Workshop？');
  for(const key of ['machines.revoke.button','machines.revoke.title','machines.revoke.confirm','machines.bulk.revokeTitle','machines.bulk.revokeConfirm']){
    const value=i18n.t(key,{name:'Workshop',count:2,defaultValue:''});
    expect(value,key).toMatch(locale==='en'?/^Revoke\b.*certificate/:/^吊销.*证书/);
  }
  expect(i18n.t('members.groups.take',{name:'Dana'})).toMatch(locale==='en'?/^Remove Dana from/:/移除Dana/);
  expect(i18n.t('settings.members.end.submit',{name:'Dana',root:'Team'})).toMatch(locale==='en'?/^Remove Dana from/:/移除/);
  for(const key of ['providers.models.removed','settings.channels.removed','settings.service.removed','admin.idp.removed','admin.capabilities.removed','hosts.runtimes.removed','notifications.webhook.removed'])expect(i18n.t(key,{name:'Workshop',defaultValue:''}),key).toMatch(locale==='en'?/deleted/i:/删除/);
});
