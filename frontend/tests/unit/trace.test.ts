/** A selected person's trail keeps its scope, exact meaning, dates and continuation. */
import { describe, expect, it } from 'vitest';
import { kindOf } from '@/features/trace/catalogue';
import { exportSpanValid, groupEntries, traceHref, traceSearch, type TraceScope } from '@/features/trace/params';
import { mayReadTrail } from '@/features/trace/Person';
import { auditSentence } from '@/features/trace/sentence';
import { i18n, setLocale } from '@/i18n';
import { ACME, memberAccess } from '../doubles/fixtures';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';

const scope: TraceScope = { type: 'node', id: ACME.web, orgId: ACME.org, kind: 'project', name: 'Web', access: { ...memberAccess, permissions: ['audit.view'] } };
describe('following a person', () => {
  it('keeps a folder and actor instead of substituting its root organization', () => {
    expect(traceHref({ ...scope, id: ACME.payments }, 'actor-id', { action: 'node.deleted', kind: 'deleted', from: '2026-10-01', group: 'kind' })).toBe(`/project/${ACME.payments}/settings?action=node.deleted&kind=deleted&from=2026-10-01&group=kind&actor=actor-id&include_agents=true&section=audit`);
  });
  it('offers a trail only from the current audit right or system authority', () => {
    expect(mayReadTrail(scope)).toBe(true);
    expect(mayReadTrail({ ...scope, access: { ...memberAccess, role: 'admin', permissions: [] } })).toBe(false);
    expect(mayReadTrail({ type: 'system', name: 'System' })).toBe(false);
    expect(mayReadTrail({ type: 'system', name: 'System' }, true)).toBe(true);
  });
  it('keeps an exact action distinct from its kind', () => {
    expect(kindOf('permission.removed')).toBe('access');
    expect(kindOf('document.deleted')).toBe('deleted');
    expect(kindOf('deleted.inspected')).toBe('access');
    expect(kindOf('auth.token_refused')).toBe('sign_ins');
    expect(kindOf('provider.reset_used')).toBe('work');
    expect(kindOf('thread.background_end')).toBe('work');
    expect(kindOf('thread.created')).toBe('made');
    expect(kindOf('admin.opened')).toBe('access');
    expect(kindOf('auth.admin_mode_changed')).toBe('sign_ins');
    expect(kindOf('vendor.removed')).toBeUndefined();
    expect(traceSearch({ actor: 'a', action: 'permission.removed', kind: 'access', from: '2026-10-01', to: '2026-10-12', group: 'kind' })).toEqual({ actor: 'a', action: 'permission.removed', kind: 'access', from: '2026-10-01', to: '2026-10-12', group: 'kind' });
  });
});
describe('continuation and export', () => {
  const row = (id: string, at: string, kind: 'made' | 'access'): TraceEntry => ({ id, at, action: 'permission.removed', kind }) as TraceEntry;
  it('continues one day across pages and reads midnight in the reader’s zone', () => {
    const first = [row('one', '2026-10-12T23:55:00Z', 'made')]; const next = [row('two', '2026-10-13T00:05:00Z', 'access')];
    expect(groupEntries([...first, ...next], 'day', 'America/New_York').map((group) => group.rows.map((item) => item.id))).toEqual([['one', 'two']]);
    expect(groupEntries([...first, ...next, ...first], 'kind', 'UTC').map((group) => group.rows.map((item) => item.id))).toEqual([['one'], ['two']]);
  });
  it('accepts a full leap year but refuses an extra day, reversed or missing dates', () => {
    expect(exportSpanValid('2024-01-01', '2024-12-31')).toBe(true);
    expect(exportSpanValid('2024-01-01', '2025-01-01')).toBe(false);
    expect(exportSpanValid('2026-10-12', '2026-10-11')).toBe(false);
    expect(exportSpanValid(undefined, '2026-10-12')).toBe(false);
  });
  it('keeps an unfamiliar action in a labelled group without losing the known rows', () => {
    const known = row('created', '2026-10-12T23:55:00Z', 'made');
    const unfamiliar: TraceEntry = { ...known, id: 'unfamiliar', action: 'future.reindexed', kind: null };
    expect(groupEntries([known, unfamiliar], 'kind', 'UTC').map((group) => [group.key, group.rows.map((entry) => entry.id)]))
      .toEqual([['made', ['created']], ['unknown', ['unfamiliar']]]);
  });
});

it('labels an unfamiliar stored action in either language and retains its safe summary', async () => {
  const entry = { action: 'future.reindexed', kind: null, summary: 'A retained observation' } as TraceEntry;
  try {
    await setLocale('en');
    expect(auditSentence(entry, i18n.t)).toBe('Unrecognized action: A retained observation');
    await setLocale('zh-CN');
    expect(auditSentence(entry, i18n.t)).toBe('未识别的操作：A retained observation');
  } finally {
    await setLocale('en');
  }
});

it('known structured deletion facts use the reader’s language and an unknown form keeps the safe sentence', async () => {
  const entry = { action: 'node.deleted', actor: { kind: 'user', user: { display_name: 'Dora' } }, details: { version: 1, name: 'Old draft' }, summary: 'A safe server sentence' } as TraceEntry;
  // RequestRow names the actor separately; the sentence says only what changed.
  try {
    for (const [locale, deleted] of [['en', 'Deleted Old draft'], ['zh-CN', '删除了Old draft']] as const) {
      await setLocale(locale);
      expect(auditSentence(entry, i18n.t)).toBe(deleted);
      expect(auditSentence({ ...entry, details: { version: 2, name: 'Old draft' } }, i18n.t)).toBe('A safe server sentence');
    }
  } finally { await setLocale('en'); }
});

it('administrative openings and the session switch use the reader’s words', async () => {
  const entry = {action:'admin.opened',actor:{kind:'user',user:{display_name:'Dora'}},object:{name:'Handbook'},details:{},summary:'server words'} as TraceEntry;
  for (const [locale,opened,on,off] of [
    ['en','Opened Handbook as admin.','Turned on As admin for this session.','Turned off As admin for this session.'],
    ['zh-CN','以管理员身份打开了Handbook。','为本次登录开启了“以管理员身份”。','为本次登录关闭了“以管理员身份”。'],
  ] as const) {
    try {
      await setLocale(locale);
      expect(auditSentence(entry,i18n.t)).toBe(opened);
      expect(auditSentence({...entry,action:'auth.admin_mode_changed',details:{as_admin:true}},i18n.t)).toBe(on);
      expect(auditSentence({...entry,action:'auth.admin_mode_changed',details:{as_admin:false}},i18n.t)).toBe(off);
    } finally { await setLocale('en'); }
  }
});
