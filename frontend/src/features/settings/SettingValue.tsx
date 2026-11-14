import { McpSettingChoice } from '@/features/settings/McpChoice';
/** One own contribution edited in the row; inheritance and the effective answer belong to the row's source/details. */
import { lazy, Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';
import type { SettingsTarget } from '@/api/settings';
import { nodeQuery } from '@/api/nodes';
import { placePath } from '@/features/navigation/places';
import { OrgTermsControl } from '@/features/settings/OrgTerms';
import { ValueControl, type ValueControlProps } from '@/features/settings/ValueControl';
import { SettingChoiceContext } from '@/features/settings/choiceCopy';
import type { SettingDef, SettingSource, ResolvedSetting as SettingWithParts } from '@/generated/api-types';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { ProviderDefaultChoice } from '@/features/settings/ProviderChoice';
import { EMBEDDINGS_SETTING, EmbeddingsChoice, SEMANTIC_TUNING_SETTING, WORD_TUNING_SETTING, SemanticTuning } from '@/features/settings/EmbeddingsChoice';
import { isResourceSetting, ResourceSettingChoice } from '@/features/settings/ResourceChoice';
import { SKILLS_SETTING, SkillsSettingChoice } from '@/features/settings/SkillsChoice';
import { HarnessVersionsChoice, PROGRAM_VERSIONS_KEY } from '@/features/settings/HarnessVersionsChoice';
import { CallsRules } from '@/features/settings/CallsRules';
import { SummarizerEditor } from '@/features/settings/SummarizerEditor';
import { DocumentSetChoice, isThingSetting, MountSettingChoice, ThingSettingChoice } from '@/features/settings/ThingChoice';
import { SummaryTargetsSetting } from '@/features/settings/SummaryTargetsEditor';
import { AGENT_FEATURES_SETTING, AgentFeaturesChoice } from '@/features/settings/AgentFeaturesChoice';
import { AGENT_MODEL_SETTING, AgentEffortChoice, AgentModelChoice } from '@/features/settings/AgentModelChoice';
import { AGENT_PARAMS_SETTING, AgentParamsEditor } from '@/features/settings/AgentParamsEditor';
import { inheritedTargets } from '@/features/settings/settingParts';
import { AGENT_ACCESS_SETTING, AgentAccessSetting } from '@/features/access/AgentAccessChoice';
import { AskModelsSetting } from './AskModelsSetting';
import { CapabilityBackendChoice } from './CapabilityBackendChoice';
import { isCapabilityBackendSetting } from './capabilityBackendValue';

// The arrangement of the tree reads the live tree of the org: only a page that edits that order loads it.
const NavigationOrderChoice = lazy(() => import('@/features/settings/NavigationOrderChoice').then((module) => ({ default: module.NavigationOrderChoice })));

function OrgSource({ id, name }: { id: string; name: string }) {
  const node = useQuery(nodeQuery(id));
  if (node.data?.kind !== 'org') return <span>{name}</span>;
  return <a href={`/org/${encodeURIComponent(node.data.address)}`} className="rounded-sm text-accent-text underline underline-offset-2">{name}</a>;
}

/** Org screens use their served address; other places use their stable id, and a concealed place never becomes a link. */
export function SourceName({ source }: { source: SettingSource }) {
  const { t } = useTranslation();
  const node = source.node;
  if (node === null) return <span>{t(source.scope === 'default' ? 'settings.editor.from.default' : source.scope === 'system' ? 'settings.editor.from.system' : source.scope === 'host_own' ? 'settings.editor.from.host' : 'settings.editor.from.environment')}</span>;
  if (node.redacted || node.name === null) return <span>{t('access.place.hidden')}</span>;
  if (node.kind === 'org') return <OrgSource id={node.id} name={node.name} />;
  return <a href={placePath(node)} className="rounded-sm text-accent-text underline underline-offset-2">{node.name}</a>;
}

export interface SettingValueProps extends ValueControlProps {
  def: Pick<SettingDef, 'merge'> & Partial<Pick<SettingDef, 'key' | 'default'>>;
  setting: SettingWithParts;
  target: SettingsTarget;
  ignored?: boolean;
  ordered?: boolean;
  onApply?: (value: unknown) => void;
  onClear?: () => void;
  scope?: string | null;
}

export function SettingValue({ def, setting, target, ignored = false, onApply, onClear, scope, ...control }: SettingValueProps) {
  return <SettingChoiceContext.Provider value={def.key ?? null}>
    {def.key === 'ask.models' ? <AskModelsSetting key={`${target.kind}:${target.kind === 'system' ? '' : target.id}`} {...control} disabled={control.disabled === true || ignored} setting={setting} target={target} />
      : ignored ? <SettingSummary def={{ ...def, display: control.display }} value={setting.value} target={target} />
      : def.key !== undefined && isCapabilityBackendSetting(def.key) ? <CapabilityBackendChoice {...control} settingKey={def.key} target={target} />
      : def.key === 'org.terms' ? <OrgTermsControl {...control} setting={setting} target={target} hideLabel />
      : def.key === AGENT_ACCESS_SETTING ? <AgentAccessSetting {...control} hideLabel />
      : def.key === 'agent.default_provider_id' ? <ProviderDefaultChoice {...control} hideLabel />
      : def.key !== undefined && isThingSetting(def.key) ? <ThingSettingChoice {...control} settingKey={def.key} onClear={onClear} hideLabel />
      : def.key === 'env.mounts' ? <MountSettingChoice {...control} hideLabel />
      : def.key === 'docs.templates' ? <DocumentSetChoice {...control} hideLabel />
      : def.key === 'auto.summarizer' ? <SummarizerEditor {...control} hideLabel />
      : def.key === AGENT_MODEL_SETTING ? <AgentModelChoice {...control} target={target} hideLabel />
      : def.key === 'agent.effort' ? <AgentEffortChoice {...control} target={target} hideLabel />
      : def.key === AGENT_PARAMS_SETTING ? <AgentParamsEditor {...control} target={target} setting={setting} scope={scope} hideLabel />
      : def.key === AGENT_FEATURES_SETTING ? <AgentFeaturesChoice {...control} defaults={def.default} inherited={inheritedTargets(setting, target)} hideLabel />
      : def.key === 'navigation.order' ? <Suspense fallback={null}><NavigationOrderChoice {...control} target={target} hideLabel /></Suspense>
      : def.key === 'summary.targets' ? <SummaryTargetsSetting {...control} target={target} defaults={def.default} inherited={inheritedTargets(setting, target)} hideLabel />
        : def.key === EMBEDDINGS_SETTING ? <EmbeddingsChoice {...control} hideLabel />
        : def.key === SEMANTIC_TUNING_SETTING || def.key === WORD_TUNING_SETTING ? <SemanticTuning {...control} hideLabel />
        : def.key !== undefined && isResourceSetting(def.key) ? <ResourceSettingChoice {...control} settingKey={def.key} hideLabel />
          : def.key === 'mcp.enabled' ? <McpSettingChoice {...control} effective={setting.value} environment={target.kind==='environment'?target.id:null} hideLabel />
          : def.key === SKILLS_SETTING ? <SkillsSettingChoice {...control} hideLabel /> : def.key === PROGRAM_VERSIONS_KEY ? <HarnessVersionsChoice value={control.value} effective={setting.value} onChange={control.onChange} onApply={onApply} onEdited={control.onEdited} disabled={control.disabled} target={target} /> : def.key === 'calls.rules' ? <CallsRules {...control} hideLabel /> : <ValueControl {...control} hideLabel />}
  </SettingChoiceContext.Provider>;
}
