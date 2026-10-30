/** Real tutorial targets. Source evidence is checked with the shipped content; selectors contain no person's identifier. */
export interface TargetEvidence { source: string; marker: string }
export interface TutorialTarget { selector: string; evidence: readonly TargetEvidence[] }

export const TARGETS = {
  'new-thread': { selector: '[data-tutorial-target="new-thread"]', evidence: [{ source: 'features/topic/Threads.tsx', marker: 'data-tutorial-target="new-thread"' }] },
  composer: { selector: '[data-composer] textarea', evidence: [{ source: 'features/composer/Composer.tsx', marker: 'data-composer' }] },
  conversation: { selector: '[data-screen="thread"] [role="log"]', evidence: [{ source: 'features/thread/Thread.tsx', marker: 'data-screen="thread"' }, { source: 'ui/VirtualList.tsx', marker: 'role="log"' }] },
  stop: { selector: '[data-tutorial-target="thread-stop"]', evidence: [{ source: 'features/composer/StopControl.tsx', marker: 'data-tutorial-target="thread-stop"' }] },
  permissions: { selector: '[data-tutorial-target="permission-page"]', evidence: [{ source: 'features/access/AccessDialog.tsx', marker: 'data-tutorial-target="permission-page"' }] },
  holders: { selector: '[data-tutorial-target="holder-picker"]', evidence: [{ source: 'features/access/AddHolders.tsx', marker: 'data-tutorial-target="holder-picker"' }] },
  machines: { selector: '[data-resource-kind="machines"]', evidence: [{ source: 'features/resources/ResourceList.tsx', marker: 'data-resource-kind={kind}' }, { source: 'features/resources/machines/MachineList.tsx', marker: 'kind="machines"' }] },
  installer: { selector: '[data-tutorial-target="machine-enrollment"]', evidence: [{ source: 'features/resources/machines/Enrollment.tsx', marker: 'data-tutorial-target="machine-enrollment"' }] },
  'machine-policy': { selector: '[data-tutorial-target="machine-policy"]', evidence: [{ source: 'features/resources/machines/Policy.tsx', marker: 'data-tutorial-target="machine-policy"' }] },
  providers: { selector: '[data-resource-kind="providers"]', evidence: [{ source: 'features/resources/ResourceList.tsx', marker: 'data-resource-kind={kind}' }, { source: 'features/resources/providers/ProviderList.tsx', marker: 'kind="providers"' }] },
  'provider-form': { selector: '[data-tutorial-target="provider-form"]', evidence: [{ source: 'features/resources/providers/AddProvider.tsx', marker: 'data-tutorial-target="provider-form"' }] },
  'provider-models': { selector: '#provider-models', evidence: [{ source: 'features/resources/providers/Models.tsx', marker: 'id="provider-models"' }] },
  code: { selector: '[data-testid="code-status"]', evidence: [{ source: 'features/files/viewers/Code.tsx', marker: 'data-testid="code-status"' }] },
  document: { selector: '[data-screen="document"] [data-document-scroll] article', evidence: [{ source: 'features/documents/Document.tsx', marker: 'data-document-scroll' }] },
  'document-save': { selector: '[data-screen="document"] [data-save]', evidence: [{ source: 'features/documents/Document.tsx', marker: 'data-save={save.phase}' }] },
  knowledge: { selector: '[data-testid="thread-knowledge"]', evidence: [{ source: 'features/inspector/Knowledge.tsx', marker: 'data-testid="thread-knowledge"' }] },
  usage: { selector: '#own-usage', evidence: [{ source: 'features/usage/Mine.tsx', marker: 'id="own-usage"' }] },
  limits: { selector: '#own-allowances', evidence: [{ source: 'features/usage/Mine.tsx', marker: 'id="own-allowances"' }] },
  'thread-usage': { selector: '[data-tutorial-target="thread-usage"]', evidence: [{ source: 'features/usage/ThreadUsage.tsx', marker: 'data-tutorial-target="thread-usage"' }] },
  parameters: { selector: '[data-testid="parameters-panel"]', evidence: [{ source: 'features/inspector/Parameters.tsx', marker: 'data-testid="parameters-panel"' }] },
  approval: { selector: '[data-tutorial-target="approval-card"]', evidence: [{ source: 'features/approvals/Card.tsx', marker: 'data-tutorial-target="approval-card"' }] },
  capability: { selector: '[data-kind="capability"]', evidence: [{ source: 'features/approvals/Card.tsx', marker: 'data-kind={approval.kind}' }, { source: 'features/approvals/Capability.tsx', marker: '<CardFrame approval={approval}' }] },
  rules: { selector: '[data-approval-rules]', evidence: [{ source: 'features/approvals/ThreadRules.tsx', marker: 'data-approval-rules' }] },
  ports: { selector: '[data-testid="ports-panel"]', evidence: [{ source: 'features/ports/Ports.tsx', marker: 'data-testid="ports-panel"' }] },
  grants: { selector: '[data-testid="port-grants"]', evidence: [{ source: 'features/ports/Grants.tsx', marker: 'data-testid="port-grants"' }] },
  template: { selector: '#template-spec', evidence: [{ source: 'features/resources/templates/Template.tsx', marker: 'id="template-spec"' }] },
  policy: { selector: '[data-setting="capability.policy"]', evidence: [{ source: 'features/settings/SettingRow.tsx', marker: 'settingKey: def.key' }, { source: 'ui/SettingRow.tsx', marker: 'data-setting={settingKey}' }] },
  network: { selector: '[data-testid="environment-network"]', evidence: [{ source: 'features/resources/environments/Network.tsx', marker: 'data-testid="environment-network"' }] },
  'environment-settings': { selector: '#environment-settings', evidence: [{ source: 'features/resources/environments/Settings.tsx', marker: 'id="environment-settings"' }] },
} as const satisfies Record<string, TutorialTarget>;

export type TargetId = keyof typeof TARGETS;
