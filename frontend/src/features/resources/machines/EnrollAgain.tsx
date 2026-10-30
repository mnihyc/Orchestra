/** The dialog that enrolls a machine again: the command to run on that machine, what stays (its name, its page, what is registered from it) and what goes (its old certificate), and the wait for the machine to come back.
 *
 * For a machine whose certificate lapsed past renewing, was revoked, or was lost with its disk. A token is made
 * for that machine alone as the dialog opens; the command that uses it gives the machine a new certificate and
 * it is the same machine afterwards, so nothing registered from it is made again. A certificate of it that still
 * stood is revoked when the new one is first used, so that the old disk, if somebody has it, is out. The dialog
 * asks for the machine every few seconds and says when its certificate is new.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { LoaderCircle } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { enrollMachineAgain, machineEnrollmentInfoQuery, machineKey, machineQuery, machinesKey } from '@/api/resources';
import { CommandBox } from '@/features/resources/Facts';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { OS_NAMES } from '@/features/resources/machines/facts';
import type { Machine, MachineOs } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { MINUTE_MS, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tabs } from '@/ui/Tabs';

/** How much is registered from a machine: what stays with it when it is enrolled again. */
export function registeredCount(machine: Pick<Machine, 'agents' | 'providers' | 'environments'>): number {
  return machine.agents.length + machine.providers.length + machine.environments.length;
}

/** Whether a machine needs to be enrolled again: it was revoked, or its certificate has ended. */
export function lostEnrollment(machine: Pick<Machine, 'status' | 'cert_not_after'>, now: number): 'revoked' | 'lapsed' | null {
  if (machine.status === 'revoked') return 'revoked';
  return Date.parse(machine.cert_not_after) <= now ? 'lapsed' : null;
}

export function EnrollAgainDialog({ machine, onClose }: { machine: Machine; onClose: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const now = useNow();
  const [os, setOs] = useState<MachineOs>(machine.platform_os);
  // How the machine stood as the dialog opened: a certificate that is not this one is the new one.
  const [before] = useState({ ends: machine.cert_not_after, status: machine.status });

  const info = useQuery(machineEnrollmentInfoQuery);
  const mayShow = info.data?.secure === true && !info.isError;
  const node = machine.roles.includes('node');
  const make = useMutation({ mutationFn: () => enrollMachineAgain(machine.id, newActKey()) });
  const made = make.data;

  // The token is made as the dialog opens, once.
  const started = useRef(false);
  useEffect(() => {
    if (started.current || !mayShow) return;
    started.current = true;
    make.mutate();
    // `make` is a new object at every render; the dialog's opening is what decides.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mayShow]);

  const expiresAt = made === undefined ? null : Date.parse(made.expires_at);
  const expired = expiresAt !== null && expiresAt <= now;
  const watch = useQuery({ ...machineQuery(machine.id), enabled: made !== undefined });
  const current = watch.data;
  // A new certificate ends later than the one the machine had; a machine that was revoked is well again as soon as it is active.
  const enrolled = current?.status === 'active' && (before.status !== 'active' || Date.parse(current.cert_not_after) > Date.parse(before.ends));
  useEffect(() => {
    if (!enrolled) return;
    void queryClient.invalidateQueries({ queryKey: machinesKey });
    void queryClient.invalidateQueries({ queryKey: machineKey(machine.id) });
  }, [enrolled, machine.id, queryClient]);

  // Whole minutes are counted down, so that a reading is never more than there is.
  const minutesLeft = expiresAt === null ? 0 : expiresAt <= now ? 0 : Math.max(1, Math.floor((expiresAt - now) / MINUTE_MS));
  const registered = registeredCount(machine);

  return (
    <Dialog
      open
      onOpenChange={(next) => !next && onClose()}
      size="lg"
      title={t('machines.again.title', { name: machine.name })}
      description={t('machines.again.lead', { name: machine.name })}
      footer={
        <><TaskGuideLink guide="connect-a-machine" heading={t('machineInstallation.againHeading')} /><Button variant={enrolled ? 'primary' : 'secondary'} onClick={onClose}>
          {enrolled ? t('machines.add.done') : t('common.close')}
        </Button></>
      }
    >
      <div className="flex flex-col gap-4 py-2">
        <div className="grid gap-4 sm:grid-cols-2">
          <section aria-labelledby="again-stays" className="flex flex-col gap-1">
            <h3 id="again-stays" className="text-sm font-medium text-text">
              {t('machines.again.stays')}
            </h3>
            <ul className="list-disc pl-6 text-sm text-text-muted">
              <li>{t('machines.again.staysName', { name: machine.name })}</li>
              <li>{t('machines.again.staysRegistered', { count: registered })}</li>
            </ul>
          </section>
          <section aria-labelledby="again-goes" className="flex flex-col gap-1">
            <h3 id="again-goes" className="text-sm font-medium text-text">
              {t('machines.again.goes')}
            </h3>
            <ul className="list-disc pl-6 text-sm text-text-muted">
              <li>{machine.status === 'revoked' ? t('machines.again.goesRevoked') : t('machines.again.goesOld')}</li>
            </ul>
          </section>
        </div>
        {info.isError ? <ErrorAlert error={info.error} /> : info.data?.secure === false ? (
          <InlineAlert tone="warning">{t('machineInstallation.configureAddress', {setting:info.data.configure ?? 'public_url'})}</InlineAlert>
        ) : make.isError ? (
          <ErrorAlert error={make.error} />
        ) : made === undefined || !mayShow ? (
          <div aria-busy="true">
            <SkeletonRows rows={3} />
          </div>
        ) : watch.isError ? <ErrorAlert error={watch.error} /> : enrolled ? (
          <InlineAlert tone="success" title={t('machines.again.done', { name: machine.name })}>
            {current.online ? t('machines.add.arrivedOnline') : t('machines.add.arrivedEnrolled')}
          </InlineAlert>
        ) : (
          <>
            {!expired && (node ? <CommandBox command={made.commands.linux} label={t('machines.add.copy')} /> : <Tabs
              label={t('machines.add.systems')}
              value={os}
              onValueChange={setOs}
              tabs={(['linux', 'macos', 'windows'] as const).map((value) => ({
                value,
                label: OS_NAMES[value],
                panel: (
                  <div className="flex flex-col gap-2 pt-3">
                    <p className="text-sm text-text-muted">{t(`machines.add.run.${value}`)}</p>
                    <CommandBox command={made.commands[value]} label={t('machines.add.copy')} />
                  </div>
                ),
              }))}
            />)}
            <p className="text-sm text-text-muted">{expired ? t('machines.add.expired') : t('machines.add.validFor', { count: minutesLeft })}</p>
            {expired ? (
              <div>
                <Button pending={make.isPending} onClick={() => make.mutate()}>
                  {t('machines.add.again')}
                </Button>
              </div>
            ) : (
              <p role="status" className="flex items-center gap-2 text-base text-text-muted">
                <LoaderCircle className="animate-spin" aria-hidden />
                {t('machines.again.waiting', { name: machine.name })}
              </p>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}
