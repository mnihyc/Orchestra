// @vitest-environment node
/** Restricted creation/update replies acknowledge the accepted act without becoming full content. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ReceiptAcknowledged } from '@/api/client';
import { updateGroup } from '@/api/directory';
import { createFolder, createProject, createTopic, createTopicAndThread } from '@/api/nodes';
import { createThread } from '@/api/threads';
import type { CreateThread, GroupUpdateResponse, ProjectCreateResponse, ThreadCreateResponse, TopicCreateReply } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => wire.mockReset());

const parent = '00000000-0000-4000-8101-000000000001';
const node = '00000000-0000-4000-8101-000000000002';
const thread = '00000000-0000-4000-8101-000000000003';
const input = '00000000-0000-4000-8101-000000000004';
const key = 'creation-once';
const named = { name: 'Only this creation', description: 'An accepted change' };
const first: CreateThread = {
  environment: { use: '00000000-0000-4000-8101-000000000005' },
  first_input: { client_id: input, content: [{ type: 'text', text: 'Only this first input' }] },
};

type Receipt = Extract<ProjectCreateResponse | TopicCreateReply | ThreadCreateResponse | GroupUpdateResponse, { projection: 'receipt' }>;
interface Case {
  name: string;
  method: 'POST' | 'PATCH';
  path: string;
  body: unknown;
  key: string | null;
  receipt: Receipt;
  submit: () => Promise<unknown>;
}

const cases: Case[] = [
  { name: 'project', method: 'POST', path: `/orgs/${parent}/projects`, body: named, key,
    receipt: { projection: 'receipt', node_id: node, kind: 'project' } satisfies ProjectCreateResponse,
    submit: () => createProject(parent, named, key) },
  { name: 'folder', method: 'POST', path: `/projects/${parent}/projects`, body: named, key,
    receipt: { projection: 'receipt', node_id: node, kind: 'project' } satisfies ProjectCreateResponse,
    submit: () => createFolder(parent, named, key) },
  { name: 'plain topic', method: 'POST', path: `/projects/${parent}/topics`, body: named, key,
    receipt: { projection: 'receipt', node_id: node, kind: 'topic' } satisfies TopicCreateReply,
    submit: () => createTopic(parent, named, key) },
  { name: 'topic with first thread', method: 'POST', path: `/projects/${parent}/topics`, body: { first_thread: first }, key,
    receipt: { projection: 'receipt', topic_id: node, thread_id: thread, input_id: input } satisfies TopicCreateReply,
    submit: () => createTopicAndThread(parent, first, key) },
  { name: 'thread', method: 'POST', path: `/topics/${parent}/threads`, body: first, key,
    receipt: { projection: 'receipt', topic_id: parent, thread_id: thread, input_id: input } satisfies ThreadCreateResponse,
    submit: () => createThread(parent, first, key) },
  { name: 'group update', method: 'PATCH', path: `/groups/${parent}`, body: named, key: null,
    receipt: { projection: 'receipt', group_id: parent } satisfies GroupUpdateResponse,
    submit: () => updateGroup(parent, named) },
];

it.each(cases)('keeps the $name receipt acknowledged after one exact HTTP write', async (test) => {
  const sent: Request[] = [];
  wire.mockImplementation((request) => {
    sent.push(request.clone());
    return Promise.resolve(new Response(JSON.stringify(test.receipt), {
      status: 200, headers: { 'Content-Type': 'application/json', 'X-Request-Id': 'accepted-once' },
    }));
  });
  const result: unknown = await test.submit().catch((caught: unknown) => caught);
  expect(result).toBeInstanceOf(ReceiptAcknowledged);
  if (!(result instanceof ReceiptAcknowledged)) throw new Error('Expected the accepted change to retain its receipt');
  expect(result.committed).toBe(true);
  expect(result.status).toBe(200);
  expect(result.requestId).toBe('accepted-once');
  expect(result.receipt).toEqual(test.receipt);
  expect(wire).toHaveBeenCalledOnce();
  expect(sent.map((request) => `${request.method} ${new URL(request.url).pathname}`)).toEqual([`${test.method} /api/v1${test.path}`]);
  const request = sent[0];
  if (request === undefined) throw new Error('Expected the single accepted request');
  const body: unknown = await request.json();
  expect(body).toEqual(test.body);
  expect(request.headers.get('Idempotency-Key')).toBe(test.key);
});
