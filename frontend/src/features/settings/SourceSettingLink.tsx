/** A short setting-source link uses the served identity and the existing exact-setting destination. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useEffect, useRef } from 'react';
import { nodeQuery } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import { sourceSettingHref } from '@/features/settings/sourceHref';
import { useSourceOrgAddress } from '@/features/settings/sourceAddress';
import { useOpenPath } from '@/features/chat/open';
import type { SettingDef, SettingSource } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

export function SourceSettingLink({ source, def, target, fallback, locked = false, inherited = false, text }: {
  source: SettingSource;
  def: Pick<SettingDef, 'key' | 'class'>;
  target?: SettingsTarget;
  fallback: string;
  locked?: boolean;
  inherited?: boolean;
  /** A related-place reference uses the same canonical source-setting destination with its own name. */
  text?: string;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const open = useOpenPath();
  const sayError = useErrorText();
  const node = source.node;
  const alive = useRef(false);
  const version = useRef(0);
  const targetId = target === undefined || target.kind === 'system' ? '' : target.id;
  useEffect(() => {
    alive.current = true; version.current += 1;
    return () => { alive.current = false; };
  }, [node?.id, node?.redacted, def.key, target?.kind, targetId]);
  const visibleOrg = node?.kind === 'org' && !node.redacted && node.name !== null;
  const address = useSourceOrgAddress(visibleOrg ? node.id : null);
  const resolve = useMutation({ mutationFn: async (request: { version: number; at: string; source: SettingSource; def: Pick<SettingDef, 'key' | 'class'>; target: SettingsTarget | undefined }) => {
    const from = request.source.node;
    if (from?.kind !== 'org' || from.redacted || from.name === null) throw new Error(t('settings.compact.sourceUnavailable'));
    const current = await client.query({ ...nodeQuery(from.id), staleTime: 0 });
    const href = current.kind === 'org' ? sourceSettingHref(request.source, request.def, request.target, current.address) : null;
    if (href === null) throw new Error(t('settings.compact.sourceUnavailable'));
    return href;
  }, onSuccess: (href, request) => { if (alive.current && request.version === version.current && request.at === location.href) open(href); },
    onError: (error, request) => { if (alive.current && request.version === version.current && request.at === location.href) toast({ tone: 'danger', title: sayError(error).message }); },
  });
  if (node !== null && (node.redacted || node.name === null)) return <span>{t('settings.compact.hiddenSource')}</span>;
  const place = node?.name ?? (source.scope === 'system' ? t('settings.editor.from.system') : source.scope === 'default' ? t('settings.editor.from.default') : fallback);
  const href = sourceSettingHref(source, def, target, address);
  const label = text ?? t(locked ? 'settings.compact.lockedOn' : inherited ? 'settings.compact.inheritedFrom' : 'settings.compact.setOn', { place });
  if (href !== null) return <a href={href} className="focus-inset min-w-0 rounded-md wrap-anywhere text-accent-text underline underline-offset-2">{label}</a>;
  return visibleOrg ? <Button role="link" variant="ghost" size="sm" pending={resolve.isPending} className="h-auto min-h-6 min-w-0 max-w-full px-0 text-xs text-accent-text underline underline-offset-2"
    onClick={() => resolve.mutate({ version: version.current, at: location.href, source, def, target })}><span className="min-w-0 text-left wrap-anywhere whitespace-normal">{label}</span></Button> : <span>{label}</span>;
}
