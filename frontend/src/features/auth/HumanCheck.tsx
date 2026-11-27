/** The human check of the screens a person without a session meets: one widget, one token for each try.
 *
 * Where the system asks it of an act, a screen asks for a token before it sends, sends it in one
 * header, and counts it as spent whatever the answer was: a vendor's token serves one request. In a
 * checkbox the box is then fresh again and one short line says why, so that nobody is left with a
 * spent token and a button that stays off. Unseen, the widget is run for each try and nothing is drawn.
 * Where the system asks nothing, none of this exists: no script is added, no request is made and the
 * screen is as it was.
 */
import { useQuery } from '@tanstack/react-query';
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Trans, useTranslation } from 'react-i18next';

import { metaQuery } from '@/api/auth';
import type { HumanCheckAct, MetaHumanCheck } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { type Drawn, loadVendor, UNLOADED, type Vendor, VendorUnavailable } from '@/lib/humanCheck';
import { linkClasses } from '@/ui/link';

/** The one short line a widget may say under itself. */
export type CheckLine = 'spent' | 'expired' | 'unavailable';

/** A try cannot be made yet: the box has not been ticked. */
export class CheckWaiting extends Error {
  constructor() {
    super('the human check has not been passed yet');
    this.name = 'CheckWaiting';
  }
}

/** What a screen holds of the human check. */
export interface HumanCheck {
  /** What the system says of its check, where it asks one. */
  readonly check: MetaHumanCheck | null;
  /** Whether the system asks the check of `act`. */
  asks(act: HumanCheckAct): boolean;
  /** Whether `act` cannot be tried yet: the check is a box that is not ticked. */
  waiting(act: HumanCheckAct): boolean;
  /** The one short line to say under the widget. */
  readonly line: CheckLine | null;
  /**
   * The token for one try of `act`: none where the system asks none; [`UNLOADED`] where the vendor's widget could not be
   * loaded or run here, for the try to be made all the same and the server to look into it; rejects with [`CheckWaiting`]
   * where the box has not been ticked.
   */
  take(act: HumanCheckAct): Promise<string | null>;
  /** The token of the try just made is used up, whatever came of it; `refused` says that the try did not end the screen. */
  spend(refused: boolean): void;
  /** @internal what the widget component needs. */
  readonly inner: Inner;
}

interface Inner {
  vendor: Vendor | null;
  onToken: (token: string) => void;
  onExpired: () => void;
  onFailed: () => void;
  bind: (drawn: Drawn | null) => void;
}

/**
 * What a screen holds of the human check, for the acts the screen can ask. Without a check on for one
 * of them, it holds nothing and does nothing: no script is added.
 */
export function useHumanCheck(acts: readonly HumanCheckAct[]): HumanCheck {
  const meta = useQuery(metaQuery).data;
  const locale = useAppearance((state) => state.locale);
  const system = meta?.human_check ?? null;
  const check = system !== null && acts.some((act) => system.acts.includes(act)) ? system : null;
  const [vendor, setVendor] = useState<Vendor | null>(null);
  const [failedToLoad, setFailedToLoad] = useState(false);
  const [token, setToken] = useState<string | null>(null);
  // The token is also kept where a handler that runs before the next render reads it.
  const held = useRef<string | null>(null);
  const [line, setLine] = useState<CheckLine | null>(null);
  const drawn = useRef<Drawn | null>(null);
  const provider = check?.provider;
  const siteKey = check?.site_key;
  const verification = check?.verification;
  const alternate = check?.alternate_host;
  const loading = useRef<Promise<Vendor> | null>(null);

  // The vendor's script is added when a screen that asks for the check is open, and not before.
  useEffect(() => {
    if (check === null) return;
    let live = true;
    const asked = loadVendor(check, locale);
    loading.current = asked;
    asked.then(
      (loaded) => {
        if (live) setVendor(loaded);
      },
      () => {
        if (live) {
          setFailedToLoad(true);
          setLine('unavailable');
        }
      },
    );
    return () => {
      live = false;
    };
    // The check is the same while these are.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, siteKey, verification, alternate, locale]);

  const asks = useCallback((act: HumanCheckAct) => check !== null && acts.includes(act) && check.acts.includes(act), [check, acts]);
  const boxed = check?.mode === 'checkbox';
  const waiting = useCallback((act: HumanCheckAct) => asks(act) && boxed && token === null && !failedToLoad, [asks, boxed, token, failedToLoad]);

  const take = useCallback(
    async (act: HumanCheckAct): Promise<string | null> => {
      if (check === null || !asks(act)) return null;
      if (boxed) {
        if (held.current !== null) return held.current;
        if (failedToLoad) return UNLOADED;
        throw new CheckWaiting();
      }
      setLine(null);
      try {
        const run = loading.current;
        if (run === null) throw new VendorUnavailable();
        return await (await run).run(act);
      } catch {
        // The vendor could not be run here: nobody has a token to give, and the try is made with the word that says so.
        setLine('unavailable');
        return UNLOADED;
      }
    },
    [check, asks, boxed, failedToLoad],
  );

  const spend = useCallback(
    (refused: boolean) => {
      if (check === null) return;
      held.current = null;
      setToken(null);
      drawn.current?.reset();
      // A box that never loaded was not spent: its own line stands.
      if (boxed && refused && !failedToLoad) setLine('spent');
    },
    [check, boxed, failedToLoad],
  );

  const inner = useMemo<Inner>(
    () => ({
      vendor,
      onToken: (given) => {
        held.current = given;
        setToken(given);
        setLine(null);
      },
      onExpired: () => {
        held.current = null;
        setToken(null);
        setLine('expired');
      },
      onFailed: () => {
        held.current = null;
        setToken(null);
        setLine('unavailable');
      },
      bind: (next) => {
        drawn.current = next;
      },
    }),
    [vendor],
  );

  return useMemo(() => ({ check, asks, waiting, line, take, spend, inner }), [check, asks, waiting, line, take, spend, inner]);
}

/** The box of the check, above the button of the try it is for; nothing where the system asks no box. */
export function HumanCheckWidget({ check, act }: { check: HumanCheck; act: HumanCheckAct }) {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement>(null);
  const { vendor, onToken, onExpired, onFailed, bind } = check.inner;
  const boxed = check.check?.mode === 'checkbox' && check.asks(act);

  useEffect(() => {
    const where = container.current;
    if (!boxed || vendor === null || where === null) return;
    const drawn = vendor.render(where, { token: onToken, expired: onExpired, failed: onFailed });
    bind(drawn);
    return () => {
      bind(null);
      drawn.dispose();
    };
  }, [boxed, vendor, onToken, onExpired, onFailed, bind]);

  if (check.check === null || !check.asks(act)) return null;
  return (
    <div className="flex flex-col gap-2" data-human-check={check.check.provider}>
      {boxed && <div ref={container} className="min-h-[78px] max-w-full overflow-hidden" />}
      {check.line !== null && (
        <p role="status" className="text-sm text-text-muted">
          {t(`auth.humanCheck.${check.line}`)}
        </p>
      )}
    </div>
  );
}

/** The sentence a vendor requires of a page that uses it, below the button; nothing where none is required. */
export function HumanCheckNotice({ check, act }: { check: HumanCheck; act: HumanCheckAct }): ReactNode {
  const { t } = useTranslation();
  if (check.check === null || !check.asks(act)) return null;
  if (check.check.provider === 'recaptcha') {
    return (
      <p className="text-center text-xs text-text-muted" data-human-check-notice="recaptcha">
        {t('auth.humanCheck.noticeRecaptcha')}
      </p>
    );
  }
  // hCaptcha asks for its words where it runs unseen; its box carries them itself.
  if (check.check.mode !== 'invisible') return null;
  return (
    <p className="text-center text-xs text-text-muted" data-human-check-notice="hcaptcha">
      <Trans
        i18nKey="auth.humanCheck.noticeHcaptcha"
        components={{
          privacy: <a className={linkClasses} href="https://www.hcaptcha.com/privacy" target="_blank" rel="noreferrer noopener" />,
          terms: <a className={linkClasses} href="https://www.hcaptcha.com/terms" target="_blank" rel="noreferrer noopener" />,
        }}
      />
    </p>
  );
}
