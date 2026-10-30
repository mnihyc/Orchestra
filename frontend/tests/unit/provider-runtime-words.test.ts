/** Runtime markers and future values are presented in human words in either catalogue. */
import { expect,it } from 'vitest';
import { providerRuntimeState } from '@/features/resources/providers/state';
import { i18n,setLocale } from '@/i18n';

it.each(['en','zh-CN'] as const)('names a new runtime and keeps an unknown marker off the screen in %s',async(locale)=>{
  await setLocale(locale);
  try {
    const t=i18n.getFixedT(locale);
    expect(providerRuntimeState('creating',t)).toBe(locale==='en'?'Creating runtime':'正在创建运行时');
    const unknown=providerRuntimeState('future_internal_state',t);
    expect(unknown).toBe(locale==='en'?'State unavailable':'状态暂不可用');
    expect(unknown).not.toContain('future_internal_state');
  } finally {await setLocale('en');}
});
