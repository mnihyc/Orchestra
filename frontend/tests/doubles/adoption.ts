/** The sessions the harnesses of the machines hold that were begun outside Orch, what adopting one would do, and adopting it: a thread with its earlier turns from the session before Orch. */
import { randomUUID } from 'node:crypto';

import type { AdoptPlan, Item, NativeSession } from '../../src/generated/api-types.ts';
import { id as fixtureId, KIND, userRef } from './fixtures.ts';
import { agentMessage, marker, userMessage } from './thread-rows.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** A session a harness holds: what lists it, the model it ran on, and what was said in it before. */
interface HeldSession {
  session: NativeSession;
  model: string | null;
  /** Its earlier turns: what the person said, and what the agent answered. */
  turns: { said: string; answered: string }[];
}

/** The provider registered from ada-laptop and the profile of its harness there. */
const LAPTOP_PROVIDER = fixtureId(KIND.provider, 2);
const LAPTOP_AGENT = fixtureId(KIND.agent, 4);
const LAPTOP = fixtureId(KIND.machine, 1);

function seeded(): Map<string, HeldSession[]> {
  return new Map([
    [
      `${LAPTOP_PROVIDER}/${LAPTOP_AGENT}`,
      [
        {
          session: { native_session_ref: 'session-login', title: 'Fix the login button', cwd: '/home/ada/work/main/web', updated_at: '2026-10-14T08:00:00.000Z', adopted_thread_id: null },
          model: 'gpt-5.5-codex',
          turns: [
            { said: 'Fix the login button: it does nothing', answered: 'The handler was undefined; it calls submit() now.' },
            { said: 'Run the tests', answered: 'All 42 pass.' },
          ],
        },
        {
          session: { native_session_ref: 'session-elsewhere', title: null, cwd: '/srv/other', updated_at: '2026-10-13T08:00:00.000Z', adopted_thread_id: null },
          model: null,
          turns: [],
        },
      ],
    ],
  ]);
}

export class AdoptionWorld implements Part {
  private held = seeded();
  private readonly world: World;

  constructor(world: World) {
    this.world = world;
  }

  reset(): void {
    this.held = seeded();
  }

  private find(body: Record<string, unknown>): { key: string; entry: HeldSession } {
    const key = `${String(body.provider_id)}/${String(body.agent_id)}`;
    const entry = this.held.get(key)?.find((held) => held.session.native_session_ref === body.native_session_ref);
    if (entry === undefined) throw new Refusal(404, 'not_found', 'the harness holds no such session');
    return { key, entry };
  }

  /** What adopting the session into the topic would do. */
  private plan(person: Person, topicId: string, body: Record<string, unknown>): AdoptPlan {
    this.world.places.need(person, topicId, 'topic.interact');
    const { entry } = this.find(body);
    const cwd = entry.session.cwd;
    const environment = this.world.resources.environments
      .filter((record) => record.machine === LAPTOP && cwd !== null && (cwd === record.root || cwd.startsWith(`${record.root}/`)))
      .sort((one, other) => other.root.length - one.root.length)[0];
    const offered = entry.model !== null && this.world.resources.modelsOf(String(body.provider_id)).some((model) => model.model_name === entry.model && model.enabled);
    return {
      session: entry.session,
      name: typeof body.name === 'string' && body.name.trim() !== '' ? body.name.trim() : (entry.session.title ?? 'Adopted session'),
      environment: environment === undefined ? null : this.world.resources.environmentRef(environment),
      directory: environment === undefined || cwd === null ? null : cwd.slice(environment.root.length).replace(/^\//, ''),
      register_first: environment === undefined ? cwd : null,
      session_model: entry.model,
      model: offered ? entry.model : null,
      model_reason: offered ? null : entry.model === null ? 'not_reported' : 'not_offered',
      turns: entry.turns.length,
      history_missing: null,
      adopted_thread_id: entry.session.adopted_thread_id,
    };
  }

  /** Takes the session in: a thread made as any is, with the line that says so and its earlier turns from before Orch. */
  private adopt(person: Person, topicId: string, body: Record<string, unknown>): Answer {
    const plan = this.plan(person, topicId, body);
    const { entry } = this.find(body);
    if (plan.adopted_thread_id !== null) throw new Refusal(409, 'conflict.exists', 'a thread has taken this session in already', { existing_id: plan.adopted_thread_id });
    if (plan.environment === null) throw new Refusal(400, 'environment.none_usable', `none of your environments on that machine holds ${plan.register_first ?? 'its folder'}; register the directory first`);
    const made = this.world.threads.handle(person, 'POST', `/topics/${topicId}/threads`, new URLSearchParams(), {
      name: plan.name,
      binding: { provider_id: body.provider_id, agent_id: body.agent_id },
      environment: { use: plan.environment.id },
      directory: plan.directory ?? '',
      model: plan.model ?? this.world.resources.modelsOf(String(body.provider_id)).find((model) => model.enabled)?.model_name,
    });
    const thread = made?.body;
    if (typeof thread !== 'object' || thread === null || !('id' in thread) || typeof thread.id !== 'string') throw new Refusal(500, 'internal', 'the thread was not made');
    const owner = this.world.people.get('ada');
    const by = userRef(person.me);
    const items: Item[] = [
      marker(`adopted-${randomUUID()}`, { marker: 'adopted', data: { by, native_session_ref: entry.session.native_session_ref, turns: entry.turns.length, items: entry.turns.length * 2, history_missing: null, session_model: entry.model, model_reason: plan.model_reason } }),
    ];
    entry.turns.forEach((turn, index) => {
      const said = userMessage(`before-said-${index}-${randomUUID()}`, turn.said, { author: userRef((owner ?? person).me) });
      items.push({ ...said, before_orch: true, payload: { ...said.payload, input_id: null } } as Item);
      items.push({ ...agentMessage(`before-answer-${index}-${randomUUID()}`, turn.answered), before_orch: true });
    });
    this.world.threads.emit(thread.id, { items });
    entry.session.adopted_thread_id = thread.id;
    return made ?? { status: 500 };
  }

  handle = (person: Person, method: string, path: string, _query: URLSearchParams, body: Record<string, unknown>): Answer | null => {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    if ((found = /^GET \/threads\/([^/]+)\/adopt\/history$/.exec(route))) {
      const threadId = found[1] ?? '';
      this.world.threads.handle(person, 'GET', `/threads/${threadId}`, new URLSearchParams(), {});
      const adopted = [...this.held.values()].flat().some((entry) => entry.session.adopted_thread_id === threadId);
      return { status: 200, body: adopted ? { state: 'read', reason: null, can_read_again: false } : null };
    }
    if ((found = /^GET \/providers\/([^/]+)\/harnesses\/([^/]+)\/sessions$/.exec(route))) {
      const listed = this.held.get(`${found[1] ?? ''}/${found[2] ?? ''}`);
      if (listed === undefined) return { status: 200, body: [] };
      return { status: 200, body: listed.map((held) => held.session) };
    }
    if ((found = /^POST \/topics\/([^/]+)\/threads\/adopt\/preview$/.exec(route))) return { status: 200, body: this.plan(person, found[1] ?? '', body) };
    if ((found = /^POST \/topics\/([^/]+)\/threads\/adopt$/.exec(route))) return this.adopt(person, found[1] ?? '', body);
    return null;
  };
}
