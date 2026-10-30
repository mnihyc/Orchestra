// One turn, the way a browser sees it: follow a thread's feed on the realtime socket (through the web edge),
// send the text over HTTP, decide the approvals the turn asks for over HTTP, and print what the frames bring as
// one JSON object a line until the turn of the input has ended.
//
//   ORCH_TOKEN=<access token> node follow.mjs --api <http base of /api/v1> --ws <ws url of /api/v1/ws>
//                                              --thread <id> [--approve <option id>] [--timeout <seconds>] -- <text>
//
// Lines: {"event":"item","kind":…,"status":…,"summary":…}, {"event":"approval","title":…,"option":…,"state":…},
// {"event":"turn","state":…}. Exit status: 0 when the turn completed, 1 when it failed or was interrupted, 2 when
// it did not end in time, 3 when the input was refused.
import { randomUUID } from 'node:crypto';

const argv = process.argv.slice(2);
const split = argv.indexOf('--');
const flags = split < 0 ? argv : argv.slice(0, split);
const text = split < 0 ? '' : argv.slice(split + 1).join(' ');
const flag = (name, fallback) => {
  const at = flags.indexOf(`--${name}`);
  return at < 0 ? fallback : flags[at + 1];
};
const api = flag('api');
const wsUrl = flag('ws');
const thread = flag('thread');
const approveWith = flag('approve', null);
const timeoutSeconds = Number(flag('timeout', '120'));
const token = process.env.ORCH_TOKEN ?? '';

const emit = (object) => console.log(JSON.stringify(object));
const clip = (value, max = 160) => {
  const line = (typeof value === 'string' ? value : JSON.stringify(value)).replace(/\s+/g, ' ');
  return line.length > max ? `${line.slice(0, max)}...` : line;
};

async function http(method, path, body) {
  const reply = await fetch(`${api}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, ...(body ? { 'content-type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const raw = await reply.text();
  let json = null;
  try {
    json = JSON.parse(raw);
  } catch {
    /* an answer without a body */
  }
  return { status: reply.status, json, raw };
}

// What one item says of itself, in a line.
function summary(row) {
  const payload = row.payload ?? {};
  switch (row.kind) {
    case 'agent_message':
      return payload.text ?? '';
    case 'user_message':
      return (payload.parts ?? []).map((part) => part.text ?? `[${part.type}]`).join(' ');
    case 'tool_call': {
      const tool = payload.tool ?? {};
      if (tool.kind === 'command') {
        return `${tool.command}${tool.exit_code != null ? ` -> exit ${tool.exit_code}` : ''} ${tool.output?.inline ?? ''}`.trim();
      }
      if (tool.kind === 'file_change') {
        return (tool.changes ?? []).map((change) => `${change.change} ${change.path}`).join(', ');
      }
      return clip(tool);
    }
    default:
      return clip(payload);
  }
}

const shown = new Map(); // item id -> "kind/status"
const turns = new Map(); // turn id -> state
const asked = new Set();
let inputId = null;
let finished = null;
const clientId = `follow-${randomUUID()}`;

async function lookAtApprovals() {
  const list = await http('GET', '/approvals');
  for (const approval of list.json?.items ?? []) {
    if (approval.thread_id !== thread || approval.state !== 'pending' || asked.has(approval.id)) continue;
    asked.add(approval.id);
    if (!approveWith) {
      emit({ event: 'approval', title: approval.title, state: 'pending' });
      continue;
    }
    const decided = await http('POST', `/approvals/${approval.id}/decision`, {
      option_id: approveWith,
      note: 'decided by the smoke scenario',
    });
    emit({ event: 'approval', title: approval.title, option: approveWith, state: decided.json?.state ?? `HTTP ${decided.status}` });
  }
}

function apply(frame) {
  const data = frame.data ?? {};
  if ((data.thread?.pending_approvals ?? 0) > 0) lookAtApprovals().catch((error) => emit({ event: 'error', detail: String(error) }));
  for (const row of data.items ?? []) {
    const state = `${row.kind}/${row.status}`;
    if (shown.get(row.id) === state || row.kind === 'marker') continue;
    shown.set(row.id, state);
    emit({ event: 'item', kind: row.kind, status: row.status, summary: summary(row) });
  }
  for (const turn of data.turns ?? []) {
    if (turns.get(turn.id) === turn.state || turn.input_id !== inputId) continue;
    turns.set(turn.id, turn.state);
    emit({ event: 'turn', state: turn.state, error: turn.error ?? null });
    if (['completed', 'failed', 'interrupted'].includes(turn.state)) {
      finished = turn.state;
      setTimeout(() => process.exit(finished === 'completed' ? 0 : 1), 500);
    }
  }
}

const ws = new WebSocket(wsUrl, ['orch.v1']);
ws.onopen = () => ws.send(JSON.stringify({ op: 'auth', token }));
ws.onmessage = async (event) => {
  const frame = JSON.parse(event.data);
  if (frame.op === 'ready') {
    ws.send(JSON.stringify({ op: 'sub', id: 1, feed: `thread:${thread}` }));
  } else if (frame.op === 'subscribed') {
    const reply = await http('POST', `/threads/${thread}/inputs`, {
      content: [{ type: 'text', text }],
      delivery: 'queue',
      client_id: clientId,
    });
    if (reply.status !== 201 && reply.status !== 200) {
      emit({ event: 'refused', status: reply.status, detail: clip(reply.raw, 400) });
      process.exit(3);
    }
    inputId = reply.json.input.id;
  } else if (frame.op === 'sync') {
    apply(frame);
  }
};
ws.onclose = (event) => {
  if (!finished) emit({ event: 'closed', code: event.code });
};
setTimeout(() => {
  emit({ event: 'timeout', seconds: timeoutSeconds });
  process.exit(2);
}, timeoutSeconds * 1000);
