/** What a harness found of its own in an environment's workspace, and whether it may use it.
 *
 * Each harness that found a configuration of its own here: the
 * answer its managers gave, who gave it and when, or that the question still waits, with where it was found.
 * Whoever manages the environment answers or changes it here, Trust or Keep it out, and the same question is
 * offered in a thread that waits for it; the sessions opened after it use it.
 */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { isProgramName, programName } from '@/features/resources/programNames';
import { EmptyState } from '@/ui/EmptyState';

import { setConfigTrust } from '@/api/trust';
import type { ConfigTrust, ConfigTrustAnswer, Environment } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';

/** What was found and what each answer does. */
export function TrustQuestion({ harness, path }: { harness: string; path: string }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2 text-sm">
      <p className="text-text">{t('environments.configTrust.found', { harness })}</p>
      {path !== '' && <p className="font-mono wrap-anywhere text-text-muted">{path}</p>}
      <p className="text-text-muted">{t('environments.configTrust.trustMeans', { harness })}</p>
      <p className="text-text-muted">{t('environments.configTrust.keepOutMeans', { harness })}</p>
      <p className="text-text-subtle">{t('environments.configTrust.remembered')}</p>
    </div>
  );
}

/** Trust and Keep it out for one harness, the one already given marked as such. */
export function TrustAnswers({ environmentId, entry, label }: { environmentId: string; entry: ConfigTrust; label: string }) {
  const { t } = useTranslation();
  const change = useMutation({
    mutationFn: (answer: ConfigTrustAnswer) => setConfigTrust(environmentId, entry.harness, answer),
  });
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap gap-2" role="group" aria-label={label}>
        <Button size="sm" variant="secondary" disabled={change.isPending || entry.answer === 'trust'} pending={change.isPending && change.variables === 'trust'} onClick={() => change.mutate('trust')}>
          {t('environments.configTrust.trust')}
        </Button>
        <Button size="sm" variant="secondary" disabled={change.isPending || entry.answer === 'keep_out'} pending={change.isPending && change.variables === 'keep_out'} onClick={() => change.mutate('keep_out')}>
          {t('environments.configTrust.keepOut')}
        </Button>
      </div>
      {change.error !== null && <ErrorAlert error={change.error} />}
    </div>
  );
}

export function HarnessTrustSection({ environment }: { environment: Environment }) {
  const { t, i18n } = useTranslation();
  const manages = environment.access.permissions.includes('environment.manage');
  const title = (entry: ConfigTrust) => {
    const named = 'program' in entry && isProgramName(entry.program) ? entry.program : null;
    return programName(named, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  };
  return (
    <Section id="environment-config-trust" title={t('environments.configTrust.title')} lead={t('environments.configTrust.lead')}>
      {environment.config_trust.length === 0 ? (
        <EmptyState compact title={t('environments.configTrust.none')} />
      ) : (
        <ul className="flex flex-col gap-2">
          {environment.config_trust.map((entry) => (
            <li key={entry.harness} className="flex flex-col gap-1 rounded-md border border-border px-3 py-2" data-config-trust={entry.harness}>
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="font-medium text-text">{title(entry)}</span>
                <span className="text-text-muted">
                  {entry.answer === null ? t('environments.configTrust.asking') : entry.answer === 'trust' ? t('environments.configTrust.trusted') : t('environments.configTrust.keptOut')}
                </span>
                {entry.by !== null && entry.at !== null && (
                  <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-subtle">
                    <UserChip user={entry.by} size={24} /><RelativeTime at={entry.at} />
                  </span>
                )}
              </div>
              {entry.path !== '' && <p className="font-mono text-xs wrap-anywhere text-text-muted">{entry.path}</p>}
              {manages && <TrustAnswers environmentId={environment.id} entry={entry} label={t('environments.configTrust.change', { harness: title(entry) })} />}
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}
