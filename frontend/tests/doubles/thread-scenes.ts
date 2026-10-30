/** Conversations the fake API can fill a thread with: one that is over, one in the middle of a turn, one that waits for an approval, and a long one.
 *
 * A scene is a list of steps. Each step is rows to put into the thread, in the order the system
 * would have made them; the approvals the rows refer to and the whole outputs of commands whose
 * output is longer than what the row holds go with the scene. A test can play a scene as it is or
 * take its pieces and play them one at a time.
 */
import type { Approval, Item, ThreadInput, Turn } from '../../src/generated/api-types.ts';
import { me, USUAL_MODEL, userRef } from './fixtures.ts';
import {
  agentMessage,
  ALLOW_DENY,
  AT,
  announcement,
  approvalItem,
  change,
  command,
  errorItem,
  fileChange,
  fileRead,
  harnessApproval,
  LOGIN_DIFF,
  marker,
  noticeMessage,
  plan,
  reasoning,
  threadInput,
  toolCall,
  turn as rowTurn,
  userMessage,
} from './thread-rows.ts';
import type { Rows } from './threads.ts';

/** A turn of the model the team's threads usually run on, unless the scene says another: what the thread says of its model and what its turns say are the same. */
const turn = (id: string, options: Parameters<typeof rowTurn>[1]): Turn => rowTurn(id, { model: USUAL_MODEL.name, display: USUAL_MODEL.display, effort: USUAL_MODEL.effort, ...options });

export const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
export const GRACE = userRef(me(2, 'grace', 'Grace Hopper'));

export interface Scene {
  steps: Rows[];
  approvals: Approval[];
  outputs: Record<string, string>;
}

const at = (minutes: number): string => new Date(Date.parse(AT) + minutes * 60_000).toISOString();

const GREEN = '\u001b[32m';
const RED = '\u001b[31m';
const DIM = '\u001b[2m';
const OFF = '\u001b[0m';

const SEARCH_HITS = ['src/login.ts:42:  return redirect(next);', 'src/logout.ts:12:  return redirect("/login");', 'src/session.ts:88:  // redirect to the page the person came from'].join('\n');

const FAILING_TESTS = [
  `${DIM}> web@1.0.0 test${OFF}`,
  `${DIM}> vitest run${OFF}`,
  '',
  ` ${RED}FAIL${OFF}  tests/login.test.ts > login > sends people to the page they came from`,
  `${RED}AssertionError: expected '/' to be '/projects'${OFF}`,
  '  at tests/login.test.ts:31:24',
  '',
  ` Test Files  ${RED}1 failed${OFF} | 3 passed (4)`,
  `      Tests  ${RED}1 failed${OFF} | 18 passed (19)`,
].join('\n');

const PASSING_TESTS = [`${DIM}> vitest run${OFF}`, '', ` ${GREEN}✓${OFF} tests/login.test.ts (6)`, ` ${GREEN}✓${OFF} tests/session.test.ts (5)`, '', ` Test Files  ${GREEN}4 passed${OFF} (4)`, `      Tests  ${GREEN}19 passed${OFF} (19)`].join('\n');

const EXPLANATION = [
  'The handler reads `next` from the query and then ignores it: `redirect(next)` is called with whatever was there, and `undefined` sends people to the root.',
  '',
  'Two things follow from that:',
  '',
  '- a missing `next` has to fall back to the home page,',
  '- an address that points to another site has to be refused.',
  '',
  '```ts',
  'export function login(request: Request) {',
  '  const next = request.query.next;',
  '  return redirect(next ?? "/");',
  '}',
  '```',
].join('\n');

const SUMMARY = [
  'The redirect now falls back to the home page when `next` is missing.',
  '',
  '| Check | Result |',
  '| --- | --- |',
  '| Unit tests | 19 passed |',
  '| Type check | clean |',
  '',
  'See [the change](https://example.org/changes/42) for the whole diff.',
].join('\n');

/** Two turns that are over, with what the work of each looked like. */
export function history(): Scene {
  const first: Rows = {
    items: [
      marker('m-renamed', { marker: 'renamed', data: { by: ADA, from: 'New thread', to: 'Main thread' } }, { at: at(-5) }),
      userMessage('u1', 'Why does the login page send people to the wrong place after they sign in?', { author: ADA, turn: 't1', inputId: 'in-1', at: at(0) }),
      reasoning('r1', 'Look at how the redirect target is read from the request, then at where it is used.', { turn: 't1', at: at(1) }),
      command('c1', {
        command: 'rg -n "redirect" src',
        output: SEARCH_HITS,
        exit: 0,
        turn: 't1',
        at: at(1),
        actions: [{ type: 'search', query: 'redirect', path: 'src' }],
        durationMs: 180,
      }),
      fileRead('f1', { path: 'src/login.ts', content: 'export function login(request: Request) {\n  const next = request.query.next;\n  return redirect(next);\n}\n', from: 40, to: 44, turn: 't1', at: at(2) }),
      agentMessage('a1', EXPLANATION, { turn: 't1', at: at(3), phase: 'final' }),
    ],
    turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1', startedAt: at(0), completedAt: at(3), tokens: 18_400 })],
    inputs: [{ ...threadInput('in-1', 'Why does the login page send people to the wrong place after they sign in?', { author: ADA, position: '1', state: 'delivered' }), turn_id: 't1', delivered_at: at(0) }],
  };
  const second: Rows = {
    items: [
      userMessage('u2', 'Please fix it, and run the tests.', { author: ADA, turn: 't2', inputId: 'in-2', at: at(10) }),
      plan('p1', [
        { text: 'Fall back to the home page when `next` is missing', status: 'completed' },
        { text: 'Refuse addresses of other sites', status: 'completed' },
        { text: 'Run the tests', status: 'completed' },
      ], { turn: 't2', at: at(11) }),
      fileChange('e1', { changes: [change('src/login.ts', LOGIN_DIFF)], turn: 't2', at: at(12) }),
      command('c2', { command: 'npm test', output: FAILING_TESTS, exit: 1, turn: 't2', at: at(13), durationMs: 9_400 }),
      command('c3', { command: 'npm test', output: PASSING_TESTS, exit: 0, turn: 't2', at: at(14), durationMs: 8_100 }),
      agentMessage('a2', SUMMARY, { turn: 't2', at: at(15), phase: 'final' }),
    ],
    turns: [turn('t2', { ordinal: 2, state: 'completed', inputId: 'in-2', startedAt: at(10), completedAt: at(15), tokens: 41_900 })],
    inputs: [{ ...threadInput('in-2', 'Please fix it, and run the tests.', { author: ADA, position: '2', state: 'delivered' }), turn_id: 't2', delivered_at: at(10) }],
    thread: { state: 'idle' },
  };
  return { steps: [first, second], approvals: [], outputs: {} };
}

/** The history, and a third turn that is under way: the agent has begun to answer and a command runs. */
export function running(now: number = Date.now()): Scene {
  const base = history();
  const began = new Date(now - 45_000).toISOString();
  const third: Rows = {
    items: [
      userMessage('u3', 'Now do the same for the logout page.', { author: ADA, turn: 't3', inputId: 'in-3', at: began }),
      reasoning('r3', 'The logout handler sends people to `/login`; check whether it keeps the page they were on.', { turn: 't3', at: began, status: 'in_progress' }),
      command('c4', { command: 'rg -n "logout" src tests', output: 'src/logout.ts:3:export function logout(request: Request) {', turn: 't3', at: began, status: 'in_progress', durationMs: null }),
      agentMessage('a3', 'Looking at the logout handler', { turn: 't3', at: began, status: 'in_progress', phase: 'commentary' }),
    ],
    turns: [turn('t3', { ordinal: 3, state: 'running', inputId: 'in-3', startedAt: began, phase: { kind: 'working', since: began } })],
    inputs: [{ ...threadInput('in-3', 'Now do the same for the logout page.', { author: ADA, position: '3', state: 'delivered' }), turn_id: 't3', delivered_at: began }],
    thread: { state: 'running' },
  };
  return { steps: [...base.steps, third], approvals: [], outputs: {} };
}

/** The history, and a third turn that waits for somebody to allow a command. */
export function waiting(threadId: string, now: number = Date.now()): Scene {
  const base = history();
  const began = new Date(now - 90_000).toISOString();
  const approval = harnessApproval('ap-1', {
    threadId,
    itemId: 'i-ap-1',
    title: 'Run npm install left-pad',
    requester: ADA,
    request: { request_kind: 'command', detail: { command: 'npm install left-pad', cwd: '/work/web', reason: 'The fix needs a padding helper that the project does not have.' }, options: ALLOW_DENY },
  });
  const third: Rows = {
    items: [
      userMessage('u3', 'Pad the ticket numbers in the report.', { author: ADA, turn: 't3', inputId: 'in-3', at: began }),
      agentMessage('a3', 'I need a small package for that.', { turn: 't3', at: began, status: 'completed', phase: 'commentary' }),
      approvalItem('i-ap-1', 'ap-1', { turn: 't3', at: began }),
    ],
    turns: [turn('t3', { ordinal: 3, state: 'running', inputId: 'in-3', startedAt: began, phase: { kind: 'working', since: began } })],
    inputs: [{ ...threadInput('in-3', 'Pad the ticket numbers in the report.', { author: ADA, position: '3', state: 'delivered' }), turn_id: 't3', delivered_at: began }],
    thread: { state: 'waiting_approval' },
  };
  return { steps: [...base.steps, third], approvals: [approval], outputs: {} };
}

/** A turn that failed, with its error, and a stopped one: the two ways a turn ends badly. */
export function failures(): Scene {
  const base = history();
  const failed: Rows = {
    items: [
      userMessage('u3', 'Try the migration.', { author: ADA, turn: 't3', inputId: 'in-3', at: at(20) }),
      errorItem('err-1', 'The model refused the request.', { turn: 't3', at: at(21), retryable: true }),
    ],
    turns: [turn('t3', { ordinal: 3, state: 'failed', inputId: 'in-3', startedAt: at(20), completedAt: at(21) })],
    inputs: [{ ...threadInput('in-3', 'Try the migration.', { author: ADA, position: '3', state: 'delivered' }), turn_id: 't3', delivered_at: at(20) }],
  };
  const stopped: Rows = {
    items: [userMessage('u4', 'Try it with the other config.', { author: ADA, turn: 't4', inputId: 'in-4', at: at(25) }), agentMessage('a4', 'Starting with the config file', { turn: 't4', at: at(26), phase: 'commentary' })],
    turns: [turn('t4', { ordinal: 4, state: 'interrupted', inputId: 'in-4', startedAt: at(25), completedAt: at(26), interruptedBy: ADA })],
    inputs: [{ ...threadInput('in-4', 'Try it with the other config.', { author: ADA, position: '4', state: 'delivered' }), turn_id: 't4', delivered_at: at(25) }],
  };
  return { steps: [...base.steps, failed, stopped], approvals: [], outputs: {} };
}

/** One of each: every kind of item, every kind of step and every kind of request of a harness, in a turn that waits for answers. */
export function everything(threadId: string, now: number = Date.now()): Scene {
  const began = new Date(now - 120_000).toISOString();
  const opts = { turn: 't1', at: began } as const;
  const requester = ADA;
  const request = (id: string, title: string, detail: Parameters<typeof harnessApproval>[1]['request']) => harnessApproval(id, { threadId, itemId: `i-${id}`, title, requester, request: detail });
  const approvals: Approval[] = [
    request('ap-command', 'Run rm -rf build && npm run build', { request_kind: 'command', detail: { command: 'rm -rf build && npm run build', cwd: '/work/web', reason: 'A clean build' }, options: ALLOW_DENY }),
    request('ap-files', 'Change src/login.ts', { request_kind: 'file_change', detail: { changes: [change('src/login.ts', LOGIN_DIFF)], reason: 'It needs write access to the source' }, options: ALLOW_DENY }),
    request('ap-permissions', 'More permissions', { request_kind: 'permissions', detail: { permissions: { network: { enabled: true }, filesystem: { write: ['/work/web'] } }, reason: 'To fetch packages' }, options: ALLOW_DENY }),
    request('ap-input', 'The agent asks you', {
      request_kind: 'user_input',
      detail: {
        questions: [
          { id: 'q1', header: 'Framework', question: 'Which test framework should be used?', options: [{ label: 'Vitest', description: 'Fast, made for Vite' }, { label: 'Jest', description: 'The old default' }], multiple: false, allow_other: true },
          { id: 'q2', header: 'Checks', question: 'Which checks may it run?', options: [{ label: 'Types', description: 'tsc' }, { label: 'Lint', description: 'eslint' }, { label: 'Tests', description: 'the suite' }], multiple: true, allow_other: false },
          { id: 'q3', header: 'Anything else', question: 'Is there anything it should know?', options: null, multiple: false, allow_other: true },
        ],
      },
      options: [],
    }),
    request('ap-mcp', 'The tracker asks for details', {
      request_kind: 'mcp_elicitation',
      detail: { message: 'The tracker needs a few details to file the issue.', schema: { type: 'object', properties: { title: { type: 'string', title: 'Title' }, priority: { type: 'string', title: 'Priority', enum: ['low', 'high'] }, urgent: { type: 'boolean', title: 'Urgent' } }, required: ['title'] }, server: 'tracker' },
      options: [],
    }),
    request('ap-other', 'Something else the harness asks', { request_kind: 'other', detail: { summary: 'Open a connection to the build cache', raw: { host: 'cache.internal', port: 443 } }, options: ALLOW_DENY }),
  ];
  const decided: Approval = { ...request('ap-done', 'Run npm install', { request_kind: 'command', detail: { command: 'npm install', cwd: '/work/web', reason: null }, options: ALLOW_DENY }), state: 'approved', can_decide: false, decided_by: GRACE, decided_at: began, decision: { option_id: 'allow_once' } };
  approvals.push(decided);

  const items: Item[] = [
    marker('m-created', { marker: 'created', data: { agent: null, by: ADA, effort: USUAL_MODEL.effort, environment: null, model: USUAL_MODEL.name, provider: null } }, { at: at(-30) }),
    marker('m-renamed', { marker: 'renamed', data: { by: ADA, from: 'New thread', to: 'Everything at once' } }, { at: at(-20) }),
    marker('m-setting', { marker: 'setting_changed', data: { by: GRACE, from: 'queue', key: 'collab.default_delivery', label: 'How messages are sent', to: 'steer' } }, { at: at(-10) }),
    marker('m-model', { marker: 'model_changed', data: { by: ADA, effort: { from: 'medium', to: USUAL_MODEL.effort }, from: 'gpt-5.4-chat', to: USUAL_MODEL.name } }, { at: at(-5) }),
    marker('m-access', { marker: 'access_changed', data: { by: ADA, summary: 'Anybody who may work in the topic may work here' } }, { at: at(-2) }),
    userMessage('u1', '', {
      author: ADA,
      turn: 't1',
      inputId: 'in-1',
      at: began,
      parts: [{ type: 'text', text: 'Look at these, then fix it:\n\n1. the screenshot\n2. the notes' }, { type: 'image', blob: 'b1', media_type: 'image/png', name: 'screenshot.png' }, { type: 'file', blob: 'b2', name: 'notes.txt', path: 'notes.txt', size: 2_048 }],
    }),
    noticeMessage('n-system', 'The environment was restarted while this turn ran.', 'system', opts),
    announcement('n-announce', { ...opts, title: 'Release freeze', body: 'No merges until Monday.', from: GRACE, severity: 'important' }),
    reasoning('r1', 'The redirect is read in `login`, so start there. Then check `logout` for the same fault.', opts),
    plan('p1', [{ text: 'Read the handler', status: 'completed' }, { text: 'Change the redirect', status: 'in_progress' }, { text: 'Run the tests', status: 'pending' }], { ...opts, explanation: 'Small change, then proof.' }),
    command('c-ansi', { ...opts, command: 'npm test', output: FAILING_TESTS, exit: 1, durationMs: 9_400 }),
    command('c-search', { ...opts, command: 'rg -n redirect src', output: SEARCH_HITS, exit: 0, actions: [{ type: 'search', query: 'redirect', path: 'src' }], durationMs: 180 }),
    command('c-list', { ...opts, command: 'ls -la src', output: 'login.ts\nlogout.ts', exit: 0, actions: [{ type: 'list', path: 'src' }], durationMs: 20 }),
    command('c-multi', { ...opts, command: 'set -e\ncd web\nnpm run build', output: 'built', exit: 0, durationMs: 4_000 }),
    fileChange('e-modify', { ...opts, changes: [change('src/login.ts', LOGIN_DIFF)] }),
    fileChange('e-many', { ...opts, changes: [change('src/new.ts', '@@ -0,0 +1,2 @@\n+export const a = 1;\n+export const b = 2;', { change: 'add' }), change('src/old.ts', '@@ -1,2 +0,0 @@\n-removed\n-removed too', { change: 'delete' }), change('src/b.ts', null, { change: 'rename', old_path: 'src/a.ts' }), change('assets/logo.png', null, { binary: true })] }),
    fileRead('f-read', { ...opts, path: 'src/login.ts', content: 'export function login() {\n  return redirect(next);\n}\n', from: 40, to: 42 }),
    toolCall('t-search', { kind: 'file_search', matches: [{ path: 'src/login.ts', line: 42, text: 'return redirect(next);' }, { path: 'src/logout.ts', line: 12, text: 'return redirect("/login");' }], mode: 'content', path: 'src', query: 'redirect', total: 2, truncated: false }, { ...opts, title: 'Search' }),
    toolCall('t-names', { kind: 'file_search', matches: [{ path: 'src/login.test.ts', line: null, text: null }], mode: 'name', path: null, query: 'login*', total: 1, truncated: false }, { ...opts, title: 'Find files' }),
    toolCall('t-web', { kind: 'web_search', query: 'redirect after login best practice', results: [{ title: 'Open redirect', url: 'https://example.org/open-redirect' }, { title: 'Safe redirects', url: 'https://example.org/safe' }] }, { ...opts, title: 'Web search' }),
    toolCall('t-fetch', { kind: 'web_fetch', content: { blob: null, inline: 'The page says: validate the target.', total_len: 35, truncated: false }, status: 200, title: 'Safe redirects', url: 'https://example.org/safe' }, { ...opts, title: 'Fetch' }),
    toolCall('t-image', { kind: 'image', blob: 'img-1', caption: 'The login page', media_type: 'image/png', name: 'login.png' }, { ...opts, title: 'Image' }),
    toolCall('t-mcp', { kind: 'mcp', arguments: { query: 'login bug' }, error: null, result: { issues: [{ id: 7, title: 'Login redirect' }] }, server: 'tracker', tool: 'search' }, { ...opts, title: 'tracker search' }),
    toolCall('t-mcp-bad', { kind: 'mcp', arguments: { id: 9 }, error: 'The tracker is not reachable.', result: null, server: 'tracker', tool: 'get_issue' }, { ...opts, title: 'tracker get_issue', status: 'failed', error: true }),
    toolCall('t-sub', { kind: 'subagent', child_stream: 'sub:reviewer', name: 'reviewer', prompt: 'Review the change for open redirects', role: 'review', state: 'running' }, { ...opts, title: 'Sub-agent', status: 'in_progress' }),
    { ...agentMessage('sub-review-message', 'The redirect needs to stay on the same origin.', { ...opts, stream: 'sub:reviewer' }), parent_item_id: 't-sub' },
    { ...command('sub-review-check', { ...opts, stream: 'sub:reviewer', command: 'npm test login', output: 'The open redirect case passes.', exit: 0 }), parent_item_id: 't-sub' },
    toolCall('t-orch', { kind: 'orch', arguments: { path: 'docs/' }, result: { files: ['docs/login.md'] }, tool: 'list_shared' }, { ...opts, title: 'Listed shared files' }),
    toolCall('t-other', { kind: 'other', arguments: { a: 1 }, name: 'custom_tool', result: { ok: true } }, { ...opts, title: 'custom_tool' }),
    approvalItem('i-ap-command', 'ap-command', opts),
    approvalItem('i-ap-files', 'ap-files', opts),
    approvalItem('i-ap-permissions', 'ap-permissions', opts),
    approvalItem('i-ap-input', 'ap-input', opts),
    approvalItem('i-ap-mcp', 'ap-mcp', opts),
    approvalItem('i-ap-other', 'ap-other', opts),
    approvalItem('i-ap-done', 'ap-done', { ...opts, status: 'approved' }),
    errorItem('err-1', 'The model refused one request and the harness went on.', { ...opts, retryable: true }),
    agentMessage('a1', 'Working through the list.\n\n- one\n- two', { ...opts, status: 'in_progress', phase: 'commentary' }),
  ];
  const first: Rows = {
    items,
    turns: [turn('t1', { ordinal: 1, state: 'running', inputId: 'in-1', startedAt: began, phase: { kind: 'working', since: began } })],
    inputs: [{ ...threadInput('in-1', 'Look at these', { author: ADA, position: '1', state: 'delivered' }), turn_id: 't1', delivered_at: began }],
    thread: { state: 'waiting_approval' },
  };
  return { steps: [first], approvals, outputs: {} };
}

const COMMANDS = ['npm run build', 'cargo check -p orch-api', 'rg -n "TODO" src', 'git status --short', 'ls -la src/features'];

/**
 * A conversation of about `count` items, made of turns like the ones above, in steps of at most 500
 * items. Every turn has a message of the person, a thought, two commands with their output, a change
 * and the answer.
 */
export function long(count: number): Scene {
  const steps: Rows[] = [];
  const perTurn = 6;
  const turns = Math.ceil(count / perTurn);
  let items: Item[] = [];
  let turnRows: Turn[] = [];
  let inputRows: ThreadInput[] = [];
  for (let n = 1; n <= turns; n += 1) {
    const turnId = `lt${n}`;
    const when = at(n * 2);
    items.push(
      userMessage(`lu${n}`, `Question ${n}: why does check ${n} fail on the second run, and what would you change?`, { author: ADA, turn: turnId, inputId: `lin${n}`, at: when }),
      reasoning(`lr${n}`, `Compare the first and the second run of check ${n}.`, { turn: turnId, at: when }),
      command(`lc${n}a`, { command: COMMANDS[n % COMMANDS.length] ?? 'ls', output: Array.from({ length: 12 }, (_, line) => `line ${line + 1} of the output of step ${n}`).join('\n'), exit: 0, turn: turnId, at: when, durationMs: 1_200 }),
      command(`lc${n}b`, { command: COMMANDS[(n + 2) % COMMANDS.length] ?? 'ls', output: `done ${n}`, exit: n % 7 === 0 ? 1 : 0, turn: turnId, at: when, durationMs: 800 }),
      fileChange(`le${n}`, { changes: [change(`src/check${n % 40}.ts`, LOGIN_DIFF)], turn: turnId, at: when }),
      agentMessage(`la${n}`, `Check ${n} keeps state between runs.\n\nResetting it in \`beforeEach\` fixes the second run; I changed \`src/check${n % 40}.ts\` and ran the suite.`, { turn: turnId, at: when, phase: 'final' }),
    );
    turnRows.push(turn(turnId, { ordinal: n, state: 'completed', inputId: `lin${n}`, startedAt: when, completedAt: when, tokens: 1_000 + n }));
    inputRows.push({ ...threadInput(`lin${n}`, `Question ${n}`, { author: ADA, position: String(n), state: 'delivered' }), turn_id: turnId, delivered_at: when });
    if (items.length >= 500 || n === turns) {
      steps.push({ items, turns: turnRows, inputs: inputRows, ...(n === turns ? { thread: { state: 'idle' as const } } : {}) });
      items = [];
      turnRows = [];
      inputRows = [];
    }
  }
  return { steps, approvals: [], outputs: {} };
}

export type SceneName = 'history' | 'running' | 'waiting' | 'failures' | 'everything' | 'long';

/** A scene by its name, for the thread it is for. */
export function sceneOf(name: SceneName, threadId: string, count = 5_000): Scene {
  switch (name) {
    case 'history':
      return history();
    case 'running':
      return running();
    case 'waiting':
      return waiting(threadId);
    case 'failures':
      return failures();
    case 'everything':
      return everything(threadId);
    case 'long':
      return long(count);
  }
}
