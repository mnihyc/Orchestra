/** Preview facts stay separate from the native copy's completed setup observations. */
import { skipToken, useQuery, type QueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { CopySetupNote } from '@/api/copySetup';
import type { DuplicateReceipt } from '@/api/duplicates';
import { libraryReader } from '@/api/resourceLibrary';
import type { McpServer, Provider, ProviderListRow } from '@/generated/api-types';
import { holds } from '@/lib/permissions';

export function providerCopySetup(provider: Provider | ProviderListRow): CopySetupNote[] {
  return [
    ...(provider.kind === 'subscription' ? ['sign_in_not_copied' as const] : []),
    ...(!holds(provider.access, 'access.own') ? ['credentials_not_copied' as const] : []),
    ...(('sources' in provider ? [provider.sources.facts, provider.sources.models, provider.sources.presenter].some((source) => source?.address !== null && source?.address !== undefined) : provider.source_consent === true) ? ['source_consent_reset' as const] : []),
  ];
}
export function mcpCopySetup(server: Pick<McpServer, 'access'>): CopySetupNote[] {
  return holds(server.access, 'access.own') ? [] : ['credentials_not_copied'];
}
const key = (kind: string, id: string, version = client.session.getState().readerVersion ?? 0) => ['copy-setup', version, libraryReader(), kind, id] as const;

/** Remember only the accepted server response, under its actual result and reader. No source is reread. */
export function rememberCopySetup(cache: QueryClient, receipt: DuplicateReceipt) {
  if (receipt.kind === 'provider' || receipt.kind === 'mcp_server') cache.setQueryData(key(receipt.kind, receipt.id), { notes: receipt.setup ?? null });
}

export function copySetupDescription(t: TFunction, kind: string, notes?: readonly CopySetupNote[]): string | undefined {
  if (kind !== 'provider' && kind !== 'mcp_server' || notes?.length === 0) return undefined;
  return notes === undefined ? t('duplicate.setup.unknown') : notes.map((note) => t(`duplicate.setup.before.${note}`)).join(' · ');
}

export function CopySetupBefore({ kind, notes }: { kind: string; notes?: readonly CopySetupNote[] }) {
  const { t } = useTranslation();
  const description = copySetupDescription(t, kind, notes);
  return description === undefined ? null : <p data-copy-setup className="text-sm text-text-muted">{description}</p>;
}

/** Historical accepted-result facts, never a current sign-in, readiness or authority claim. */
export function CopySetupResult({ kind, id }: { kind: string; id: string }) {
  const { t } = useTranslation();
  const version = useStore(client.session, (state) => state.readerVersion ?? 0);
  const result = useQuery<{ notes: readonly CopySetupNote[] | null }>({ queryKey: key(kind, id, version), queryFn: skipToken });
  if (result.data === undefined || result.data.notes?.length === 0) return null;
  return <p data-copy-setup className="text-sm text-text-muted">{result.data.notes === null ? t('duplicate.setup.unknown') : result.data.notes.map((note) => t(`duplicate.setup.after.${note}`)).join(' · ')}</p>;
}
