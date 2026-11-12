/** Reported native modes are human-readable without replacing the requested policy. */
import { afterEach, expect, it } from 'vitest';
import { nativeChangeValue, markerSentence } from '@/features/timeline/items/Marker';
import { i18n, setLocale } from '@/i18n';

afterEach(async()=>{await setLocale('en');});
for(const locale of ['en','zh-CN'] as const) it(`shows Claude's actual permission mode in the thread (${locale})`,async()=>{
  await setLocale(locale);
  const labels={default:'ask',acceptEdits:'onRequest',auto:'automatic',bypassPermissions:'bypass',plan:'plan'} as const;
  for(const [mode,label] of Object.entries(labels)) {
    const reported={native_mode:mode};
    expect(nativeChangeValue('approval_mode',reported,i18n.t)).toBe(i18n.t(`harnessChanges.values.${label}`));
    const line=markerSentence({marker:'harness_changed',data:{program:'Claude Code',change:'approval_mode',subject:null,
      previous:{state:'unsupported'},value:reported,expected:{state:'told',value:{native_mode:'default'}},
      changed:false,diverged:mode!=='default',next_turn:false,turn_id:'turn',reported_at:'2026-11-07T00:00:00Z'}},i18n.t,{locale});
    expect(line).toContain('Claude Code');
    expect(line).toContain(i18n.t('harnessChanges.settings.approval_mode'));
    expect(line).toContain(i18n.t(`harnessChanges.values.${label}`));
  }
  expect(nativeChangeValue('approval_mode',{native_mode:'future-native-mode'},i18n.t)).toBe('future-native-mode');
});
