/** The one session-wide switch, with a persistent named title-bar indicator while the reach is on. */
import { useIsMutating, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ShieldUser } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { useCallback } from 'react';
import { adminModeKey, adminModeQuery, setAdminMode } from '@/api/adminMode';
import type { AdminMode } from '@/generated/api-types';
import { meQuery } from '@/api/auth';
import { client } from '@/api/client';
import { Button } from '@/ui/Button';
import { Badge } from '@/ui/Badge';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';
import { Switch } from '@/ui/Switch';
import { useFrameReady } from '@/shell/FrameReadiness';

export function useAdminMode() {
  const ready = useFrameReady();
  const signedIn = useStore(client.session, (session) => session.status === 'signed_in');
  return useQuery({ ...adminModeQuery, enabled: signedIn && ready });
}

/** The destination stays in a system admin's account menu; its page still requires session reach. */
export function useManagementAvailable(): boolean {
  const ready = useFrameReady();
  const me = useQuery({ ...meQuery, enabled: ready });
  return me.data?.is_system_admin === true;
}

export function useAdminModeWrite() {
  const cache = useQueryClient();
  const { t } = useTranslation();
  const sayError = useErrorText();
  const change = useMutation({ mutationKey: ['admin-mode'], mutationFn: setAdminMode,
    onError: (error) => { toast({ tone: 'danger', title: t('adminReach.mode.failed'), description: sayError(error).message }); },
  });
  const changing = useIsMutating({ mutationKey: ['admin-mode'] }) > 0;
  const mutate = change.mutate;
  const setMode = useCallback((enabled: boolean) => {
    const current = cache.getQueryData<AdminMode>(adminModeKey);
    if (current?.available !== true || current.as_admin === enabled || cache.isMutating({ mutationKey: ['admin-mode'] }) > 0) return;
    mutate(enabled);
  }, [cache, mutate]);
  return { change, changing, setMode };
}

/** Management's off-state has one ordinary action; the title bar keeps the sole session switch. */
export function AdminModeEnableAction() {
  const { t } = useTranslation();
  const mode = useAdminMode();
  const { changing, setMode } = useAdminModeWrite();
  if (mode.data?.available !== true || mode.data.as_admin) return null;
  return <Button size="sm" icon={<ShieldUser aria-hidden />} pending={changing} onClick={() => setMode(true)}>{t('adminReach.mode.enable')}</Button>;
}

/** This is a recorded fact too: historical audit rows keep it when the live session is off. */
export function AdminModeStamp({ asAdmin }: { asAdmin: boolean | undefined }) {
  const { t } = useTranslation();
  return asAdmin === true ? <Badge tone="warning">{t('adminReach.mode.name')}</Badge> : null;
}

export function AdminModeControl() {
  const { t } = useTranslation();
  const mode = useAdminMode();
  const sayError = useErrorText();
  const { change, changing, setMode } = useAdminModeWrite();
  if (mode.data?.available !== true) return null;
  const enabled = mode.data.as_admin;
  const label = t('adminReach.mode.label', { state: t(enabled ? 'adminReach.mode.stateOn' : 'adminReach.mode.stateOff') });
  return <Switch
    name={<span className="inline-flex items-center gap-1 text-sm"><ShieldUser aria-hidden />
      <span className="hidden min-[600px]:inline">{label}</span>
      <span aria-hidden className="min-[600px]:hidden">{t(enabled ? 'adminReach.mode.stateOn' : 'adminReach.mode.stateOff')}</span>
    </span>}
    aria-label={label} aria-busy={changing || undefined} aria-disabled={changing || undefined} data-as-admin={enabled}
    checked={enabled} onCheckedChange={setMode} className="shrink-0"
    tooltip={change.error === null ? t('adminReach.mode.description') : sayError(change.error).message}
  />;
}
