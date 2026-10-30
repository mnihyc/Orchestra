/** One bounded result card for an immutable program build. */
import { useTranslation } from 'react-i18next';
import type { HarnessBuild } from '@/generated/api-types';
import { StatusTag, type TagTone } from '@/features/admin/StatusTag';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';
import { RelativeTime } from '@/ui/RelativeTime';
import { buildRefusal } from '@/lib/versions';
import { caseUnavailable, checkName, disabledFeatures, featureFallback, featureName, unsupportedReason } from './buildChecks';
import { programNameIn } from '@/features/resources/programNames';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';

/** How far a build is known to work, with its existing status trigger opening the check's results. */
export function BuildStatus({ build, name, catalogue }: { build: HarnessBuild; name?: string | undefined; catalogue?: unknown }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const status = build.status;
  const refusal = buildRefusal(build);
  const check = build.check;
  const cases = check?.cases ?? [];
  const firstFailure = cases.find(caseUnavailable);
  const off = disabledFeatures(build);
  const tone: TagTone = refusal !== null || status === 'failed' ? 'danger' : off.length > 0 ? 'warning' : status === 'checked' ? 'accent' : 'neutral';
  const firstFeature = firstFailure?.feature ?? off[0];
  const failedName = firstFailure === undefined ? null : checkName(firstFailure.name, t, catalogue, locale);
  const hasDetails = check !== undefined && check !== null && (check.error !== null || check.missing.length > 0 || off.length > 0 || cases.length > 0);
  const label = refusal !== null ? t('programVersions.refused') : status === 'checking' ? t(`admin.harnesses.checking.${check?.stage === 'conformance' ? 'conformance' : 'interface'}`)
    : firstFeature !== undefined ? t('programVersions.offInBuild', { feature: featureName(firstFeature, t, catalogue, locale) })
      : status === 'failed' && failedName !== null ? <>{t('admin.harnesses.status.failed')}{' · '}{failedName.known ? failedName.text : <code className="font-mono">{failedName.text}</code>}</> : t(`admin.harnesses.status.${status}`);
  const plainStatus = refusal !== null || (status !== 'checking' && firstFeature === undefined && !(status === 'failed' && failedName !== null));
  const tag = <StatusTag tone={tone} className={plainStatus ? 'min-w-max whitespace-nowrap' : 'max-w-full whitespace-normal wrap-anywhere'}>{label}</StatusTag>;
  return !hasDetails ? tag : <Popover label={t('admin.harnesses.checkResults')} trigger={<Tooltip content={t('admin.harnesses.checkResults')}><button type="button" className="focus-inset min-w-0 max-w-full rounded-md text-left">{tag}</button></Tooltip>}>
    <div className="flex min-w-0 flex-col gap-3 text-sm wrap-anywhere">
      <span className="font-medium">{name ?? programNameIn(build, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown')} {build.version}</span>
      {off.map((feature) => <div key={feature}>
        <div>{t('programVersions.offInBuild', { feature: featureName(feature, t, catalogue, locale) })}</div>
        {featureFallback(build, feature, t) !== null && <div className="text-text-muted">{featureFallback(build, feature, t)}</div>}
      </div>)}
      {cases.map((result) => {
        const name = checkName(result.name, t, catalogue, locale);
        const unsupported = unsupportedReason(result);
        return <div key={result.name} data-build-case={result.name}>
          <div>{name.known ? name.text : <code className="font-mono">{name.text}</code>}{' · '}{unsupported !== null ? t('programVersions.caseUnavailable') : t(`admin.harnesses.status.${result.passed ? 'checked' : 'failed'}`)}</div>
          {(unsupported ?? result.error) !== null && <div className="text-text-muted whitespace-pre-line">{unsupported ?? result.error}</div>}
        </div>;
      })}
      {off.includes('import_native') && <ArticleGuideLink article="carry-a-thread" heading="read-the-method" title={t('admin.harnesses.historySupport')} />}
      {check.error !== null && <div className="text-text-muted whitespace-pre-line">{check.error}</div>}
      {check.missing.length > 0 && <div className="text-text-muted">{t('admin.harnesses.missing', { names: check.missing.join(', ') })}</div>}
      <span className="text-text-muted"><RelativeTime at={check.at} />{check.machine === null ? null : ` · ${check.machine}`}</span>
    </div>
  </Popover>;
}
