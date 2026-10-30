/** Provider policy captions name the controlling place in both languages. */
import { expect, it } from 'vitest';
import { providerWhy } from '@/features/start/words';
import type { ProviderRow } from '@/features/start/choices';
import { i18n, setLocale } from '@/i18n';
import { harnessName } from '../doubles/harness-names';

it('keeps a forbidden provider visible with the actual controlling place', async () => {
  const row:ProviderRow = { id:'provider',isDefault:false,choosable:false,why:{ kind:'reason',reason:'provider_not_allowed' },entry:{ provider:{ id:'provider',name:'Team Pro',kind:'subscription',harness_kind:'fake', harness: harnessName('fake'),address:'acme.team.pro',status:'ready',placement:'managed' },usable:false,reason:'provider_not_allowed',policy_scope:{ id:'project',name:'Research',kind:'project',redacted:false },models:[],agents:[] } };
  expect(providerWhy(row,null,i18n.t.bind(i18n))).toBe('Not allowed by Research');
  await setLocale('zh-CN');
  expect(providerWhy(row,null,i18n.t.bind(i18n))).toBe('Research 不允许使用');
  await setLocale('en');
});
