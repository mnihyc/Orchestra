/** One own contribution edited in the row; inheritance and the effective answer belong to the row's source/details. */
import { useTranslation } from 'react-i18next';
import { useQuery } from '@tanstack/react-query';
import type { SettingsTarget } from '@/api/settings';
import { nodeQuery } from '@/api/nodes';
import { placePath } from '@/features/navigation/places';
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
  def: Pick<SettingDef, 'merge'> & Partial<Pick<SettingDef, 'key'>>;
  setting: SettingWithParts;
  target: SettingsTarget;
  ignored?: boolean;
  ordered?: boolean;
  onApply?: (value: unknown) => void;
}

export function SettingValue({ def, setting, target, ignored = false, onApply, ...control }: SettingValueProps) {
  return <SettingChoiceContext.Provider value={def.key ?? null}>
    {ignored ? <SettingSummary def={{ ...def, display: control.display }} value={setting.value} />
      : def.key === 'agent.default_provider_id' ? <ProviderDefaultChoice {...control} hideLabel />
        : def.key === EMBEDDINGS_SETTING ? <EmbeddingsChoice {...control} hideLabel />
        : def.key === SEMANTIC_TUNING_SETTING || def.key === WORD_TUNING_SETTING ? <SemanticTuning {...control} hideLabel />
        : def.key !== undefined && isResourceSetting(def.key) ? <ResourceSettingChoice {...control} settingKey={def.key} hideLabel />
          : def.key === SKILLS_SETTING ? <SkillsSettingChoice {...control} hideLabel /> : def.key === PROGRAM_VERSIONS_KEY ? <HarnessVersionsChoice value={control.value} effective={setting.value} onChange={control.onChange} onApply={onApply} onEdited={control.onEdited} disabled={control.disabled} target={target} /> : def.key === 'calls.rules' ? <CallsRules {...control} hideLabel /> : <ValueControl {...control} hideLabel />}
  </SettingChoiceContext.Provider>;
}
