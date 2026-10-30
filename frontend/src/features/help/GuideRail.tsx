/** A section has one help entrance; related articles stay inside its short answer. */
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { HelpHint } from '@/ui/HelpHint';

export function GuideRail({ label, children }: { label: string; children: ReactNode }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <HelpHint term={label} open={open} onOpenChange={setOpen} what={
    <div className="flex flex-col gap-2" onClickCapture={(event) => {
      if (event.target instanceof Element && event.target.closest('[data-help-article]') !== null) setOpen(false);
    }}>
      <p className="text-sm text-text-muted">{t('helpFlow.guides')}</p>
      <div className="flex flex-col items-start gap-1">{children}</div>
    </div>
  } />;
}
