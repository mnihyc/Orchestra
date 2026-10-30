/** The page the screens before sign-in stand on: the system's name and notice, one column for the form, and the language and theme at its foot.
 *
 * Nobody is signed in here, so nothing of anybody's work is on this page: the mark, what the system
 * calls itself, the notice its administrators put up, one card, and the two things a stranger may
 * already want to set, their language and their theme.
 */
import { useQuery } from '@tanstack/react-query';
import { Monitor, Moon, Sun } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { metaQuery } from '@/api/auth';
import type { Theme } from '@/generated/api-types';
import { LOCALES } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { usePageTitle } from '@/lib/title';
import { Mark } from '@/shell/Mark';
import { Banner } from '@/ui/Banner';
import { cx } from '@/ui/cx';
import { SegmentedControl } from '@/ui/SegmentedControl';

/** Whether a notice that ends at `until` is still to be shown. */
function stillUp(until: string | null): boolean {
  return until === null || Date.parse(until) > Date.now();
}

export function AuthFrame({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const meta = useQuery(metaQuery).data;
  const theme = useAppearance((state) => state.theme);
  const locale = useAppearance((state) => state.locale);
  const set = useAppearance((state) => state.set);
  const banner = meta?.banner && stillUp(meta.banner.until) ? meta.banner : null;
  const locales = LOCALES.filter((entry) => meta === undefined || meta.locales.includes(entry.locale));

  return (
    <div className="flex min-h-dvh flex-col bg-bg-inset">
      {banner && <Banner tone={banner.level}>{banner.text}</Banner>}
      <main className="flex flex-1 flex-col items-center justify-center px-4 py-6">
        <div className="flex w-full max-w-form flex-col gap-6">
          <div className="flex flex-col items-center gap-3">
            <Mark className="size-10" />
            <h1 className="text-center text-xl font-semibold wrap-anywhere text-text">{meta?.name ?? t('app.name')}</h1>
          </div>
          {children}
        </div>
      </main>
      <footer className="flex flex-wrap items-center justify-center gap-x-6 gap-y-3 px-4 pb-6 text-sm text-text-muted">
        <SegmentedControl
          size="sm"
          className="max-w-full"
          label={t('preferences.language')}
          value={locale}
          onValueChange={(value) => set({ locale: value })}
          segments={locales.map((entry) => ({ value: entry.locale, name: entry.name }))}
        />
        <SegmentedControl<Theme>
          size="sm"
          className="max-w-full"
          label={t('preferences.theme')}
          value={theme}
          onValueChange={(value) => set({ theme: value })}
          segments={[
            { value: 'system', name: t('choiceCatalog.theme.system.name'), icon: <Monitor aria-hidden /> },
            { value: 'light', name: t('choiceCatalog.theme.light.name'), icon: <Sun aria-hidden /> },
            { value: 'dark', name: t('choiceCatalog.theme.dark.name'), icon: <Moon aria-hidden /> },
          ]}
        />
        {meta !== undefined && <span>{t('app.version', { version: meta.version })}</span>}
      </footer>
    </div>
  );
}

export interface AuthCardProps {
  /** What this screen is for, retained in the browser tab without repeating its form action. */
  title: string;
  /** One or two sentences under the heading. */
  lead?: ReactNode;
  children?: ReactNode;
  /** Ways elsewhere, under the card's content: "Back to sign-in". */
  foot?: ReactNode;
  className?: string;
}

/** One auth form: its title names the browser tab; the product already supplies the page heading. */
export function AuthCard({ title, lead, children, foot, className }: AuthCardProps) {
  usePageTitle(title);
  return (
    <section className={cx('flex flex-col gap-6 rounded-xl border border-border bg-surface p-6 shadow-sm', className)}>
      {lead !== undefined && <p className="text-base text-text-muted">{lead}</p>}
      {children}
      {foot !== undefined && <footer className="border-t border-border pt-4 text-sm text-text-muted">{foot}</footer>}
    </section>
  );
}
