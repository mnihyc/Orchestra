/** The sections of a resource's page, as the address names them: read by the routes before the pages themselves are fetched.
 *
 * A route checks the address's `tab` when it is entered, which is before its screen's code arrives; keeping the names
 * here lets it do so without the screen (a template's brings the code editor with it).
 */

/** The sections of a template's page. */
export type TemplateTab = 'overview' | 'spec' | 'access' | 'danger';

export const TEMPLATE_TABS: readonly TemplateTab[] = ['overview', 'spec', 'access', 'danger'];

/** The sections a provider's page has had; old ones still lead to where their content is now. */
export type ProviderTab = 'overview' | 'runtime' | 'key' | 'models' | 'arrangements' | 'threads' | 'sessions' | 'settings' | 'access' | 'danger' | 'usage' | 'activity';

export const PROVIDER_TABS: readonly ProviderTab[] = ['overview', 'models', 'key', 'usage', 'settings', 'activity'];

/** Old bookmarks keep their destination's content within the six current sections. */
export function providerTab(value: unknown): ProviderTab | undefined {
  if (value === 'runtime' || value === 'arrangements' || value === 'access' || value === 'danger') return 'settings';
  if (value === 'threads' || value === 'sessions') return 'activity';
  return PROVIDER_TABS.find((tab) => tab === value);
}

/** The sections of a host's page. */
export type HostTab = 'overview' | 'runtimes' | 'storage' | 'settings' | 'policy' | 'ports' | 'access';

export const HOST_TABS: readonly HostTab[] = ['overview', 'runtimes', 'storage', 'settings', 'policy', 'ports', 'access'];

/** The sections of an environment's page. */
export type EnvironmentTab = 'overview' | 'threads' | 'files' | 'terminal' | 'ports' | 'browsers' | 'vnc' | 'tools' | 'limits' | 'trust' | 'controls' | 'access' | 'settings' | 'danger';

export const ENVIRONMENT_TABS: readonly EnvironmentTab[] = ['overview', 'threads', 'files', 'tools', 'terminal', 'ports', 'browsers', 'vnc', 'limits', 'trust', 'controls', 'access', 'settings', 'danger'];

/** The sections of an agent's page. */
export type AgentTab = 'overview' | 'parameters' | 'profile' | 'access';

export const AGENT_TABS: readonly AgentTab[] = ['overview', 'parameters', 'profile', 'access'];

/** The sections of a mount's page. */
export type MountTab = 'overview' | 'locations' | 'uses' | 'settings' | 'access';

export const MOUNT_TABS: readonly MountTab[] = ['overview', 'locations', 'uses', 'settings', 'access'];

/** The sections of an MCP server's page. */
export type McpTab = 'overview' | 'settings' | 'access';

export const MCP_TABS: readonly McpTab[] = ['overview', 'settings', 'access'];
