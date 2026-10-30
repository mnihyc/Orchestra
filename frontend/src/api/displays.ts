/** API client for the displays of an environment: the list with what was found there to serve one, opening a virtual screen, closing one (or no longer showing one an agent announced), and a ticket for its socket.
 *
 * The display itself is a socket that carries the protocol of VNC as it is, drawn by an existing viewer
 * (`features/vnc`); here are the requests around it. The list is kept current by the environment's feed
 * of displays, which the view follows.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { Display, DisplayTicketCreateReply, EnvironmentDisplayCreateBody, EnvironmentDisplays } from '@/generated/api-types';

export const displayKeys = {
  list: (environmentId: string) => ['environment-displays', environmentId] as const,
};

/** The displays of an environment, what was found there to serve one, and why they are off where they are. */
export const displaysQuery = (environmentId: string) =>
  queryOptions({
    queryKey: displayKeys.list(environmentId),
    queryFn: ({ signal }): Promise<EnvironmentDisplays> => ok(api.GET('/environments/{id}/displays', { params: { path: { id: environmentId } }, signal })),
  });

/** The same list, after the environment looked again for what it has to serve a display. */
export function lookAgainForDisplays(environmentId: string): Promise<EnvironmentDisplays> {
  return ok(api.GET('/environments/{id}/displays', { params: { path: { id: environmentId }, query: { look: true } } }));
}

/** Opens a virtual screen of the size asked, or the environment's; beside a thread, the thread is named. */
export function openDisplay(environmentId: string, body: EnvironmentDisplayCreateBody, threadId: string | null): Promise<Display> {
  return ok(api.POST('/environments/{id}/displays', { params: { path: { id: environmentId }, query: threadId === null ? {} : { thread: threadId } }, body }));
}

/** Closes a display Orch opened, with what shows on it; one an agent announced is only no longer shown. */
export function closeDisplay(displayId: string): Promise<Display> {
  return ok(api.DELETE('/displays/{display_id}', { params: { path: { display_id: displayId } } }));
}

/** A single-use ticket for the display's socket: to watch, or with `control` to work in it. */
export function displayTicket(displayId: string, control: boolean): Promise<DisplayTicketCreateReply> {
  return ok(api.POST('/displays/{display_id}/ticket', { params: { path: { display_id: displayId } }, body: { control } }));
}
