/** "Open a browser…": a profile of the list or a new one by name, how it starts, and where it begins; joining the one that already runs on a profile is said as joining.
 *
 * Above the button stands, always, what a browser is: it acts at every site as whoever signed in to it
 * there, and everybody who may drive browsers here, every agent that works here and whoever may talk to
 * one of them can use what is signed in to in this profile. A way of starting that cannot be had here is
 * shown switched off with why. A profile's name is the name of its directory, and is checked as such.
 */
import { useMutation } from '@tanstack/react-query';
import { AppWindow } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { openBrowser } from '@/api/browsers';
import type { Browser, BrowserPreset, EnvironmentBrowsers } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';

/** A profile's name: the name of its directory. */
export const PROFILE_NAME = /^[a-z0-9][a-z0-9._-]{0,62}$/;

/** The choice of profile: one of the list, or a new one. */
const NEW = '\u0000new';

export interface OpenDialogProps {
  environmentId: string;
  environmentName: string;
  list: EnvironmentBrowsers;
  /** How many people may watch browsers there, where it is known. */
  watchers: number | null;
  threadId: string | null;
  /** Where the first tab goes, as another screen asked. */
  url?: string;
  /** A profile to start on, as a row asked. */
  profile?: string;
  onOpened: (browser: Browser) => void;
  onClose: () => void;
}

export function OpenDialog({ environmentId, environmentName, list, watchers, threadId, url: firstUrl = '', profile: firstProfile, onOpened, onClose }: OpenDialogProps) {
  const { t } = useTranslation();
  const [choice, setChoice] = useState<string>(firstProfile ?? list.profiles.find((one) => one.browser === null)?.name ?? list.profiles[0]?.name ?? NEW);
  const [name, setName] = useState('');
  const available = list.presets.filter((preset) => preset.available);
  const [preset, setPreset] = useState<BrowserPreset>(available[0]?.name ?? 'headless');
  const [url, setUrl] = useState(firstUrl);
  const making = choice === NEW;
  const profileName = making ? name.trim() : choice;
  const running = making ? null : list.profiles.find((one) => one.name === choice)?.browser ?? null;
  const nameFault = making && name !== '' && !PROFILE_NAME.test(name.trim()) ? t('browser.open.nameFault') : making && list.profiles.some((one) => one.name === name.trim()) ? t('browser.open.nameTaken') : null;
  const open = useMutation({
    mutationFn: () => openBrowser(environmentId, { profile: profileName, create: making, ...(running === null ? { preset } : {}), ...(url.trim() === '' ? {} : { url: url.trim() }) }, threadId),
    onSuccess: (browser) => {
      onOpened(browser);
    },
  });
  const ready = profileName !== '' && nameFault === null && (running !== null || available.some((one) => one.name === preset));
  const presetWords = (kind: BrowserPreset) => ({ ...choiceOption(t, `browserMode.${kind}`), description: t(kind === 'window' ? 'browser.open.windowHint' : 'browser.open.headlessHint') });
  return (
    <Dialog
      open
      size="md"
      title={t('browser.open.title', { environment: environmentName })}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button variant="primary" icon={<AppWindow aria-hidden />} pending={open.isPending} disabled={!ready} onClick={() => open.mutate()}>
            {running === null ? t('browser.open.do') : t('browser.open.join', { profile: profileName })}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (ready) open.mutate(); }}>
        <RadioGroup<string>
          variant="list"
          label={t('browser.open.profile')}
          value={choice}
          onValueChange={setChoice}
          options={[
            ...list.profiles.map((one) => ({ value: one.name, name: one.browser === null ? one.name : t('browser.open.joinProfile', { profile: one.name }) })),
            { value: NEW, ...choiceOption(t, 'browserProfile.new'), description: t('browser.open.newProfile') },
          ]}
        />
        {making && <Input label={t('browser.open.name')} hint={t('browser.open.nameHint')} mono value={name} error={nameFault ?? undefined} onChange={(event) => setName(event.target.value.toLowerCase())} />}
        {running === null && (
          <RadioGroup<BrowserPreset>
            variant="list"
            label={t('browser.open.starts')}
            value={preset}
            onValueChange={setPreset}
            options={(list.presets.length === 0 ? [{ name: 'headless' as const, available: true, reason: null }] : list.presets).map((one) => ({ value: one.name, ...presetWords(one.name), disabled: one.available ? false : (one.reason ?? t('browser.open.cannotHere')) }))}
          />
        )}
        <Input label={t('browser.open.url')} optional mono type="url" inputMode="url" value={url} placeholder={t('browser.open.urlPlaceholder')} onChange={(event) => setUrl(event.target.value)} />
        <p className="text-sm text-text-muted" data-signins-sentence>
          {t('browser.open.signIns', { environment: environmentName })} {watchers === null ? null : t('browser.open.watchers', { count: watchers })}
        </p>
        {open.error !== null && <ErrorAlert error={open.error} />}
      </form>
    </Dialog>
  );
}
