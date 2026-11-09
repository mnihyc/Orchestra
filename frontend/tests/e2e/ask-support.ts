/** The Ask click path uses product controls in both languages, with contract-shaped server replies. */
import { createInstance } from 'i18next';
import { expect, type APIRequestContext, type Page, type Route } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { AskCreateBody, AskEvent, AskEventData, AskGetReply, AskMessageBody } from '../../src/generated/api-types';
import { ASK_ANSWER, ASK_ID, ASK_TURN, ASK_USER, askConversation, askMessage, askModels } from '../unit/ask-fixtures';
import { id } from '../doubles/fixtures';
import { threadDraft } from '../doubles/thread-rows';
import { DOCS } from './documents-support';
import { ACME, DOUBLE, PEOPLE, signIn } from './support';

export async function fixture(page: Page, supplied: AskGetReply[] = []) {
  const records = new Map<string, AskGetReply>(supplied.map((snapshot) => [snapshot.conversation.id, snapshot]));
  const events = new Map<string, AskEvent[]>();
  const creates: AskCreateBody[] = [], sends: AskMessageBody[] = [], keeps: unknown[] = [];
  const waiting: { route: Route; id: string; after: number; done: () => void }[] = [];
  page.on('requestfailed', (request) => {
    for (const waiter of [...waiting]) if (waiter.route.request() === request) { waiting.splice(waiting.indexOf(waiter), 1); waiter.done(); }
  });
  page.on('close', () => { for (const waiter of waiting.splice(0)) waiter.done(); });
  let stops = 0;
  const drafts = new Map<string, ReturnType<typeof threadDraft> | null>();
  const olderPages = new Map<string, AskGetReply>();
  await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: askModels(new URL(route.request().url()).searchParams.get('root_org_id') ?? ACME.org) }));
  await page.route('**/api/v1/threads/*/draft', async (route) => {
    const id = new URL(route.request().url()).pathname.split('/')[4];
    if (id === undefined || !records.has(id)) { await route.continue(); return; }
    if (route.request().method() === 'GET') { await route.fulfill({ json: drafts.get(id) ?? null }); return; }
    if (route.request().method() === 'DELETE') { drafts.set(id, null); await route.fulfill({ status: 204 }); return; }
    const body = route.request().postDataJSON() as { content: AskMessageBody['content'] };
    const draft = threadDraft(body.content); drafts.set(id, draft); await route.fulfill({ json: draft });
  });
  await page.route('**/api/v1/threads/*/attachments', async (route) => {
    const conversationId = new URL(route.request().url()).pathname.split('/')[4];
    if (conversationId === undefined || !records.has(conversationId)) { await route.continue(); return; }
    const body = route.request().postDataJSON() as { blob: string; name: string };
    await route.fulfill({ json: { blob: body.blob, name: body.name, path: `.orch/attachments/${body.name}`, size: 11 } });
  });
  const flush = async (id: string) => {
    for (const waiter of [...waiting]) {
      if (waiter.id !== id) continue;
      const pending = (events.get(id) ?? []).filter((event) => event.seq > waiter.after);
      if (pending.length === 0) continue;
      waiting.splice(waiting.indexOf(waiter), 1);
      await waiter.route.fulfill({ contentType: 'text/event-stream', body: pending.map((event) => `event: ask\nid: ${event.seq}\ndata: ${JSON.stringify(event)}\n\n`).join('') }).catch(() => undefined);
      waiter.done();
    }
  };
  const publish = async (id: string, data: AskEventData) => {
    const snapshot = records.get(id); if (snapshot === undefined) throw new Error('No conversation fixture');
    snapshot.conversation.seq += 1;
    events.set(id, [...events.get(id) ?? [], { conversation_id: id, seq: snapshot.conversation.seq, data }]);
    await flush(id);
  };
  const record = (id: string) => { const snapshot = records.get(id); if (snapshot === undefined) throw new Error('No conversation fixture'); return snapshot; };
  await page.route('**/api/v1/ask/conversations**', async (route) => {
    const request = route.request(), url = new URL(request.url()), tail = url.pathname.slice('/api/v1/ask/conversations'.length).split('/').filter(Boolean);
    const method = request.method(), conversationId = tail[0];
    if (tail.length === 0 && method === 'GET') {
      const root = url.searchParams.get('root_org_id');
      const inspection = url.searchParams.get('inspect') === 'true';
      await route.fulfill({ json: { items: [...records.values()].map((value) => value.conversation).filter((row) => (root === null || row.root_org_id === root) && (inspection || row.user_id === askConversation().user_id)), next_cursor: null } }); return;
    }
    if (tail.length === 0 && method === 'POST') {
      const body = request.postDataJSON() as AskCreateBody; creates.push(body);
      const nextId = records.size === 0 ? ASK_ID : id(0x72, records.size + 10);
      records.set(nextId, { conversation: askConversation({ id: nextId, root_org_id: body.root_org_id ?? ACME.org, title: 'Release questions', seq: 0 }), messages: [], previous_cursor: null });
      await route.fulfill({ status: 201, json: { conversation_id: nextId, turn_id: null, seq: 0 } }); return;
    }
    if (conversationId === undefined || !records.has(conversationId)) { await route.fulfill({ status: 404, json: { status: 404, code: 'not_found', message: 'Conversation unavailable', request_id: 'fixture-ask-missing' } }); return; }
    const current = record(conversationId);
    if (tail.length === 1 && method === 'GET') { await route.fulfill({ json: olderPages.get(`${conversationId}:${url.searchParams.get('before') ?? ''}`) ?? current }); return; }
    if (tail[1] === 'export' && method === 'GET') { await route.fulfill({ json: current }); return; }
    if (tail[1] === 'events') {
      const after = Number(url.searchParams.get('after') ?? 0);
      if ((events.get(conversationId) ?? []).some((event) => event.seq > after)) {
        await route.fulfill({ contentType: 'text/event-stream', body: (events.get(conversationId) ?? []).filter((event) => event.seq > after).map((event) => `event: ask\ndata: ${JSON.stringify(event)}\n\n`).join('') });
      } else await new Promise<void>((done) => { waiting.push({ route, id: conversationId, after, done }); });
      return;
    }
    if (tail[1] === 'messages' && method === 'POST') {
      const body = request.postDataJSON() as AskMessageBody;
      if (!sends.some((before) => before.client_id === body.client_id)) {
        sends.push(body);
        current.messages.push(askMessage({ id: ASK_USER, role: 'user', content: body.content }), askMessage({ id: ASK_ANSWER, content: [{ type: 'text', text: '' }], complete: false }));
        current.conversation.state = 'running'; current.conversation.seq += 1;
      }
      await route.fulfill({ status: 202, json: { conversation_id: conversationId, turn_id: ASK_TURN, seq: current.conversation.seq } }); return;
    }
    if (tail[1] === 'stop' && method === 'POST') {
      stops += 1; current.conversation.state = 'ready';
      await publish(conversationId, { kind: 'conversation', conversation: { ...current.conversation } });
      await route.fulfill({ json: { conversation_id: conversationId, turn_id: ASK_TURN, seq: current.conversation.seq } }); return;
    }
    if (tail[1] === 'keep' && method === 'POST') {
      keeps.push(request.postDataJSON()); current.conversation.state = 'kept';
      await route.fulfill({ status: 201, json: { conversation_id: conversationId, thread_id: ACME.depsThread, topic_id: ACME.upgradeDeps } }); return;
    }
    await route.fulfill({ status: 405 });
  });
  return {
    creates, sends, keeps, record, drafts, get stops() { return stops; },
    olderPage: (cursor: string, snapshot: AskGetReply) => { olderPages.set(`${snapshot.conversation.id}:${cursor}`, snapshot); },
    foreignNotice: async () => {
      const seq = record(ASK_ID).conversation.seq + 1;
      events.set(ASK_ID, [...events.get(ASK_ID) ?? [], { conversation_id: id(0x73, 1), seq, data: { kind: 'notice', code: 'fixture', message: 'Another organization note' } }]);
      await flush(ASK_ID);
    },
    delta: async (text: string) => {
      const snapshot = record(ASK_ID), answer = snapshot.messages.find((message) => message.id === ASK_ANSWER);
      if (answer === undefined) throw new Error('No answer fixture');
      const before = answer.content.flatMap((part) => part.type === 'text' ? [part.text] : []).join('');
      answer.content = [{ type: 'text', text: before + text }];
      await publish(ASK_ID, { kind: 'delta', message_id: ASK_ANSWER, offset: Array.from(before).length, text });
    },
    finish: async () => {
      const snapshot = record(ASK_ID), answer = snapshot.messages.find((message) => message.id === ASK_ANSWER);
      if (answer === undefined) throw new Error('No answer fixture');
      answer.complete = true; answer.citations = [{ reference: 'release-plan', url: `/doc/${DOCS.plan}`, title: 'Rollout plan' }];
      await publish(ASK_ID, { kind: 'message', message: { ...answer } });
      snapshot.conversation.state = 'ready'; await publish(ASK_ID, { kind: 'conversation', conversation: { ...snapshot.conversation } });
    },
    contextRefusal: async (code: 'ask_context_no_room' | 'ask_context_too_long', model: string) => {
      const snapshot = record(ASK_ID), answer = snapshot.messages.find((message) => message.id === ASK_ANSWER);
      if (answer === undefined) throw new Error('No answer fixture');
      answer.complete = true; snapshot.conversation.state = 'ready';
      await publish(ASK_ID, { kind: 'message', message: { ...answer } });
      await publish(ASK_ID, { kind: 'conversation', conversation: { ...snapshot.conversation } });
      const error = { kind: 'error' as const, code, model, message: 'Server context refusal' };
      await publish(ASK_ID, error);
    },
    capStop: async (message: string) => {
      const snapshot = record(ASK_ID), answer = snapshot.messages.find((message) => message.id === ASK_ANSWER);
      if (answer === undefined) throw new Error('No answer fixture');
      answer.complete = true; snapshot.conversation.state = 'ready';
      await publish(ASK_ID, { kind: 'error', code: 'cap_reached', message });
      await publish(ASK_ID, { kind: 'reset' });
    },
  };
}

export async function language(page: Page, request: APIRequestContext, locale: 'en' | 'zh-CN', theme: 'light' | 'dark', person: { login: string; password: string } = PEOPLE.ada) {
  await signIn(page, person);
  const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: person.login, password: person.password } });
  const { access_token: token } = await login.json() as { access_token: string };
  expect((await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } })).ok()).toBe(true);
  const i18n = createInstance(); await i18n.init({ lng: locale, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
  return i18n.getFixedT(locale);
}
