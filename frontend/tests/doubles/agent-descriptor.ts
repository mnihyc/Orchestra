/** Complete descriptor defaults for screen fixtures; each case changes only what it reports. */
import type { AgentDescriptor, Capabilities } from '../../src/generated/api-types.ts';

export function agentDescriptor(name: string, fields: Partial<AgentDescriptor> = {}, capabilities: Partial<Capabilities> = {}): AgentDescriptor {
  return {
    kind: 'example', adapter_version: 1,
    harness: { name, version: '1.0', supported_range: '*', status: 'supported' },
    arrangements: [], approval_modes: [], sandbox_modes: [], side_thread_modes: [],
    credentials: { accepts: [], issues: [] }, direct_protocols: [], forwarded_protocols: [],
    native_provider: null, parameters: [], runtime_template: null, ...fields,
    capabilities: {
      agent_terminal: 'none', approvals: [], auxiliary_models: 'uncontrolled', background_commands: false,
      discover_sessions: false, effort: 'none', export_native: false, fork_native: false, import_native: false,
      import_neutral: false, input_images: false, instruction_root_marker: false, interrupt: false,
      mcp: { stdio: false, http: false, reload: false, strict: false }, mcp_discovery: false, memory: 'none',
      model_discovery: false, orch_tools: 'none', phases: false, plan: false, plan_mode: false,
      provider_facts: [], reasoning_summary: false, resume: false, session_reload: 'on_change', skills: 'none',
      steer: false, subagents: false, telemetry_off: false, title: false, usage_context: 'none', usage_tokens: false,
      ...fields.capabilities, ...capabilities,
    },
  };
}
