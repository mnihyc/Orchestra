/** Structured history reads in each catalogue; removed names and old audit identifiers stay ordinary words. */
import { describe, expect, it } from 'vitest';
import { historySentence } from '@/features/inspector/historySentence';
import { activityLine } from '@/features/org/Activity';
import type { HistoryLine, ActivityEntry } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

const target = { id: '01a13540-5bc7-7580-b794-e2ff8e97f034', name: 'Acme', kind: 'org' as const, redacted: false };
const row = { actor: { display_name: 'Ada Lovelace' }, text: `Ada named the agents of ${target.id} in the View Thread row`, wording: { version: 1, verb: 'agentDefaults', target } } as HistoryLine;

describe('history wording', () => {
  it('uses structured facts in both languages, including the defaults example', async () => {
    await setLocale('en');
    expect(historySentence(row, i18n.t)).toBe('Ada Lovelace changed who may act as an agent in Acme’s defaults');
    await setLocale('zh-CN');
    expect(historySentence(row, i18n.t)).toBe('Ada Lovelace更改了Acme默认设置中谁可以以智能体身份操作');
    expect(activityLine({ kind: 'history', action: 'permission.added', node: target, actor: row.actor, title: row.text, wording: row.wording } as ActivityEntry, i18n.t)).toBe(historySentence(row, i18n.t));
    await setLocale('en');
  });
  it('names a removed thread by its kind and never fills its name with the identifier', () => {
    const gone = { ...row, wording: { version: 1, verb: 'deleted', target: { ...target, kind: 'thread' as const, name: null, redacted: true } } };
    expect(historySentence(gone, i18n.t)).toBe('Ada Lovelace deleted a removed thread');
    expect(historySentence(gone, i18n.t)).not.toContain(target.id);
  });
  it('does not interpret English fallback sentences or a newer vocabulary', () => {
    expect(historySentence({ ...row, text: 'A safe server sentence', wording: { version: 2, verb: 'created' } }, i18n.t)).toBe('A safe server sentence');
    expect(historySentence({ ...row, wording: { version: 2, verb: 'future' } }, i18n.t)).toBe('Ada Lovelace made a recorded change');
  });
  it('QS-008 composes resource and work acts in each language from the actual object, never the server sentence', async () => {
    const object = { id: 'template-1', kind: 'template', name: 'KVM allowed', redacted: false };
    const activity = {
      at: '2026-10-15T01:00:00.000Z', actor: row.actor, count: 1, node: target,
      action: 'resource.created', kind: 'history', thread: null, item_id: null,
      title: 'Ada Lovelace made KVM allowed',
      object,
      wording: { version: 1, verb: 'created', target },
    } as ActivityEntry;
    await setLocale('en');
    expect(activityLine(activity, i18n.t)).toBe('Ada Lovelace made KVM allowed');
    await setLocale('zh-CN');
    expect(activityLine(activity, i18n.t)).toBe('Ada Lovelace创建了KVM allowed');
    const work = { ...activity, object: { ...object, kind: 'environment', name: 'Build room' }, wording: { version: 1, verb: 'work', target } };
    expect(activityLine(work, i18n.t)).toBe('Ada Lovelace在Build room中运行了工作');
    const removed = { ...activity, object: { id: 'template-1', kind: 'template', name: null, redacted: true }, wording: { version: 1, verb: 'deleted', target } };
    expect(activityLine(removed, i18n.t)).toBe('Ada Lovelace删除了已移除的模板');
    expect(activityLine(removed, i18n.t)).not.toContain('template-1');
    const removedThread = { ...removed, action: 'node.deleted', node: { ...target, kind: 'thread' as const, name: null, redacted: true }, object: { ...removed.object, kind: 'thread' } };
    expect(activityLine(removedThread, i18n.t)).toBe('Ada Lovelace删除了已移除的会话');
    const unknown = { ...activity, wording: { version: 2, verb: 'future' } };
    expect(activityLine(unknown, i18n.t)).toBe('Ada Lovelace对KVM allowed作出了一项变更');
    await setLocale('en');
  });
});
