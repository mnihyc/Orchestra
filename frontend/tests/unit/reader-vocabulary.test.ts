// @vitest-environment node
/** Shipped labels use familiar words; technical identities remain values, never fallback display labels. */
import {createInstance} from 'i18next';
import {expect,it} from 'vitest';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

function leaves(value:unknown,prefix=''):string[]{
  if(typeof value==='string')return [prefix];
  return typeof value==='object'&&value!==null?Object.entries(value).flatMap(([key,inner])=>leaves(inner,prefix===''?key:`${prefix}.${key}`)):[];
}
it('keeps internal labels and withdrawn placeholder prose out of both rendered catalogues',async()=>{
  const instance=createInstance();
  await instance.init({lng:'en',fallbackLng:false,resources:{en:{translation:english},'zh-CN':{translation:chinese}},saveMissing:true,
    missingKeyHandler:(_languages,_namespace,key)=>{throw new Error(`Missing translation: ${key}`);}});
  for(const locale of ['en','zh-CN'] as const)for(const key of leaves(locale==='en'?english:chinese)){
    const value=instance.t(key,{lng:locale,defaultValue:''});
    const words=value.replace(/\{\{[^}]*\}\}/g,'').trim();
    expect(words,`${locale}:${key}`).not.toMatch(/usage part is joined|Export\.{4}|when the usage part|用量功能并入后|Configured ways do not guarantee receipt|已配置的通知方式不保证实际收到/);
    if(!/\.(name|title|label)$/.test(key))continue;
    expect(words,`${locale}:${key}`).not.toMatch(/^(?:Separated|Executor|Executors|Routing name|Orch context|Segment started|Unknown)$/i);
    expect(words,`${locale}:${key}`).not.toMatch(/^(?:docker_socket|host_network|kvm|mock-large|fake|workspace\.state_recorded|settings\.updated|auth\.admin_mode_changed)$/);
  }
  for(const locale of ['en','zh-CN'] as const)for(const key of [
    'contextUsage.ring.label','contextUsage.ring.labelBare','providerDetails.whatWithPlan','providerDetails.balanceTotals','providerDetails.spendTotals','providerDetails.compactSpend',
    'statusItems.generating','timeline.status.compacting','timeline.status.starting','timeline.status.waitingApproval',
    'timeline.marker.segmentStarted','timeline.marker.segmentResumed','timeline.marker.switched',
  ])expect(instance.t(key,{lng:locale,defaultValue:''}),`${locale}:${key}`).not.toContain(' · ');
});
