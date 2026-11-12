/** The native lifecycle and actual old-unit fact are independent; location never substitutes for them. */
import { afterEach, expect, it } from 'vitest';
import { transferCleanup, transferFacts, transferWords } from '@/features/change/environmentTransfer';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });
it('decodes only actual fields and keeps absent or foreign facts unknown', () => {
  for (const input of [null, undefined, [], { machine: {}, host: {}, template: {}, from: null }, { kind: 'environment_move' }]) {
    expect(transferFacts(input)).toEqual({ kind: null, sourceHasUnit: null });
  }
  expect(transferFacts({ kind: 'switch_copy', source_has_unit: false, from: { name: 'a host page' } })).toEqual({ kind: 'switch_copy', sourceHasUnit: false });
  expect(transferFacts({ kind: 'move', source_has_unit: false })).toEqual({ kind: 'move', sourceHasUnit: false });
  expect(transferFacts({ kind: 'switch_copy', source_has_unit: true })).toEqual({ kind: 'switch_copy', sourceHasUnit: true });
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names directory copies and keeps no-unit and unknown progress truthful ${locale}`, async () => {
    await setLocale(locale);
    const copy = transferFacts({ kind: 'switch_copy', source_has_unit: false });
    const words = transferWords(copy, 'Workspace', 'destination', i18n.t);
    expect(words.title).toBe(i18n.t('envTransfer.copyTitle', { name: 'Workspace' }));
    expect(words.confirm).toBe(i18n.t('envTransfer.copy'));
    expect(words.done).toBe(i18n.t('envTransfer.copyDone', { name: 'Workspace', destination: 'destination' }));
    expect(transferCleanup('cleanup', copy, i18n.t)).toBe(i18n.t('envTransfer.cleanupSource'));
    const noUnitMove = transferWords(transferFacts({ kind: 'move', source_has_unit: false }), 'Workspace', 'destination', i18n.t);
    expect(noUnitMove.done).toBe(i18n.t('envTransfer.done', { name: 'Workspace', destination: 'destination' }));
    const unit = transferFacts({ kind: 'move', source_has_unit: true });
    expect(transferWords(unit, 'Workspace', 'destination', i18n.t).done).toBe(i18n.t('change.env.doneMove', { name: 'Workspace', host: 'destination' }));
    expect(transferCleanup('cleanup', unit, i18n.t)).toBe(i18n.t('envTransfer.cleanupUnit'));
    const unknown = transferFacts(undefined);
    expect(transferCleanup('cleanup', unknown, i18n.t)).toBe(i18n.t('envTransfer.cleanup'));
    expect(transferWords(unknown, 'Workspace', null, i18n.t).heading).toBe(i18n.t('envTransfer.changing', { name: 'Workspace' }));
    expect(transferCleanup('copy', unit, i18n.t)).toBeNull();
  });
}
