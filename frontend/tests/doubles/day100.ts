/** Own-part setting resolution and worker-controlled fixtures, sharing the existing places and settings of the fake world. */
import type { Answer, Part, World } from './world.ts';
import { Refusal, type Person } from './world.ts';
import type { Favorite, ProjectView as PersonalProjectView } from '../../src/generated/api-types.ts';

const record = (value: unknown): Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
const text = (value: unknown, fallback = ''): string => typeof value === 'string' ? value : fallback;

export class Day100World implements Part {
  private readonly world: World;
  private readonly favoriteViews = new Map<string, Map<string, PersonalProjectView | null>>();
  constructor(world: World) { this.world = world; }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const saved = /^\/me\/state\/project_views\/([^/]+)$/.exec(path);
    if (saved?.[1] !== undefined && (method === 'PUT' || method === 'DELETE')) {
      const id = saved[1];
      if (method === 'PUT' && this.world.places.node(person, id).kind !== 'project') throw new Refusal(400, 'request.invalid', 'a saved view is only for a project');
      const state = this.world.settings.stateOf(person);
      const views = { ...record(state.project_views) };
      const next = method === 'DELETE' ? Object.fromEntries(Object.entries(views).filter(([key]) => key !== id)) : { ...views, [id]: this.view(body) };
      state.project_views = next;
      this.world.users.tell(person.me.username, { what: 'state', key: 'project_views', value: next });
      return { status: 204 };
    }
    const star = /^\/me\/favorites\/([^/]+)$/.exec(path);
    if (star?.[1] !== undefined && (method === 'PUT' || method === 'DELETE')) {
      const id = star[1];
      let supplied: PersonalProjectView | null | undefined;
      if (method === 'PUT' && Object.hasOwn(body, 'view')) {
        if (this.world.places.node(person, id).kind !== 'project') throw new Refusal(400, 'request.invalid', 'a saved view is only for a project');
        supplied = body.view === null ? null : this.view(record(body.view));
      }
      const answer = this.world.places.handle(person, method, path, query, body);
      const views = this.favoriteViews.get(person.me.username) ?? new Map<string, PersonalProjectView | null>();
      if (method === 'DELETE') views.delete(id);
      else if (supplied !== undefined) views.set(id, supplied);
      this.favoriteViews.set(person.me.username, views);
      return answer;
    }
    if (method === 'GET' && path === '/me/favorites') {
      const answer = this.world.places.handle(person, method, path, query, body);
      if (answer === null || !Array.isArray(answer.body)) return answer;
      return { ...answer, body: (answer.body as Favorite[]).map((favorite) => ({ ...favorite, view: this.favoriteViews.get(person.me.username)?.get(favorite.node.id) ?? null })) };
    }
    return null;
  }

  private view(body: Record<string, unknown>): PersonalProjectView {
    const valid = (body.view === 'list' || body.view === 'board') && (body.status === 'open' || body.status === 'done' || body.status === 'all') && (body.sort === 'activity' || body.sort === 'created' || body.sort === 'name') && Array.isArray(body.tag_ids) && body.tag_ids.every((id) => typeof id === 'string') && typeof body.q === 'string' && typeof body.show_archived === 'boolean' && typeof body.with_folders === 'boolean' && [null, 'needs_attention', 'running', 'waiting_approval', 'blocked'].includes(body.attention as string | null);
    if (!valid) throw new Refusal(400, 'request.invalid', 'the project view is invalid');
    return body as unknown as PersonalProjectView;
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/topics-open') {
      const person = this.world.people.get('ada');
      if (person === undefined) throw new Refusal(404, 'not_found', 'no such person');
      const ids = Array.isArray(body.ids) ? body.ids as unknown[] : [];
      for (const id of ids) if (typeof id === 'string') this.world.places.changeTopic(person, id, { status: 'open' });
      return { status: 204 };
    }
    if (path === '/appearance') {
      const person = this.world.people.get(text(body.username, 'ada'));
      if (person === undefined) throw new Refusal(404, 'not_found', 'no such person');
      if (body.locale === 'en' || body.locale === 'zh-CN') person.me.locale = body.locale;
      if (body.theme === 'light' || body.theme === 'dark') person.me.theme = body.theme;
      return { status: 204 };
    }
    if (path === '/setting') {
      const node = text(body.node);
      const key = text(body.key);
      if (body.clear === true) this.world.settings.clearValue(node, key);
      else this.world.settings.setValue(node, key, body.value, body.locked === true);
      return { status: 204 };
    }
    if (path === '/view') {
      const person = this.world.people.get(text(body.username, 'ada'));
      if (person === undefined) throw new Refusal(404, 'not_found', 'no such person');
      return this.handle(person, 'PUT', `/me/state/project_views/${text(body.project)}`, new URLSearchParams(), record(body.view));
    }
    return null;
  }
}
