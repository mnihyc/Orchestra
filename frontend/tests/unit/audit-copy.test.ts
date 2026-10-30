/** Every shipped exact action and setting value has readable words without changing wire identities. */
import { afterEach, expect, it } from 'vitest';
import { ACTIONS } from '@/features/trace/catalogue';
import { auditActionName, auditSetting } from '@/features/trace/words';
import { auditSentence } from '@/features/trace/sentence';
import { retainedJobWords } from '@/features/trace/jobWords';
import type { AuditEntry } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('names every shipped audit action from the %s catalogue', async (locale) => {
  await setLocale(locale);
  for (const action of Object.values(ACTIONS).flat()) {
    const [noun, verb] = action.split('.');
    expect(i18n.getResource(locale, 'translation', `trace.actionThings.${noun}`), action).toBeTypeOf('string');
    expect(i18n.getResource(locale, 'translation', `trace.actionVerbs.${verb}`), action).toBeTypeOf('string');
    expect(auditActionName(action, i18n.t), action).not.toBe(action);
    expect(auditActionName(action, i18n.t), action).not.toBe(i18n.t('trace.unrecognizedAct'));
  }
  expect(auditActionName('vendor.future_key', i18n.t)).toBe(i18n.t('trace.unrecognizedAct'));
  expect(auditSentence({action:'vendor.future_key',summary:'vendor.future_key'} as AuditEntry,i18n.t)).toBe(i18n.t('trace.unrecognizedAct'));
  expect(retainedJobWords(i18n.t,{state:'running',phase:'opaque_phase'})).toBe(i18n.t('source.job.running'));
  expect(retainedJobWords(i18n.t,{state:'queued',phase:'opaque_phase'})).toBe(i18n.t('source.job.queued'));
  for(const action of ['workspace.state_recorded','auth.admin_mode_changed']){
    const entry={action,summary:action,actor:{kind:'system'},details:{}} as AuditEntry;
    expect(auditSentence(entry,i18n.t)).toBe(auditActionName(action,i18n.t));
    expect(auditSentence(entry,i18n.t)).not.toContain(action);
  }
  const unnamed={action:'admin.opened',summary:'admin.opened',actor:{kind:'system'},details:{},object:{name:null}} as AuditEntry;
  expect(auditSentence(unnamed,i18n.t)).toBe(i18n.t('trace.sentenceAdminOpenedUnnamed',{actor:i18n.t('trace.systemActor')}));
  expect(auditSentence(unnamed,i18n.t)).not.toMatch(/Unknown|未知/);
});

it.each(['en', 'zh-CN'] as const)('reads setting changes and resets without raw modes or historical model routes in %s', async (locale) => {
  await setLocale(locale);
  const base = { action: 'settings.updated', actor: { kind: 'system' }, details: { key: 'approval.mode', value: 'on_request', was: 'ask', locked: true } } as AuditEntry;
  expect(auditSetting(base, i18n.t)).toMatchObject({ value: i18n.t('choiceCatalog.approvalMode.on_request.name'), was: i18n.t('choiceCatalog.approvalMode.ask.name'), locked: true });
  expect(auditSentence(base, i18n.t)).not.toContain('on_request');
  expect(auditSetting({ ...base, details: { key: 'approval.mode', value: 'future_mode' } }, i18n.t)?.value).toBe(i18n.t('trace.valueUnreported'));
  const reset = { ...base, action: 'settings.reset', details: { key: 'approval.mode', reset: true } };
  expect(auditSentence(reset, i18n.t)).toBe(i18n.t('trace.sentenceSettingReset', { actor: i18n.t('trace.systemActor'), setting: i18n.t('settingRegistry.approval.mode.title') }));
  const model = { ...base, details: { key: 'agent.model', value: 'private_model_route' } };
  expect(auditSentence(model, i18n.t)).not.toContain('private_model_route');
  expect(auditSetting(model, i18n.t)?.value).toBeNull();
});


it.each(['en', 'zh-CN'] as const)('translates a legacy wire-action summary without dropping its target or a safe human sentence in %s', async (locale) => {
  await setLocale(locale);
  const entry = { action: 'document.renamed', summary: 'Dora Developer · document.renamed · Main thread', details: {}, object: { name: 'Main thread' } } as AuditEntry;
  expect(auditSentence(entry, i18n.t)).toBe(i18n.t('trace.sentenceObjectAction', { action: auditActionName(entry.action, i18n.t), name: 'Main thread' }));
  expect(auditSentence(entry, i18n.t)).not.toContain(entry.action);
  expect(auditSentence({ ...entry, summary: 'A safe server sentence' }, i18n.t)).toBe('A safe server sentence');
});
