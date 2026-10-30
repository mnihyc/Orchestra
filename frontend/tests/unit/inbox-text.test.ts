/** Inbox actions follow the reader's language while authored words and unknown versions retain their safe text. */
import { describe, expect, it } from 'vitest';

import { notificationText } from '@/features/inbox/text';
import { permissionLabel } from '@/lib/permissions';
import type { Notification } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

function notice(kind: Notification['kind'], parameters: Record<string, unknown> = {}): Notification {
  return {
    id: 'notice', kind, title: 'Stored English title', body: 'Stored English explanation',
    node: null, thread_id: null, approval_id: null, chat_id: null, url: '/inbox', data: parameters,
    created_at: '2026-10-14T12:00:00.000Z', read_at: null, inbox_seq: 1, group: null, waiting_for_me: false,
    details: { v: 1, action: kind, parameters },
  };
}

describe('notification action text', () => {
  it.each(['en', 'zh-CN'] as const)('localizes an explicit person notice and preserves its authored note (%s)', async (locale) => {
    await setLocale(locale);
    try {
      const kind = 'person.notify' as unknown as Notification['kind'];
      const row = notice(kind, { subject: 'Guide' });
      row.body = 'Please review';
      expect(notificationText(row, i18n.t, locale)).toEqual({ title: locale === 'en' ? 'Guide: You were notified' : 'Guide：你收到了一条通知', body: 'Please review' });
      row.details = { v: 2, action: kind, parameters: row.data };
      expect(notificationText(row, i18n.t, locale)).toEqual({ title: row.title, body: row.body });
    } finally { await setLocale('en'); }
  });
  it.each(['en','zh-CN'] as const)('names a requested runtime update and the measured impact without English fallback (%s)',async(locale)=>{
    await setLocale(locale);
    try {
      const row=notice('environment.update',{reason:'requested',affected_threads:2});
      const text=notificationText(row,i18n.t,locale,'Current readable thread');
      expect(text.title).toBe(locale==='en'?'Update requested':'已请求更新');
      expect(text.body).toBe(locale==='en'?'2 affected threads · running work may stop':'2 个线程受影响 · 运行中的工作可能停止');
      expect(text.body).not.toContain('Stored English');
      row.details={v:2,action:row.kind,parameters:row.data};
      expect(notificationText(row,i18n.t,locale,'Current readable thread')).toEqual({title:row.title,body:row.body});
    } finally {await setLocale('en');}
  });
  it.each(['en', 'zh-CN'] as const)('names provider re-creation as a provider operation (%s)', async (locale) => {
    await setLocale(locale);
    try {
      const row = notice('environment.attention', { operation_kind: 'provider_recover', name: 'Team provider', state: 'done', provider_ids: ['provider'], environment_ids: [], thread_count: 1 });
      const text = notificationText(row, i18n.t, locale);
      expect(text.title).toBe(locale === 'en' ? 'Team provider re-created' : 'Team provider 已重新创建');
      expect(text.title).not.toContain('Environments');
    } finally { await setLocale('en'); }
  });

  it.each(['en', 'zh-CN'] as const)('names all affected resource kinds and counts private threads without their identities (%s)', async (locale) => {
    await setLocale(locale);
    try {
      const row = notice('environment.attention', { operation_kind: 'lost', name: 'host-2', thread_count: 1204, environment_ids: ['mine'], provider_ids: ['provider'], resource_ids: [['mount', 'mount'], ['mcp_server', 'server']], state: 'done' });
      const text = notificationText(row, i18n.t, locale);
      expect(text.title).toBe(i18n.t('loss.inboxTitle', { name: 'host-2' }));
      expect(text.body).toBe(locale === 'en' ? '1 of your environments · 1,204 threads · 1 provider runtime · 1 mount location · 1 MCP server' : '您有 1 个环境受影响 · 1,204 个会话 · 1 个提供方运行时 · 1 个挂载位置 · 1 个 MCP 服务器');
      expect(text.body).not.toContain('{{');
    } finally { await setLocale('en'); }
  });

  it.each(['en', 'zh-CN'] as const)('shows an answered question as content supplied by its actual person (%s)', async (locale) => {
    await setLocale(locale);
    try {
      for (const structured of [true, false]) {
        const row = notice('approval.requested', { approval_state: 'answered' });
        if (!structured) row.details = null;
        row.actor = { id: 'answerer', kind: 'human', username: 'answerer', display_name: 'Question owner', avatar: { color: 0, text: 'QO' } };
        const text = notificationText(row, i18n.t, locale, 'Current thread');
        expect(text.title).toBe(locale === 'en' ? 'Answered by Question owner' : 'Question owner已作答');
        expect(text.namesActor).toBe(true);
        row.actor = null;
        expect(notificationText(row, i18n.t, locale, 'Current thread').title).toBe(locale === 'en' ? 'Answered' : '已作答');
      }
    } finally { await setLocale('en'); }
  });

  it.each(['en','zh-CN'] as const)('shows the actual settled request and decider without repeating a pending question (%s)', async (locale) => {
    await setLocale(locale);
    const actions = [
      ['approved', 'Allowed by Ada Lovelace', 'Ada Lovelace已允许', 'Allowed', '已允许'],
      ['denied', 'Denied by Ada Lovelace', 'Ada Lovelace已拒绝', 'Denied', '已拒绝'],
      ['cancelled', 'Request withdrawn', '请求已撤回', 'Request withdrawn', '请求已撤回'],
      ['expired', 'Request expired', '请求已过期', 'Request expired', '请求已过期'],
    ] as const;
    try {
      for (const kind of ['approval.requested','capability.requested','access.requested'] as const) for (const [state, english, chinese, anonymousEnglish, anonymousChinese] of actions) {
        const row = notice(kind, { approval_state: state });
        row.actor = { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada Lovelace', avatar: { color: 0, text: 'AL' } };
        const text = notificationText(row, i18n.t, locale, 'Review the release');
        expect(text.title).toBe(locale === 'en' ? english : chinese);
        expect(text.namesActor).toBe(true);
        row.actor = null;
        expect(notificationText(row, i18n.t, locale, 'Review the release').title).toBe(locale === 'en' ? anonymousEnglish : anonymousChinese);
      }
      const old = notice('approval.requested');
      old.data = { _orch_approval_settled: true };
      expect(notificationText(old, i18n.t, locale).title).toBe(i18n.t('inbox.actions.approvalSettled'));
      const future = notice('approval.requested', { approval_state: 'approved' });
      future.details = { v: 2, action: future.kind, parameters: future.data };
      expect(notificationText(future, i18n.t, locale, 'Review the release')).toEqual({ title: future.title, body: future.body });
    } finally { await setLocale('en'); }
  });

  it.each(['en','zh-CN'] as const)('names Proceed, Keep planning and Run it from the canonical decision (%s)',async locale=>{
    await setLocale(locale);
    try {for(const action of ['proceed','keep_planning','run'] as const){
      const row=notice('approval.requested',{approval_state:action==='keep_planning'?'answered':'approved',request_action:action});
      expect(notificationText(row,i18n.t,locale,'Current thread').title).toBe(i18n.t(`approvals.source.${action}`));
    }}finally{await setLocale('en');}
  });

  it.each(['en','zh-CN'] as const)('replaces saved failed-job diagnostics with the shared recovery sentence (%s)', async (locale) => {
    await setLocale(locale);
    try {
      for (const details of [true,false]) {
        const row=notice('job.finished',{state:'failed',error_code:'harness_unavailable'});
        row.body='harness_unavailable: agent.harnesses: model_inputs_segment_id_fkey';
        if (!details) row.details=null;
        const text=notificationText(row,i18n.t,locale,'Current thread');
        expect(text.body).toBe(i18n.t('ui.error.agentProgramUnavailable'));
        expect(text.body).not.toMatch(/harness_unavailable|agent\.harnesses|_fkey/u);
      }
    } finally {await setLocale('en');}
  });
  it.each(['en', 'zh-CN'] as const)('localizes access facts while retaining the person’s words (%s)', async (locale) => {
    await setLocale(locale);
    try {
      const row = notice('access.requested', { permissions: ['thread.interact'], reason: 'Review the redirect, please.' });
      const text = notificationText(row, i18n.t, locale, 'Current thread');
      expect(text.body).toContain(i18n.t('inbox.actions.requestedPermissions', { permissions: permissionLabel('thread.interact') }));
      expect(text.body).toContain('Review the redirect, please.');
      expect(text.body).not.toContain('Stored English');
      const granted = notice('access.decided', { approved: true, permissions: ['thread.interact'], note: '' });
      expect(notificationText(granted, i18n.t, locale, 'Current thread').body).toBe(i18n.t('inbox.actions.givenPermissions', { permissions: permissionLabel('thread.interact') }));
    } finally { await setLocale('en'); }
  });
  it('renders a completed job and a sign-in lock in Chinese without translating a stored English sentence', async () => {
    await setLocale('zh-CN');
    try {
      const text = notificationText(notice('job.finished', { state: 'succeeded', subject: 'Runtime' }), i18n.t, 'zh-CN');
      expect(text).toEqual({ title: 'Runtime：任务已完成', body: '' });
      const lock = notificationText(notice('sign_in.attention', { way: 'password', until: '2026-10-14T13:00:00.000Z' }), i18n.t, 'zh-CN');
      expect(lock.title).toBe('你的登录需要处理');
      expect(lock.body).toContain('密码错误次数过多');
      expect(lock.body).not.toContain('Stored English');
    } finally { await setLocale('en'); }
  });

  it('keeps the author of an announcement in control of its words and degrades an unknown version safely', () => {
    const authored = notice('announcement');
    authored.title = 'Ready for review';
    authored.body = 'Please check the design.';
    expect(notificationText(authored, i18n.t, 'en')).toEqual({ title: authored.title, body: authored.body });
    const future = notice('job.finished');
    future.details = { v: 2, action: 'job.finished', parameters: {} };
    expect(notificationText(future, i18n.t, 'en')).toEqual({ title: future.title, body: future.body });
    expect(notificationText(future, i18n.t, 'en', 'Current thread')).toEqual({ title: future.title, body: future.body });
  });
  it('uses a current thread reference instead of a stored subject, with the event said once', () => {
    const row = notice('turn.completed', { subject: 'Old thread name' });
    expect(notificationText(row, i18n.t, 'en', 'Current thread')).toEqual({ title: 'Turn completed', body: '' });
    expect(notificationText(row, i18n.t, 'en', null)).toEqual({ title: 'Turn completed', body: '' });
    const mentioned = notice('mention');
    expect(notificationText(mentioned, i18n.t, 'en', 'Current thread').body).toBe(mentioned.body);
  });
  it.each(['en', 'zh-CN'] as const)('does not repeat raw storage failures from current or historical jobs (%s)', async (locale) => {
    await setLocale(locale);
    try {
      const row = notice('job.finished', { state: 'failed', error_code: 'internal' });
      row.body = 'a row this depends on is missing (model_inputs_segment_id_fkey)';
      expect(notificationText(row, i18n.t, locale, 'Current thread').body).toBe(i18n.t('errors.internal'));
      row.details = undefined;
      expect(notificationText(row, i18n.t, locale, 'Current thread').body).toBe(i18n.t('errors.internal'));
    } finally { await setLocale('en'); }
  });
  it.each(['en','zh-CN'] as const)('explains a temporary removal deadline in the readers language (%s)',async(locale)=>{
    await setLocale(locale);
    try {
      const row=notice('environment.attention',{reason:'temporary',subject:'Experiment',expires_at:'2026-10-18T12:00:00Z'});
      const text=notificationText(row,i18n.t,locale);
      expect(text.title).toBe(i18n.t('temporary.noticeTitle',{name:'Experiment'}));
      expect(text.body).toContain(locale==='en'?'export':'导出');
      expect(text.body).not.toContain('Stored English');
      row.details={v:2,action:row.kind,parameters:row.data};
      expect(notificationText(row,i18n.t,locale)).toEqual({title:row.title,body:row.body});
    } finally {await setLocale('en');}
  });
});

for (const locale of ['en','zh-CN'] as const) it(`names a stopped recovery with the same counted reason (${locale})`,async()=>{
  await setLocale(locale);
  try {
    const row=notice('thread.blocked',{reason:'repeat_limit',failure_kind:'provider_busy',count:4,window_secs:300});
    const text=notificationText(row,i18n.t,locale,'Current thread');
    expect(text.body).toContain(i18n.t('calls.kinds.provider_busy'));
    expect(text.body).toContain('4');
    expect(text.body).toContain(locale==='en'?'5 min':'5分钟');
    expect(text.body).not.toContain(row.body);
    expect(text.title).toBe(i18n.t('inbox.actions.threadBlocked'));
  } finally {await setLocale('en');}
});
