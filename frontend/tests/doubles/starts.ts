/** What starting a thread is asked from: the defaults each place gives the threads made in it, what a thread there could be bound to before it exists, and the acts that mend a default.
 *
 * The defaults are settings written at a place (an org, a project, a topic) and read from the nearest one down the
 * tree, as the real settings are; the team's way of working is written once at Web, so that every topic in it starts
 * from whole defaults, and a place of the tree that has none says so. What a thread could be bound to is the world's own
 * list of providers, profiles, models and environments, said for the person who asks and judged for one environment
 * (`Resources.bindingOptions`); this part only reads the settings that decide how. A control under `/__double/start`
 * writes or clears a default, and the lists the dialog and the inline starter choose among are asked at
 * `GET /nodes/{id}/binding/options`.
 */
import type { ArrangementType } from '../../src/generated/api-types.ts';
import type { BindingGround } from './resources.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

const isText = (value: unknown): value is string => typeof value === 'string' && value !== '';
const asTypes = (value: unknown): ArrangementType[] | null => (Array.isArray(value) ? (value as ArrangementType[]) : null);

export class Starts implements Part {
  readonly world: World;
  /** What the fake was asked to change, in order, with the body: a page that sends a request through a stream cannot be asked what it sent, and the fake can. */
  private sent: { method: string; path: string; body: unknown }[] = [];
  /** Whether the request for what a thread could be bound to is served: a server that does not have it answers as for a request that is not there. */
  private served = true;

  constructor(world: World) {
    this.world = world;
    this.reset();
  }

  reset(): void {
    this.sent = [];
    this.served = true;
  }

  /** What the settings of a place say about how a thread made there is bound. */
  ground(nodeId: string, kind: 'topic' | 'project'): BindingGround {
    const settings = this.world.settings;
    const text = (key: string): string | null => {
      const value = settings.valueAt(nodeId, key).value;
      return isText(value) ? value : null;
    };
    const locked = ['agent.default_provider_id', 'agent.default_agent_id', 'agent.model'].some((key) => settings.valueAt(nodeId, key).lockedBy !== null);
    return {
      node: { id: nodeId, kind },
      defaults: { provider: text('agent.default_provider_id'), agent: text('agent.default_agent_id'), model: text('agent.model'), effort: text('agent.effort'), locked },
      prefer: asTypes(settings.valueAt(nodeId, 'binding.prefer').value) ?? [],
      allowed: asTypes(settings.valueAt(nodeId, 'binding.allowed').value),
      environmentDefault: settings.valueAt(nodeId, 'env.thread_default').value === 'fresh' ? 'fresh' : 'shared',
      topicDefault: text('env.default_id'),
    };
  }

  /** The arrangement a thread made in a topic is bound with: the one named, if it can be had and is allowed, otherwise the first that can be had. */
  arrangementFor(topicId: string, agentId: string, providerId: string, environmentId: string | null, requested: ArrangementType | null): ArrangementType {
    const ground = this.ground(topicId, 'topic');
    return this.world.resources.settle(agentId, providerId, environmentId, requested, ground.prefer, ground.allowed);
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    const places = this.world.places;
    let found: RegExpExecArray | null;
    // This part is asked first, so that it sees every request that changes something, whoever answers it.
    if (method !== 'GET') this.sent.push({ method, path, body });

    if ((found = /^GET \/nodes\/([^/]+)\/binding\/options$/.exec(route))) {
      const id = found[1] ?? '';
      const row = places.rowOf(id);
      if (!this.served) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
      if (row === undefined || row.deleted || (row.kind !== 'topic' && row.kind !== 'project')) throw new Refusal(404, 'not_found', 'there is no such place, or it was deleted');
      places.need(person, id, `${row.kind}.interact`);
      const fresh = query.get('environment_new') === 'true';
      if (fresh && query.has('environment_id')) throw new Refusal(400, 'request.invalid', 'choose one environment target');
      return { status: 200, body: this.world.resources.bindingOptions(person, this.ground(id, row.kind), query.get('environment_id'), fresh) };
    }

    if ((found = /^PUT \/topics\/([^/]+)\/default-environment$/.exec(route))) {
      const id = found[1] ?? '';
      if (places.rowOf(id)?.kind !== 'topic') return null;
      places.need(person, id, 'topic.manage');
      const chosen = typeof body.environment_id === 'string' ? body.environment_id : null;
      this.world.settings.handle(person, chosen === null ? 'DELETE' : 'PUT', `/nodes/${id}/settings/env.default_id`, new URLSearchParams(), { value: chosen });
      return { status: 200, body: places.topic(person, places.rowOf(id)!) };
    }
    return null;
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/sent') return { status: 200, body: this.sent };
    if (path === '/serve') {
      // Whether the request for what a thread could be bound to is answered at all.
      this.served = body.options !== false;
      return { status: 204 };
    }
    if (path === '/defaults') {
      // Writes the defaults of a place, or takes them away: `values` by key, with `null` for a key that is to be left unset.
      const node = typeof body.node === 'string' ? body.node : '';
      const values = typeof body.values === 'object' && body.values !== null ? (body.values as Record<string, unknown>) : {};
      for (const [key, value] of Object.entries(values)) {
        if (!this.world.settings.knows(key)) return { status: 400, body: { detail: `no setting ${key} here` } };
        if (value === null) this.world.settings.clearValue(node, key);
        else this.world.settings.setValue(node, key, value, body.locked === true);
      }
      return { status: 204 };
    }
    return null;
  }
}
