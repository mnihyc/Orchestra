/** Noninteractive cards explain the same controls when a reader has no qualifying work yet. */
import { useTranslation } from 'react-i18next';
import type { TutorialExample as ExampleKind } from '@/help/tutorials/model';

export function TutorialExample({ kind }: { kind: ExampleKind }) {
  const { t } = useTranslation();
  return <figure data-testid="tutorial-example" className="flex flex-col gap-2 rounded-md border border-border bg-bg-subtle p-3 text-sm">
    <figcaption className="text-xs font-medium text-text-muted">{t('helpScreenReview.example')}</figcaption>
    <h3 className="font-medium">{t(`helpScreenReview.examples.${kind}.title`)}</h3>
    <dl className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-surface p-2">
      <dt className="text-text-muted">{t(`helpScreenReview.examples.${kind}.label`)}</dt>
      <dd className="font-medium">{t(`helpScreenReview.examples.${kind}.value`)}</dd>
    </dl>
    <p>{t(`helpScreenReview.examples.${kind}.detail`)}</p>
    <p className="text-xs text-text-muted">{t('helpScreenReview.exampleNote')}</p>
  </figure>;
}
