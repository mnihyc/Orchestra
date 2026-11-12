/** Endpoint constraints and body defaults retain their existing independent wire members. */
import { expect, it } from 'vitest';
import { draftOf, newEndpoint, upstreamOf } from '@/features/resources/providers/Endpoints';
import type { ProviderSetupProtocol } from '@/generated/api-types';

const optional = { required: false, choices: null, default: null, help: null, max_bytes: null };
const protocol: ProviderSetupProtocol = { name: 'openai-responses', title: 'Responses', agents: [], endpoint_options: [
  { ...optional, name: 'request_options', label: 'Request options', type: 'json' },
  { ...optional, name: 'params', label: 'Body defaults', type: 'json' },
  { ...optional, name: 'headers', label: 'Headers', type: 'map' },
] };
it('round-trips the same schema while preserving body defaults and unknown header names', () => {
  const schema = { 'openai-responses': { type: 'object', properties: { safety_identifier: false }, 'x-authored': 'keep' }, native: { type: 'object' } };
  const draft = newEndpoint(protocol, 'https://endpoint.example/v1');
  draft.options = { request_options: JSON.stringify(schema), params: JSON.stringify({ store: false }), headers: { 'OpenAI-Project': 'project', 'X-Custom': 'keep' } };
  const { upstream, secrets } = upstreamOf(draft, protocol);
  expect(upstream.options).toEqual({ request_options: schema, params: { store: false }, headers: draft.options.headers });
  expect(secrets).toEqual({});
  expect(draftOf(upstream, protocol).options.request_options).toBe(JSON.stringify(schema, null, 2));
});
it('does not add an undeclared request-options member to an older/custom endpoint declaration', () => {
  const custom: ProviderSetupProtocol = { ...protocol, name: 'custom', endpoint_options: [] };
  const draft = newEndpoint(custom, 'https://custom.example');
  draft.options.request_options = '{"do_not_invent":"a contract"}';
  expect(upstreamOf(draft, custom).upstream.options).toEqual({});
});
