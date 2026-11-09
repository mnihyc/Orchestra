/** One typed report fixture for the same boundary on a machine and an environment. */
import type { EnvironmentRef, EnvironmentSandboxReadiness, MachineRef } from '../../src/generated/api-types.ts';

export function sandboxReadiness(environment: EnvironmentRef = {
  id: 'environment', name: 'Acme workspace', placement: 'connected', status: 'running', ports_exposed: false, address: 'ada/work',
  owner: { id: 'ada', username: 'ada', display_name: 'Ada', kind: 'human', avatar: { text: 'A', color: 1 } },
}, machine: MachineRef | null = { id: 'machine', name: 'world-box', placement: 'connected', platform_os: 'linux', online: true }): EnvironmentSandboxReadiness {
  return { environment, machine, policy_manage: true, sandbox: {
    executor_ref: { kind: 'orch-exec', title: { en: 'Orch tools', 'zh-CN': 'Orch 工具' }, tool_set: null },
    ready: false, requested: 'open', widest: 'workspace', source: 'machine_policy', machine_limit: 'workspace', template_limit: null,
    executor: 'orch-exec', enforceable: ['open'], agent_modes: [], reason: null,
  } };
}
