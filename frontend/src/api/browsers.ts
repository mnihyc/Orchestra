/** API client for the browsers of an environment: the list with its profiles and what was found there, opening one (or joining the one already on a profile), quitting it, a ticket for its socket, and a profile made, renamed and deleted.
 *
 * The picture and what a person does in a page go through the browser's own socket
 * (`features/browser/socket.ts`); here are the requests around it. The list is kept current by the
 * environment's feed of browsers, which the view follows.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok } from '@/api/client';
import type { Browser, BrowserProfile, BrowserTicketCreateReply, EnvironmentBrowserCreateBody, EnvironmentBrowsers } from '@/generated/api-types';

export const browserKeys = {
  list: (environmentId: string) => ['environment-browsers', environmentId] as const,
};

/** An incomplete answer is different from a completed discovery that found no browser. */
export class BrowserProgramsMissing extends Error {
  constructor() { super('The browser answer lacks its installed-program information'); this.name = 'BrowserProgramsMissing'; }
}

function checkedBrowsers(value: EnvironmentBrowsers): EnvironmentBrowsers {
  // The transport's static DTO cannot establish the shape of an older or incomplete wire answer.
  const data: unknown = value;
  if (typeof data !== 'object' || data === null || !('support' in data)) throw new BrowserProgramsMissing();
  const support: unknown = data.support;
  if (typeof support !== 'object' || support === null || !('programs' in support) || !Array.isArray(support.programs)) throw new BrowserProgramsMissing();
  return value;
}

/** The browsers of an environment, its profiles, what was found there to run one, and why they are off where they are. */
export const browsersQuery = (environmentId: string) =>
  queryOptions({
    queryKey: browserKeys.list(environmentId),
    queryFn: async ({ signal }): Promise<EnvironmentBrowsers> => checkedBrowsers(await ok(api.GET('/environments/{id}/browsers', { params: { path: { id: environmentId } }, signal }))),
  });

/** The same list, after the environment looked again for what it has to run a browser. */
export async function lookAgain(environmentId: string): Promise<EnvironmentBrowsers> {
  return checkedBrowsers(await ok(api.GET('/environments/{id}/browsers', { params: { path: { id: environmentId }, query: { look: true } } })));
}

/** Opens a browser on a profile, or joins the one that runs on it already; beside a thread, the thread is named. */
export function openBrowser(environmentId: string, body: EnvironmentBrowserCreateBody, threadId: string | null): Promise<Browser> {
  return ok(api.POST('/environments/{id}/browsers', { params: { path: { id: environmentId }, query: threadId === null ? {} : { thread: threadId } }, body }));
}

/** Asks a browser to quit; its profile and what was signed in to in it stay. */
export function quitBrowser(browserId: string): Promise<Browser> {
  return ok(api.DELETE('/browsers/{browser_id}', { params: { path: { browser_id: browserId } } }));
}

/** A single-use ticket for the browser's socket: to watch, or with `control` to drive. */
export function browserTicket(browserId: string, control: boolean): Promise<BrowserTicketCreateReply> {
  return ok(api.POST('/browsers/{browser_id}/ticket', { params: { path: { browser_id: browserId } }, body: { control } }));
}

/** Makes an empty profile. */
export function createProfile(environmentId: string, name: string): Promise<BrowserProfile> {
  return ok(api.POST('/environments/{id}/browser-profiles', { params: { path: { id: environmentId } }, body: { name } }));
}

/** Gives a profile that no browser runs on another name. */
export function renameProfile(environmentId: string, name: string, next: string): Promise<BrowserProfile> {
  return ok(api.PATCH('/environments/{id}/browser-profiles/{name}', { params: { path: { id: environmentId, name } }, body: { name: next } }));
}

/** Deletes a profile and every sign-in made in it; its name, typed, confirms it. */
export function deleteProfile(environmentId: string, name: string): Promise<void> {
  return done(api.DELETE('/environments/{id}/browser-profiles/{name}', { params: { path: { id: environmentId, name }, query: { confirm: name } } }));
}
