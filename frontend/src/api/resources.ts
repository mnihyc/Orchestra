/** API client for Providers by what one owns, may use and manages, with their sign-in and runtime; profiles, hosts with their metrics, machines, templates, MCP servers and mounts.
 *
 * What the screens of resources ask of the API: the machines of a person with their enrollment
 * tokens, their local policy and what is registered from them; the providers, with the models they
 * list, their key, the ways agents may run on them and the threads bound to them; the agent profiles
 * and the harnesses the system has; the hosts and the templates with the drivers that make them.
 * A provider's sign-in through Orch, its managed runtime and its updates, MCP servers and mounts are
 * not served yet and are not asked for here.
 *
 * Every creating request carries the key of the act, so that a second attempt after a lost answer
 * makes nothing twice. A change to something with a revision carries the revision the person saw, and
 * a stale one comes back as `conflict.stale_revision`, which the screen shows and never retries alone.
 */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import { cachedRow } from '@/api/cache';
import { queryClient, type ReadOptions } from '@/api/query';
import { type ResourceRows, resourceRows } from '@/api/listed';
import type {
  Agent,
  ArrangementType,
  CapabilityDef,
  CreateProvider,
  ComponentUpdate,
  DriverInfo,
  EnrollmentToken,
  HarnessKindInfo,
  Host,
  HostEnrollmentTokenCreateBody,
  HostUpdateBody,
  Machine,
  MachineAgentCreateBody,
  MachineEnrollmentTokenCreateBody,
  MachineEnrollmentInfo,
  MachinePolicy,
  MachineRolesChange,
  MachineProviderCreateBody,
  ModelParameters,
  ModelSuggestion,
  Provider,
  ProviderListed,
  ProviderListedPage,
  ProviderFactsOverridesSetBody,
  ProviderListScope,
  ProviderModel,
  ProviderSetup,
  ProviderSourcesSetBody,
  ProviderThreadListReply,
  ProviderUpdateBody,
  ProviderWriteReply,
  ResolvedSetting,
  Runtime,
  Template,
  TemplateCheckBody,
  TemplateCheckReply,
  TemplateCreateBody,
  TemplateUpdateBody,
  UnitKind,
  Upstream,
} from '@/generated/api-types';

/** How many rows a page of a list that pages asks for: what a screen shows before "Show more". */
export const RESOURCE_PAGE_SIZE = 50;

// ---------------------------------------------------------------- keys

export const machinesKey = ['machines'] as const;
export const machineKey = (id: string) => ['machine', id] as const;
export const machinePolicyKey = (id: string) => ['machine-policy', id] as const;
export const providersKey = ['providers'] as const;
export const providerKey = (id: string) => ['provider', id] as const;
export const providerSetupKey = (orgId: string, providerId?: string) => providerId === undefined ? ['provider-setup', orgId] as const : ['provider-setup', orgId, providerId] as const;
export const providerSettingsKey = (id: string) => ['provider-settings', id] as const;
export const providerModelsKey = (id: string) => ['provider-models', id] as const;
export const providerSuggestionsKey = (id: string) => ['provider-suggestions', id] as const;
export const providerThreadsKey = (id: string) => ['provider-threads', id] as const;
export const agentsKey = ['agents'] as const;
export const agentKey = (id: string) => ['agent', id] as const;
export const harnessKindsKey = ['harness-kinds'] as const;
export const hostsKey = ['hosts'] as const;
export const hostKey = (id: string) => ['host', id] as const;
export const hostRuntimesKey = (id: string) => ['host-runtimes', id] as const;
export const templatesKey = ['templates'] as const;
export const templateKey = (id: string) => ['template', id] as const;
export const driversKey = ['drivers'] as const;
export const capabilitiesKey = ['capabilities'] as const;

// ---------------------------------------------------------------- machines

/** The machines the caller owns, revoked ones among them. */
export const machinesQuery = queryOptions({
  queryKey: machinesKey,
  queryFn: ({ signal }): Promise<Machine[]> => ok(api.GET('/machines', { signal })),
});

/** One machine with what it reports and what is registered from it. */
export const machineQuery = (id: string) =>
  queryOptions({
    queryKey: machineKey(id),
    initialData: () => cachedRow(queryClient, 'machine', id),
    queryFn: ({ signal }): Promise<Machine> => ok(api.GET('/machines/{id}', { params: { path: { id } }, signal })),
  });

/** The native acknowledgement precedes a supervised reconnect; active roles arrive over the live row. */
export function setMachineRoles(id:string,roles:readonly string[]):Promise<MachineRolesChange> {
  return ok(api.PUT('/machines/{id}/roles',{params:{path:{id}},body:{roles:[...roles]}}));
}

/** The local policy of a machine as it last reported it, and which keys this caller may change. */
export const machinePolicyQuery = (id: string) =>
  queryOptions({
    queryKey: machinePolicyKey(id),
    queryFn: ({ signal }): Promise<MachinePolicy> =>
      ok(api.GET('/machines/{id}/policy', { params: { path: { id } }, signal })),
  });

/** A single-use token with which a machine enrolls, and the command for each system that uses it. */
export const machineEnrollmentInfoQuery=queryOptions({queryKey:['machine-enrollment-info'],queryFn:({signal}):Promise<MachineEnrollmentInfo>=>
  ok(api.GET('/machines/enrollment-options',{signal}))});

/** A single-use token with which a machine enrolls, issued only on an explicit request. */
export function createMachineToken(key: string, body: MachineEnrollmentTokenCreateBody = {}): Promise<EnrollmentToken> {
  return ok(api.POST('/machines/enrollment-tokens', { body, ...repeatable(key) }));
}

/** A single-use token for a machine that exists, which enrolls it again with its page and what is registered from it. Only its owner may ask. */
export function enrollMachineAgain(id: string, key: string): Promise<EnrollmentToken> {
  return ok(api.POST('/machines/{id}/enrollment-tokens', { params: { path: { id } }, ...repeatable(key) }));
}

export function renameMachine(id: string, name: string): Promise<Machine> {
  return ok(api.PATCH('/machines/{id}', { params: { path: { id } }, body: { name } }));
}

/** Takes the machine out of use at once: its session is dropped and it is refused when it comes back. */
export function revokeMachine(id: string): Promise<Machine> {
  return ok(api.POST('/machines/{id}/revoke', { params: { path: { id } } }));
}

/** Asks the machine what it is and has, again. */
export function probeMachine(id: string): Promise<Machine> {
  return ok(api.POST('/machines/{id}/probe', { params: { path: { id } } }));
}

/** Requests a physical program update. The response retains queued progress or the local refusal. */
export function updateMachineNow(id: string): Promise<ComponentUpdate> {
  return ok(api.POST('/machines/{id}/update', { params: { path: { id } } }));
}

/** The same verified procedure for a node; its hosted turns drain before the executable is swapped. */
export function updateHostNow(id: string): Promise<ComponentUpdate> {
  return ok(api.POST('/hosts/{id}/update', { params: { path: { id } } }));
}

/** Changes keys of the local policy that the machine's owner handed over. Nothing changes if one may not be. */
export function updateMachinePolicy(id: string, values: Record<string, unknown>): Promise<MachinePolicy> {
  return ok(api.PATCH('/machines/{id}/policy', { params: { path: { id } }, body: values }));
}

/** Registers a profile for a harness that is installed on a machine of the caller's. */
export function registerAgent(machineId: string, body: MachineAgentCreateBody, key: string): Promise<Agent> {
  return ok(api.POST('/machines/{id}/agents', { params: { path: { id: machineId } }, body, ...repeatable(key) }));
}

/** Registers a provider whose runtime is the machine: the login its harness holds there, or a key typed there. */
export function registerProvider(machineId: string, body: MachineProviderCreateBody, key: string): Promise<Provider> {
  return ok(api.POST('/machines/{id}/providers', { params: { path: { id: machineId } }, body, ...repeatable(key) }));
}

// ---------------------------------------------------------------- providers

/** The providers of the caller, by what they own, may use or manage; for an org's admin, all that lie in it. */
export const providersQuery = (scope: ProviderListScope, orgId: string | null = null) =>
  queryOptions({
    queryKey: [...providersKey, scope, orgId] as const,
    queryFn: async ({ signal }): Promise<ResourceRows<Provider>> => {
      const all: ProviderListed[] = [];
      const seen = new Set<string>();
      let cursor: string | null = null;
      do {
        const page: ProviderListedPage = await ok(api.GET('/providers', { params: { query: {
          scope, ...(orgId === null ? {} : { org_id: orgId }),
          ...(cursor === null ? {} : { cursor }),
        } }, signal }));
        all.push(...page.items);
        cursor = page.next_cursor;
        if (cursor !== null && seen.has(cursor)) throw new Error('The provider list repeated a cursor.');
        if (cursor !== null) seen.add(cursor);
      } while (cursor !== null);
      return resourceRows<Provider>(all);
    },
  });

export const providerQuery = (id: string, options: ReadOptions = {}) =>
  queryOptions({
    queryKey: providerKey(id),
    initialData: () => cachedRow(queryClient, 'provider', id),
    queryFn: (context): Promise<Provider> => ok(api.GET('/providers/{id}', { params: { path: { id } }, ...(options.keepReading === true ? {} : { signal: context.signal }) })),
  });

/** Everything the page that adds a provider is drawn from, for the org that would be its home. */
export const providerSetupQuery = (orgId: string, providerId?: string) =>
  queryOptions({
    queryKey: providerSetupKey(orgId,providerId),
    queryFn: ({ signal }): Promise<ProviderSetup> =>
      ok(api.GET('/provider-setup', { params: { query: { org_id: orgId,...(providerId === undefined ? {} : {provider_id:providerId}) } }, signal })),
  });

/** The values of the runtime keys in effect for a provider, each with where it comes from. */
export const providerSettingsQuery = (id: string) =>
  queryOptions({
    queryKey: providerSettingsKey(id),
    queryFn: ({ signal }): Promise<ResolvedSetting[]> =>
      ok(api.GET('/providers/{id}/settings', { params: { path: { id } }, signal })),
  });

export const providerModelsQuery = (id: string) =>
  queryOptions({
    queryKey: providerModelsKey(id),
    queryFn: ({ signal }): Promise<ProviderModel[]> =>
      ok(api.GET('/providers/{id}/models', { params: { path: { id } }, signal })),
  });

/** The models the provider's endpoint or harness names that are not listed yet. It fails where the provider cannot be asked. */
export const providerSuggestionsQuery = (id: string) =>
  queryOptions({
    queryKey: providerSuggestionsKey(id),
    queryFn: ({ signal }): Promise<ModelSuggestion[]> =>
      ok(api.GET('/providers/{id}/models/suggestions', { params: { path: { id } }, signal })),
    // A discovery is a question put to the vendor: it is asked when the reader asks, not on every visit.
    staleTime: 5 * 60_000,
    retry: false,
  });

/** The threads bound to a provider that the caller may view, a page at a time. */
export const providerThreadsQuery = (id: string) =>
  infiniteQueryOptions({
    queryKey: providerThreadsKey(id),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }): Promise<ProviderThreadListReply> =>
      ok(
        api.GET('/providers/{id}/threads', {
          params: { path: { id }, query: { limit: RESOURCE_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } },
          signal,
        }),
      ),
    getNextPageParam: (last) => last.next_cursor,
  });

/** Makes a provider on a key, in the org given as its home. */
export function createProvider(body: CreateProvider, key: string): Promise<ProviderWriteReply> {
  return ok(api.POST('/providers', { body, ...repeatable(key) }));
}

/** Changes a provider on the revision the person saw. */
export function updateProvider(id: string, body: ProviderUpdateBody): Promise<ProviderWriteReply> {
  return ok(api.PATCH('/providers/{id}', { params: { path: { id } }, body }));
}

export function deleteProvider(id: string): Promise<void> {
  return done(api.DELETE('/providers/{id}', { params: { path: { id } } }));
}

export function disableProvider(id: string,note?:string): Promise<Provider> {
  return ok(api.POST('/providers/{id}/disable', { params: { path: { id },...(note===undefined?{}:{query:{note}}) } }));
}

export function enableProvider(id: string,runWaiting=false): Promise<Provider> {
  return ok(api.POST('/providers/{id}/enable', { params: { path: { id }, query:{run_waiting:runWaiting} } }));
}

/** Saves the write-only key at its selected keeper and returns the provider after facts/model reads. */
export function setProviderKey(id: string, apiKey: string): Promise<Provider> {
  return ok(api.PUT('/providers/{id}/key', { params: { path: { id } }, body: { api_key: apiKey } }));
}

/** Writes one secret header of an endpoint. The value is never read back. */
export function setProviderSecret(id: string, name: string, value: string): Promise<void> {
  return done(api.PUT('/providers/{id}/secrets/{name}', { params: { path: { id, name } }, body: { value } }));
}

/** Asks the provider's connector for its status and usage again. */
export function refreshProvider(id: string): Promise<Provider> {
  return ok(api.POST('/providers/{id}/refresh', { params: { path: { id } }, body: {} }));
}

/** Writes facts by hand that win over what the harness or endpoint reports. */
export function overrideProviderFacts(id: string, body: ProviderFactsOverridesSetBody): Promise<Provider> {
  return ok(api.PUT('/providers/{id}/facts-overrides', { params: { path: { id } }, body }));
}

/** Chooses the facts/models implementations and writes their secrets; the returned provider contains no secret values. */
export function setProviderSources(id: string, body: ProviderSourcesSetBody): Promise<Provider> {
  return ok(api.PUT('/providers/{id}/sources', { params: { path: { id } }, body }));
}

/** Starts the harness process of a profile on a provider again; sessions are reopened at their next turn. */
export function restartHarness(providerId: string, agentId: string): Promise<Provider> {
  return ok(
    api.POST('/providers/{id}/harnesses/{agent_id}/restart', { params: { path: { id: providerId, agent_id: agentId } } }),
  );
}

export interface NewModel {
  model_name: string;
  display_name?: string;
  parameters?: ModelParameters;
  /** The groups of the provider's own naming that the model is put in. */
  groups?: string[];
}

export function createModel(providerId: string, body: NewModel): Promise<ProviderModel> {
  return ok(api.POST('/providers/{id}/models', { params: { path: { id: providerId } }, body }));
}

/** Changes a model's entry. `parameters` and `groups` are written as a whole: what they leave out is no longer said. */
export function updateModel(
  providerId: string,
  modelId: string,
  body: { display_name?: string; enabled?: boolean; parameters?: ModelParameters; groups?: string[] },
  note?: string,
  runWaiting?: boolean,
): Promise<ProviderModel> {
  return ok(api.PATCH('/providers/{id}/models/{model_id}', { params: { path: { id: providerId, model_id: modelId },query:{...(note===undefined?{}:{note}),...(runWaiting===undefined?{}:{run_waiting:runWaiting})} }, body }));
}

export function deleteModel(providerId: string, modelId: string): Promise<void> {
  return done(api.DELETE('/providers/{id}/models/{model_id}', { params: { path: { id: providerId, model_id: modelId } } }));
}

/** Adds the models a discovery named, each as it describes itself; the speeds, access programs and larger context it lists are kept as offered, and none of them is switched on. */
export async function acceptSuggestions(providerId: string, modelNames: string[]): Promise<ProviderModel[]> {
  const answer=await ok(
    api.POST('/providers/{id}/models/suggestions/accept', {
      params: { path: { id: providerId } },
      body: { model_names: modelNames },
    }),
  );
  return answer.added;
}

/** The arrangement types ticked, which is all a provider's arrangements are written as. */
export function arrangementsBody(types: readonly ArrangementType[]): Pick<ProviderUpdateBody, 'arrangements'> {
  return { arrangements: [...types] };
}

/** The endpoints of an API provider as the request writes them. */
export function upstreamsBody(upstreams: readonly Upstream[]): Pick<ProviderUpdateBody, 'upstreams'> {
  return { upstreams: [...upstreams] };
}

// ---------------------------------------------------------------- agent profiles

/** The profiles the caller may use or manage, the system's among them; for an org, those whose home lies in it. */
export const agentsQuery = (orgId: string | null = null) =>
  queryOptions({
    queryKey: [...agentsKey, orgId] as const,
    queryFn: ({ signal }): Promise<ResourceRows<Agent>> =>
      ok(api.GET('/agents', { params: { query: orgId === null ? {} : { org_id: orgId } }, signal })).then(resourceRows<Agent>),
  });

export const agentQuery = (id: string) =>
  queryOptions({
    queryKey: agentKey(id),
    initialData: () => cachedRow(queryClient, 'agent', id),
    queryFn: ({ signal }): Promise<Agent> => ok(api.GET('/agents/{id}', { params: { path: { id } }, signal })),
  });

/** The harnesses the system has an adapter for, with the builds there are. */
export const harnessKindsQuery = queryOptions({
  queryKey: harnessKindsKey,
  queryFn: ({ signal }): Promise<HarnessKindInfo[]> => ok(api.GET('/harness-kinds', { signal })),
});

/** Makes a profile for a harness in an org. */
export function createAgent(
  orgId: string,
  body: { name: string; harness_kind: string; profile?: unknown },
  key: string,
): Promise<Agent> {
  return ok(api.POST('/orgs/{id}/agents', { params: { path: { id: orgId } }, body: { profile: {}, ...body }, ...repeatable(key) }));
}

export function updateAgent(
  id: string,
  body: { name?: string; profile?: unknown; descriptor_overrides?: unknown; disabled?: boolean; expected_revision: number },
  note?:string,
  runWaiting?:boolean,
): Promise<Agent> {
  return ok(api.PATCH('/agents/{id}', { params: { path: { id },query:{...(note===undefined?{}:{note}),...(runWaiting===undefined?{}:{run_waiting:runWaiting})} }, body }));
}

export function deleteAgent(id: string): Promise<void> {
  return done(api.DELETE('/agents/{id}', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- hosts

/** The hosts the caller may use or manage, the system's among them. */
export const hostsQuery = (orgId: string | null) =>
  queryOptions({
    queryKey: [...hostsKey, orgId] as const,
    queryFn: ({ signal }): Promise<ResourceRows<Host>> =>
      ok(api.GET('/hosts', { params: { query: orgId === null ? {} : { org_id: orgId } }, signal })).then(resourceRows<Host>),
  });

export const hostQuery = (id: string, options: ReadOptions = {}) =>
  queryOptions({
    queryKey: hostKey(id),
    initialData: () => cachedRow(queryClient, 'host', id),
    queryFn: (context): Promise<Host> => ok(api.GET('/hosts/{id}', { params: { path: { id } }, ...(options.keepReading === true ? {} : { signal: context.signal }) })),
  });

/** A single-use token with which a host enrolls; the host will belong to the caller and have the home given. */
export function createHostToken(body: HostEnrollmentTokenCreateBody, key: string): Promise<EnrollmentToken> {
  return ok(api.POST('/hosts/enrollment-tokens', { body, ...repeatable(key) }));
}

export function updateHost(id: string, body: HostUpdateBody): Promise<Host> {
  return ok(api.PATCH('/hosts/{id}', { params: { path: { id } }, body }));
}

/** Everything that runs on a host, whoever's it is, for those who manage the host. */
export const hostRuntimesQuery = (id: string) =>
  queryOptions({
    queryKey: hostRuntimesKey(id),
    queryFn: ({ signal }): Promise<Runtime[]> => ok(api.GET('/hosts/{id}/runtimes', { params: { path: { id } }, signal })),
  });

/** Stops a unit on a host, interrupting the turns that run in it only when asked to. */
export function stopHostRuntime(hostId: string, runtimeId: string, interrupt: boolean): Promise<Runtime> {
  return ok(api.POST('/hosts/{id}/runtimes/{runtime_id}/stop', { params: { path: { id: hostId, runtime_id: runtimeId } }, body: { interrupt } }));
}

/** Takes a unit away from a host after naming it; its volumes go too only when asked to. */
export function removeHostRuntime(hostId: string, runtimeId: string, confirm: string, volumes: boolean): Promise<void> {
  return done(api.DELETE('/hosts/{id}/runtimes/{runtime_id}', { params: { path: { id: hostId, runtime_id: runtimeId }, query: { confirm, ...(volumes ? { volumes } : {}) } } }));
}

// ---------------------------------------------------------------- templates, drivers, capabilities

/** The templates the caller may use or manage, of both kinds unless one is asked for. */
export const templatesQuery = (kind: UnitKind | null, orgId: string | null) =>
  queryOptions({
    queryKey: [...templatesKey, kind, orgId] as const,
    queryFn: ({ signal }): Promise<ResourceRows<Template>> =>
      ok(
        api.GET('/templates', {
          params: { query: { ...(kind === null ? {} : { kind }), ...(orgId === null ? {} : { org_id: orgId }) } },
          signal,
        }),
      ).then(resourceRows<Template>),
  });

export const templateQuery = (id: string) =>
  queryOptions({
    queryKey: templateKey(id),
    initialData: () => cachedRow(queryClient, 'template', id),
    queryFn: ({ signal }): Promise<Template> => ok(api.GET('/templates/{id}', { params: { path: { id } }, signal })),
  });

/** The drivers the system's hosts report, with the descriptor a template's form is drawn from. */
export const driversQuery = queryOptions({
  queryKey: driversKey,
  queryFn: ({ signal }): Promise<DriverInfo[]> => ok(api.GET('/drivers', { signal })),
});

/** What an environment may be granted beyond the ordinary, each with its risk. */
export const capabilitiesQuery = queryOptions({
  queryKey: capabilitiesKey,
  queryFn: ({ signal }): Promise<CapabilityDef[]> => ok(api.GET('/capabilities', { signal })),
  staleTime: Number.POSITIVE_INFINITY,
});

export function createTemplate(body: TemplateCreateBody, key: string): Promise<Template> {
  return ok(api.POST('/templates', { body, ...repeatable(key) }));
}

/** Checks the same configuration rules as saving, without creating a resource. */
export function checkTemplate(body: TemplateCheckBody, signal?: AbortSignal): Promise<TemplateCheckReply> {
  return ok(api.POST('/templates/check', { body, signal }));
}

export function updateTemplate(id: string, body: TemplateUpdateBody): Promise<Template> {
  return ok(api.PATCH('/templates/{id}', { params: { path: { id } }, body }));
}

export function deleteTemplate(id: string): Promise<void> {
  return done(api.DELETE('/templates/{id}', { params: { path: { id } } }));
}

/** Removes a host that nothing runs on, after naming it; its machine is revoked. */
export function deleteHost(id: string, confirm: string): Promise<void> {
  return done(api.DELETE('/hosts/{id}', { params: { path: { id }, query: { confirm } } }));
}
