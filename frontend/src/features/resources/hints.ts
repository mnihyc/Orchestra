/** The forms of the resources screens that hold an option which is not plain, and the hint that stands beside each option, by the form it stands in.
 *
 * Related options can share one named guide entry; a standalone option can have its own hint.
 * This is the list of those standalone hints,
 * kept as data so that a test can open each form and name the options that have none. The words of a hint
 * are in the catalogue under its id (`help.hints.<id>`: its term, what it is and, where there is a choice,
 * which one to take by what the reader wants). An option that a descriptor declares brings the `help` text
 * the descriptor gives, and one that gives none has no "?": those are not in this list.
 */

import { TEMPLATE_GUIDE } from '@/help/templates';

/** Which forms there are, and the hints in each, in the order they are drawn. */
export const HINTED_FORMS = {
  /** The dialog that adds a provider, on an API key. */
  'add-provider': ['provider-kind', 'endpoint-format', 'key-place', 'home', 'who-may-use', 'arrangement-separated', 'arrangement-key_separated'],
  /** A provider's Key tab, where its endpoints are edited. */
  'provider-key': ['endpoint-format'],
  /** A provider's How agents run tab. */
  'provider-arrangements': ['arrangement-separated', 'arrangement-key_separated', 'arrangement-credential_shared', 'arrangement-handed_over', 'arrangement-together'],
  /** The editor of one model of a provider. */
  'provider-model': ['model-efforts', 'model-speeds', 'model-programs', 'model-context', 'model-rate'],
  /** The dialog that registers a provider from a machine. */
  'register-provider': ['machine-provider-source', 'home'],
  /** The dialog that registers an agent profile from a machine. */
  'register-agent': ['home'],
  /** The dialog that registers a directory of a machine as an environment. */
  'register-environment': ['env-root', 'env-remote-write', 'env-terminal', 'home', 'who-may-use'],
  /** The dialog that makes an environment from a template. */
  'make-environment': ['env-template', 'env-host', 'env-runtime', 'who-may-use'],
  /** An environment's page, where its switches are. */
  'environment-switches': ['env-remote-write', 'env-terminal'],
  /** The dialog that edits an environment. */
  'edit-environment': ['env-remote-write', 'env-terminal'],
  /** The dialog that makes a template. */
  'new-template': ['home'],
  /** The dialog that adds a host. */
  'add-host': ['home', 'who-may-use'],
  /** A host's Settings tab. */
  'host-settings': ['host-schedulable', 'host-when-full', 'host-labels'],
  /** The dialog that edits a host. */
  'edit-host': ['host-schedulable', 'host-when-full', 'host-labels'],
  /** The dialog that removes a unit from a host. */
  'host-remove-runtime': ['host-volumes'],
  /** The dialog that adds a machine. */
  'add-machine': ['machine-delegate'],
  /** An agent profile's Profile tab. */
  'agent-profile': ['agent-pin', 'agent-profile'],
  /** The dialog that edits an agent profile. */
  'edit-agent': ['agent-pin'],
  /** The dialog that adds an agent profile. */
  'add-agent': ['agent-harness'],
} as const satisfies Record<string, readonly string[]>;

export type HintedForm = keyof typeof HINTED_FORMS;

/** Every hint of these screens. */
export type ResourceHintId = (typeof HINTED_FORMS)[HintedForm][number];

/** The actual guide destination is also used by the browser walk of these forms. */
export function resourceHintGuide(id: string): { id: string; heading: string | null } {
  if (id === 'machine-delegate') return { id: 'connect-a-machine', heading: null };
  if (id.startsWith('template-')) return { id: TEMPLATE_GUIDE.id, heading: null };
  const heading = id.startsWith('arrangement-') ? 'how-agents-run'
    : id.startsWith('env-') ? 'environments-and-their-doors'
      : id === 'host-volumes' ? 'templates-and-updates'
        : id.startsWith('agent-') || id.startsWith('host-') || id.startsWith('machine-') ? 'profiles-hosts-and-computers'
          : 'providers-and-models';
  return { id: 'resources', heading };
}

/** The settings that have words of their own, by the key the server names them with: what each is, written under `help.settings.<key>`. A setting that is not here is shown by its key alone, without a "?". */
export const SETTING_HINTS = [
  'runtime.idle_stop_minutes',
  'runtime.template_id',
  'runtime.host_id',
  'runtime.resources',
  'runtime.update',
  'runtime.renew_days',
  'env.update',
  'env.template_id',
  'env.host_id',
  'env.storage',
  'env.mounts',
  'env.resources',
  'env.needs',
  'env.idle_stop_minutes',
  'env.idle_warning_minutes',
  'env.unused_remove_days',
  'env.variables',
  'env.secret_names',
  'mcp.enabled',
  'exec.offers',
  'exec.orch_tools',
  'workspace.ignore',
  'workspace.index',
  'capability.recreate_grace_secs',
  'switch.active_turn',
  'switch.wait_minutes',
  'switch.verify_timeout_secs',
  'switch.keep_previous',
] as const;

export type SettingHintKey = (typeof SETTING_HINTS)[number];

/** Whether a setting has words of its own. */
export function hasSettingHint(key: string): key is SettingHintKey {
  return (SETTING_HINTS as readonly string[]).includes(key);
}
