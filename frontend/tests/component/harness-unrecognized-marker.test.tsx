/** The shared passive observation keeps one native marker vocabulary for every program. */
import { afterEach, expect, it } from 'vitest';
import { markerSentence } from '@/features/timeline/items/Marker';
import type { MarkerPayload } from '@/features/timeline/markerTypes';
import { i18n, setLocale } from '@/i18n';
import { formatNumber } from '@/lib/format';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) for (const program of ['Codex', 'Claude Code', 'DeepSeek Harness', 'Fourth program']) it(`names a counted unknown ${program} event in ${locale}`, async () => {
  await setLocale(locale);
  for (const count of [1, 1000]) {
    const payload: MarkerPayload = { marker:'harness_changed', data:{
      program,change:'unrecognized_event',subject:'future/update',previous:{state:'unsupported'},
      expected:{state:'unsupported'},value:{count},changed:false,diverged:false,next_turn:false,
      turn_id:null,reported_at:'2026-10-08T14:00:00Z',
    }};
    const sentence=markerSentence(payload,i18n.t,{locale});
    expect(sentence).toBe(i18n.t(count===1?'harnessChanges.unrecognized':'harnessChanges.unrecognizedCount',{
      program,name:'future/update',number:formatNumber(count,locale),
    }));
    expect(sentence).not.toContain('reported');
    expect(sentence).not.toContain('settings.');
  }
});
