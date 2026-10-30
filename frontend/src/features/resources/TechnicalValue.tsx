/** Short resource facts keep exact paths and runtime fingerprints available on intent. */
import { useTranslation } from 'react-i18next';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { shortRoot } from '@/features/resources/environments/words';
import type { PendingUpdate } from '@/generated/api-types';
import { CopyButton } from '@/ui/CopyButton';
import { Tooltip } from '@/ui/Tooltip';
import { commandLine } from '@/lib/commandLine';
import type { TFunction } from 'i18next';
import { compareVersions } from '@/lib/versions';
import { programNameIn } from '@/features/resources/programNames';
import { i18n } from '@/i18n';

const CHANGE_SIDES = ['from', 'to'] as const;

export function PathValue({ path }: { path: string }) {
  const { t } = useTranslation();
  return <span className="inline-flex min-w-0 items-center gap-1">
    <Tooltip content={path}><code tabIndex={0} className="min-w-0 truncate rounded-sm text-sm">{shortRoot(path)}</code></Tooltip>
    <CopyButton value={path} label={t('resourceDetails.copyPath')} />
  </span>;
}

export function CommandValue({ command }: { command: readonly string[] }) {
  const { t } = useTranslation();
  const exact = commandLine(command);
  return <span className="inline-flex min-w-0 items-center gap-1">
    <Tooltip content={exact}><code tabIndex={0} className="min-w-0 truncate rounded-sm text-sm">{shortRoot(command[0] ?? '')}</code></Tooltip>
    <CopyButton value={exact} label={t('resourceDetails.copyCommand')} />
  </span>;
}

export function isTechnicalChange(change: PendingUpdate): boolean {
  return change.name === 'Runtime definition' || [change.from, change.to].some((value) => value !== null && (/^[a-f\d]{64}$/iu.test(value) || value.startsWith('{') || value.startsWith('[')));
}

export function runtimeChangeText(change: PendingUpdate, t: TFunction): string {
  if (change.what === 'harness') return programChangeText(change, t);
  if (change.name === 'Runtime definition') return t('resourceDetails.definitionChanged');
  if (change.name === 'Resource limits') return t('resourceDetails.resourceLimitsChanged');
  if (change.name === 'Workspace and home' || change.name === 'Harness home') return t('resourceDetails.homeChanged');
  if (change.name === 'Mount attachments') return t('resourceDetails.mountsChanged');
  if (change.name === 'Storage') return t('resourceDetails.storageChanged');
  return t('resourceDetails.namedChanged', { name: change.name });
}

/** A downgrade is still an available update; its direction is said in words. */
export function programChangeText(change: PendingUpdate, t: TFunction): string {
  const from = change.from ?? '–';
  const to = change.to ?? '–';
  const order = change.from === null || change.to === null ? 0 : compareVersions(change.to, change.from);
  return t('programVersions.change', { name: programNameIn(change, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), from, to, direction: t(`programVersions.${order > 0 ? 'newer' : order < 0 ? 'older' : 'different'}`) });
}

export function RuntimeChangeDetails({ change }: { change: PendingUpdate }) {
  const { t } = useTranslation();
  return <ResourceDetails><dl className="flex flex-col gap-2">{CHANGE_SIDES.map((side) => {
      const value = change[side];
      return value === null ? null : <div key={side} className="flex items-center gap-2"><dt>{t(side === 'from' ? 'trace.before' : 'trace.after')}</dt><dd className="min-w-0 wrap-anywhere"><code>{value}</code><CopyButton value={value} label={t('resourceDetails.copyId')} /></dd></div>;
  })}</dl></ResourceDetails>;
}

export function RuntimeChange({ change }: { change: PendingUpdate }) {
  const { t } = useTranslation();
  if (change.what === 'harness') return <>{programChangeText(change, t)}</>;
  if (!isTechnicalChange(change)) return <>{t('providers.runtime.update', { name: change.name, from: change.from ?? '', to: change.to ?? '' })}</>;
  return <div className="flex flex-col gap-1"><span>{runtimeChangeText(change, t)}</span><RuntimeChangeDetails change={change} /></div>;
}
