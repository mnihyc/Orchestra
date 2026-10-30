/** Per-project personal views and favorite view writes through the existing authenticated API client. */
import { api, done, ok, repeatable } from '@/api/client';
import type { Tag, Topic, TopicStatus, ProjectView as PersonalProjectView } from '@/generated/api-types';

/** Writes one entry so a simultaneous save in another project cannot replace it. */
export function saveProjectView(projectId: string, view: PersonalProjectView): Promise<void> {
  return done(api.PUT('/me/state/project_views/{project_id}' as never, { params: { path: { project_id: projectId } }, body: view } as never));
}

/** Explicit null stars the place alone; a view replaces this person's one saved choice for the project. */
export function starProjectView(projectId: string, view: PersonalProjectView | null): Promise<void> {
  const body = { view };
  return done(api.PUT('/me/favorites/{node_id}', { params: { path: { node_id: projectId } }, body } as never));
}

/** A status change and its inverse both use the revision captured when their complete review was made. */
export function changeTopicStatus(id: string, status: TopicStatus, revision: number): Promise<Topic> {
  const body = { status, expected_revision: revision };
  return ok(api.PATCH('/topics/{id}', { params: { path: { id } }, body }));
}

/** Omitting colour lets the server select the next unused colour under the defining place's guard. */
export function createPickerTag(nodeId: string, name: string, key: string): Promise<Tag> {
  return ok(api.POST('/nodes/{id}/tags', { params: { path: { id: nodeId } }, body: { name } as never, ...repeatable(key) }));
}
