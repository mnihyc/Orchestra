/** Shallow transport observations keep fixture bodies opaque until an assertion inspects them. */
export interface HttpRequest {
  body?: unknown;
  params?: { path?: Record<string, string>; query?: Record<string, unknown> };
  headers?: HeadersInit;
  signal?: AbortSignal;
}
export interface HttpAnswer { data?: unknown; error?: unknown; response: Response }
export type HttpTransport = (path: string, request: HttpRequest) => Promise<HttpAnswer>;

/** A body assertion must fail when no request/object was actually sent. */
export function bodyOf(call: Parameters<HttpTransport> | undefined): Record<string, unknown> {
  const body = call?.[1].body;
  if (body === null || typeof body !== 'object' || Array.isArray(body)) throw new Error('Expected an actual object request body');
  return body as Record<string, unknown>;
}
