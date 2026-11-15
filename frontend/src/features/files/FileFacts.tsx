/** Captured native file facts use one shared disclosure on their existing source or file card. */
import { useId, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { ApiError, client } from '@/api/client';
import { fileTransfer, type Door } from '@/api/workspace';
import type { FileSourceFacts, FileSourcePolicy, FileRuleSource, FileTransferFacts, FileTransferReply, NativeImageReceipt, PreviewAllowance } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatDateTime, formatSpan } from '@/lib/time';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { FactDisclosure, FactDisclosureBody, type FactGroup } from '@/ui/FactDisclosure';
import { Input } from '@/ui/Input';
import { ErrorAlert } from '@/ui/ErrorState';

type Format = ReturnType<typeof useFormatContext>;
interface FactsProps {
  facts?: FileSourceFacts | null | undefined;
  asOf?: string | null | undefined;
  showSource?: boolean;
  allowance?: PreviewAllowance | null | undefined;
  imageDelivery?: NativeImageReceipt | null | undefined;
  transfer?: FileTransferFacts | null | undefined;
}

function RuleSources({ sources }: { sources: readonly FileRuleSource[] }) {
  const { t } = useTranslation();
  const [search, setSearch] = useState('');
  const name = (source: FileRuleSource) => source.kind === 'configured' ? t('fileFacts.configuredRules') : source.path ?? t('fileFacts.notShared');
  return <span className="flex min-w-0 max-w-full flex-col gap-1">
    {sources.length > 10 && <Input type="search" label={t('fileFacts.searchRules')} hideLabel value={search} onChange={(event) => setSearch(event.target.value)} />}
    <span className="flex max-h-48 min-w-0 flex-col gap-1 overflow-y-auto">
      {sources.filter((source) => name(source).toLocaleLowerCase().includes(search.toLocaleLowerCase())).map((source, index) => <span key={index} className="wrap-anywhere">{name(source)}</span>)}
      {sources.length === 0 && <span>{t('fileFacts.noLocalRules')}</span>}
    </span>
  </span>;
}

const allowanceKey = (kind: PreviewAllowance['kind']) => ({ decoded_source_bytes: 'files.preview_source_bytes', pixels: 'files.preview_pixels', encoded_preview_bytes: 'files.preview_bytes', viewer_bytes: 'files.viewer_bytes' })[kind];

export function imageDeliveryFragment(receipt: NativeImageReceipt | null | undefined, t: TFunction): string | undefined {
  if (receipt?.applied_delivery !== 'path') return undefined;
  return receipt.native_image_bytes === 0 ? t('fileFacts.imagesUsePaths') : receipt.allowance_exceeded ? t('fileFacts.imageByPath') : undefined;
}

function groupsOf({ facts, asOf, showSource = false, allowance, imageDelivery, transfer }: FactsProps, t: TFunction, format: Format): FactGroup[] {
  const groups: FactGroup[] = [];
  if (showSource || facts !== null && facts !== undefined) {
    const policy = facts?.policy;
    groups.push({ key: 'source', facts: [
      { key: 'kind', label: t('fileFacts.source'), value: policy === undefined ? null : t(policy.kind === 'mount' ? 'fileFacts.readOnlyMount' : policy.kind === 'owned_data' ? 'fileFacts.ownedWhole' : 'fileFacts.workspace') },
      { key: 'exclusions', label: t('fileFacts.workspaceExclusions'), value: policy === undefined ? null : t(policy.workspace_exclusions ? 'fileFacts.applied' : 'fileFacts.wholeRoot') },
      { key: 'writes', label: t('fileFacts.writes'), value: policy === undefined ? null : t(policy.writable ? 'fileFacts.requiresPermission' : 'fileFacts.readOnly') },
      { key: 'observed', label: t('files.panel.observed'), value: (facts?.observed_at ?? asOf) == null ? null : formatDateTime(facts?.observed_at ?? asOf ?? '', format) },
    ] });
    if (policy?.kind === 'workspace' || policy === undefined) groups.push({ key: 'inclusion', title: t('fileFacts.orchExclusions'), facts: [
      { key: 'rules', label: t('fileFacts.ruleSource'), value: facts?.inclusion == null ? null : <RuleSources sources={facts.inclusion.governing_rule_sources} /> },
      { key: 'applies', label: t('fileFacts.appliesTo'), value: facts?.inclusion == null ? null : t('fileFacts.appliesToOrch') },
      { key: 'native', label: t('fileFacts.nativeAccess'), value: facts?.inclusion == null ? null : t('fileFacts.nativeSeparate') },
      { key: 'precedence', label: t('fileFacts.precedence'), value: facts?.inclusion == null ? null : t('fileFacts.deeperRules') },
    ] });
  }
  if (showSource) groups.push({ key: 'watch', facts: [
    { key: 'fallback', label: t('fileFacts.fallbackRefresh'), value: facts?.watch == null ? null : formatSpan(facts.watch.fallback_interval_secs * 1000, format, 'short') },
    { key: 'mode', label: t('fileFacts.watchMode'), value: facts?.watch == null ? null : t(facts.watch.native ? 'fileFacts.nativeNotifications' : 'fileFacts.fallbackOnly') },
    { key: 'scope', label: t('fileFacts.watchScope'), value: facts?.watch == null ? null : t(facts.watch.recursive ? 'fileFacts.wholeTreeWatch' : 'largeTree.openFolders') },
    { key: 'supplier', label: t('fileFacts.source'), value: facts?.watch == null ? null : t('fileFacts.machineConfiguration') },
  ] });
  if (transfer != null) groups.push({ key: 'transfer', facts: [
    { key: 'state', label: t('fileFacts.transfer'), value: t(`fileFacts.transferState.${transfer.state}`) },
    { key: 'last', label: t('fileFacts.lastPiece'), value: transfer.last_piece_at === null ? null : formatDateTime(transfer.last_piece_at, format) },
    { key: 'expires', label: t('fileFacts.transferExpiry'), value: transfer.expires_at === null ? null : formatDateTime(transfer.expires_at, format) },
    { key: 'retention', label: t('fileFacts.idleRetention'), value: transfer.idle_retention_secs === null ? null : formatSpan(transfer.idle_retention_secs * 1000, format, 'short'), source: transfer.retention_source === null ? null : <SourceSettingLink source={transfer.retention_source} def={{ key: 'storage.machine_upload_idle_secs', class: 'policy' }} fallback={t('fileFacts.notShared')} /> },
  ] });
  if (allowance != null) groups.push({ key: 'allowance', facts: [
    { key: 'kind', label: t(`fileFacts.allowanceKind.${allowance.kind}`), value: allowance.kind === 'pixels' ? formatNumber(allowance.limit, format.locale) : formatBytes(allowance.limit, format.locale), source: <SourceSettingLink source={allowance.source} def={{ key: allowanceKey(allowance.kind), class: 'parameter' }} fallback={t('fileFacts.notShared')} /> },
    { key: 'actual', label: t('fileFacts.actual'), value: allowance.actual_if_known === null ? null : allowance.kind === 'pixels' ? formatNumber(allowance.actual_if_known, format.locale) : formatBytes(allowance.actual_if_known, format.locale) },
  ] });
  if (imageDelivery !== undefined) groups.push({ key: 'image-delivery', kind: 'recorded_outcome', facts: [
    { key: 'delivery', label: t('fileFacts.imageDelivery'), value: imageDelivery == null ? null : imageDeliveryFragment(imageDelivery, t) ?? t(`fileFacts.imageRoute.${imageDelivery.applied_delivery}`) },
    { key: 'limit', label: t('fileFacts.nativeImageBytes'), value: imageDelivery == null ? null : formatBytes(imageDelivery.native_image_bytes, format.locale), source: imageDelivery == null ? null : imageDelivery.source === null ? <span>{t('fileFacts.notShared')}</span> : <SourceSettingLink source={imageDelivery.source} def={{ key: 'files.native_image_bytes', class: 'parameter' }} fallback={t('fileFacts.notShared')} /> },
    { key: 'resolved', label: t('fileFacts.resolvedAt'), value: imageDelivery == null ? null : formatDateTime(imageDelivery.resolved_at, format) },
  ] });
  return groups;
}

/** A file card is already open; its body supplies no second explanation opening. */
export function FileFactBody(props: FactsProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const groups = groupsOf(props, t, format);
  return groups.length === 0 ? null : <FactDisclosureBody groups={groups} />;
}

/** A reviewed copy already selected its native source domain; unknown old reviews stay unreported. */
export function FilePolicyDisclosure({ policy }: { policy?: FileSourcePolicy | null | undefined }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const groups = groupsOf({ showSource: true }, t, format).slice(0, 1);
  if (policy != null) {
    groups[0] = { key: 'copy-source', facts: [
      { key: 'source', label: t('fileFacts.source'), value: t(policy.kind === 'mount' ? 'fileFacts.readOnlyMount' : policy.kind === 'owned_data' ? 'fileFacts.ownedWhole' : 'fileFacts.workspace') },
      { key: 'exclusions', label: t('fileFacts.workspaceExclusions'), value: t(policy.workspace_exclusions ? 'fileFacts.applied' : 'fileFacts.wholeRoot') },
      { key: 'writes', label: t('fileFacts.writes'), value: t(policy.writable ? 'fileFacts.requiresPermission' : 'fileFacts.readOnly') },
    ] };
  }
  return <FactDisclosure fragment={policy?.kind === 'owned_data' ? t('fileFacts.ownedWhole') : policy?.kind === 'mount' ? t('fileFacts.readOnlyMount') : undefined} groups={groups} />;
}

/** One source opening reads only its small authorized transfer row, once per explicit opening. */
export function FileSourceDisclosure({ door, readTransfer, ...props }: FactsProps & { door?: Door; readTransfer?: () => Promise<FileTransferReply> }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const [open, setOpen] = useState(false);
  const owner = useId();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const query = useQuery({ queryKey: ['file-source-transfer', owner, reader, door?.environmentId, door?.threadId],
    queryFn: ({ signal }) => { if (readTransfer !== undefined) return readTransfer(); if (door === undefined) throw new Error('No file source'); return fileTransfer(door, signal); },
    enabled: open && (door !== undefined || readTransfer !== undefined), retry: false, gcTime: 0, staleTime: 0,
    refetchOnReconnect: false, refetchOnWindowFocus: false, meta: { fileEnvironment: door?.environmentId ?? null } });
  const privateSource = query.error instanceof ApiError && (query.error.status === 401 || query.error.status === 403);
  const transfer = !query.isFetching && query.error === null ? query.data?.transfer : undefined;
  const source = props.facts?.policy.kind;
  const fragment = privateSource ? undefined : transfer?.state === 'interrupted' && transfer.expires_at !== null
    ? t('fileFacts.transferExpires', { at: formatDateTime(transfer.expires_at, format) }) : source === 'mount' ? t('fileFacts.readOnlyMount') : source === 'owned_data' ? t('fileFacts.ownedWhole') : undefined;
  return <FactDisclosure open={open} onOpenChange={setOpen} fragment={fragment}
    groups={privateSource ? [{ key: 'private', facts: [{ key: 'source', label: t('fileFacts.source'), value: t('fileFacts.notShared') }] }] : groupsOf({ ...props, showSource: true, transfer }, t, format)}>
    {query.error !== null && !privateSource && <ErrorAlert error={query.error} />}
  </FactDisclosure>;
}

/** A viewer refusal has its own affected value; cards use the inline body instead. */
export function PreviewRefusal({ allowance }: { allowance?: PreviewAllowance | null | undefined }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  return <FactDisclosure fragment={t('fileFacts.previewExceeded')} groups={allowance == null ? [{ key: 'allowance', facts: [{ key: 'unknown', label: t('fileFacts.allowance') }] }] : groupsOf({ allowance }, t, format)} />;
}
