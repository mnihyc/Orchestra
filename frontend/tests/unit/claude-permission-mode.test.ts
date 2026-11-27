/** Reported native modes are human-readable without replacing the requested policy. */
import { afterEach, expect, it } from 'vitest';
import { nativeChangeValue, markerSentence } from '@/features/timeline/items/Marker';
import { i18n, setLocale } from '@/i18n';

afterEach(async()=>{await setLocale('en');});
for(const locale of ['en','zh-CN'] as const) it(`shows Claude's actual permission mode in the thread (${locale})`,async()=>{
  await setLocale(locale);
  const labels={default:'choiceCatalog.approvalMode.ask.name',acceptEdits:'choiceCatalog.approvalMode.on_request.name',auto:'choiceCatalog.approvalMode.auto.name',bypassPermissions:'choiceCatalog.approvalMode.bypass.name',plan:'harnessChanges.values.plan'} as const;
  for(const [mode,key] of Object.entries(labels)) {
    const reported={native_mode:mode};
    const shown=i18n.t('harnessChanges.ownValue',{name:i18n.t(key),own:mode});
    expect(nativeChangeValue('approval_mode',reported,i18n.t)).toBe(shown);
    expect(nativeChangeValue('approval_mode',reported,i18n.t,true)).toBe(i18n.t(key));
    const line=markerSentence({marker:'harness_changed',data:{program:'Claude Code',change:'approval_mode',subject:null,
      previous:{state:'unsupported'},value:reported,expected:{state:'told',value:{native_mode:'default'}},
      changed:false,diverged:mode!=='default',next_turn:false,turn_id:'turn',reported_at:'2026-11-07T00:00:00Z'}},i18n.t,{locale});
    expect(line).toContain('Claude Code');
    expect(line).toContain(i18n.t('harnessChanges.settings.approval_mode'));
    expect(line).toContain(shown);
  }
  expect(nativeChangeValue('approval_mode',{native_mode:'future-native-mode'},i18n.t)).toBe('future-native-mode');
  const policies:[Record<string,string>,string,string][]=[
    [{policy:'never'},'choiceCatalog.approvalMode.bypass.name','never'],
    [{policy:'untrusted'},'choiceCatalog.approvalMode.ask.name','untrusted'],
    [{policy:'on-request'},'choiceCatalog.approvalMode.on_request.name','on-request'],
    [{policy:'on-request',reviewer:'auto_review'},'choiceCatalog.approvalMode.auto.name','auto_review'],
    [{policy:'on-failure'},'harnessChanges.values.onFailure','on-failure'],
  ];
  for(const [reported,key,own] of policies) {
    expect(nativeChangeValue('approval_mode',reported,i18n.t)).toBe(i18n.t('harnessChanges.ownValue',{name:i18n.t(key),own}));
    expect(nativeChangeValue('approval_mode',reported,i18n.t,true)).toBe(i18n.t(key));
  }
  expect(nativeChangeValue('approval_mode',{native_mode:'future-native-mode'},i18n.t,true)).toBe('future-native-mode');
});
