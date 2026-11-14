/** Short resource facts keep exact paths available on intent, and the identifiers of runtime changes behind Copy ID. */
import { useTranslation } from 'react-i18next';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { shortRoot } from '@/features/resources/environments/words';
import type { PendingUpdate } from '@/generated/api-types';
import { CopyButton } from '@/ui/CopyButton';
import { CopyId } from '@/ui/CopyId';
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

/** A checksum of a definition: sixty-four hexadecimal digits that say nothing to a person. */
const CHECKSUM = /^[a-f\d]{64}$/iu;
/** A tool's build is its release and the first twelve digits of its checksum, "0.1.3-a789093c891f". The digits tell two
 * builds of one release apart; the release is what a person knows. */
const BUILD_DIGITS = /-[0-9a-f]{12}$/iu;

/** The release a tool build belongs to: "0.1.3-a789093c891f" is release 0.1.3. */
export function releaseOf(version: string): string {
  return version.replace(BUILD_DIGITS, '');
}

const checksums = (change: PendingUpdate): string[] => [change.from, change.to].flatMap((value) => value !== null && CHECKSUM.test(value) ? [value] : []);

export function isTechnicalChange(change: PendingUpdate): boolean {
  return change.name === 'Runtime definition' || [change.from, change.to].some((value) => value !== null && (CHECKSUM.test(value) || value.startsWith('{') || value.startsWith('[')));
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

/** A tool of Orch's own: the release it goes from and to, and a newer build of the same release said as that. */
export function toolChangeText(change: PendingUpdate, t: TFunction): string {
  const name = t(change.what === 'connector' ? 'providers.runtime.tools.connector' : 'providers.runtime.tools.executor');
  const to = releaseOf(change.to ?? '');
  if (change.from === null) return t('providers.runtime.toolAdded', { name, release: to });
  const from = releaseOf(change.from);
  return from === to ? t('providers.runtime.toolRebuilt', { name, release: to }) : t('providers.runtime.update', { name, from, to });
}

/** What a change comes to, in words that name no build, checksum or identifier. */
export function changeText(change: PendingUpdate, t: TFunction): string {
  if (change.what === 'harness') return programChangeText(change, t);
  if (isTechnicalChange(change)) return runtimeChangeText(change, t);
  if (change.what === 'connector' || change.what === 'executor') return toolChangeText(change, t);
  return t('providers.runtime.update', { name: change.name, from: change.from ?? '', to: change.to ?? '' });
}

/** A downgrade is still an available update; its direction is said in words. */
export function programChangeText(change: PendingUpdate, t: TFunction): string {
  const from = change.from ?? '–';
  const to = change.to ?? '–';
  const order = change.from === null || change.to === null ? 0 : compareVersions(change.to, change.from);
  return t('programVersions.change', { name: programNameIn(change, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), from, to, direction: t(`programVersions.${order > 0 ? 'newer' : order < 0 ? 'older' : 'different'}`) });
}

/** What lies behind a change: the checksums of a definition stay behind Copy ID, and a value that is a setting is read as it is. */
export function RuntimeChangeDetails({ change }: { change: PendingUpdate }) {
  const { t } = useTranslation();
  const hashes = checksums(change);
  if (hashes.length > 0) return <CopyId value={hashes.join('\n')} />;
  return <ResourceDetails><dl className="flex flex-col gap-2">{CHANGE_SIDES.map((side) => {
      const value = change[side];
      return value === null ? null : <div key={side} className="flex items-center gap-2"><dt>{t(side === 'from' ? 'trace.before' : 'trace.after')}</dt><dd className="min-w-0 wrap-anywhere"><code>{value}</code><CopyButton value={value} label={t('resourceDetails.copyValue')} /></dd></div>;
  })}</dl></ResourceDetails>;
}

export function RuntimeChange({ change }: { change: PendingUpdate }) {
  const { t } = useTranslation();
  if (!isTechnicalChange(change)) return <>{changeText(change, t)}</>;
  return <div className="flex flex-col gap-1"><span>{changeText(change, t)}</span><RuntimeChangeDetails change={change} /></div>;
}
