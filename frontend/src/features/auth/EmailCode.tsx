/** The code sent by mail: asking for one, the one field for it, and the same sentence whatever the name.
 *
 * The page cannot know whether a code went out: the answer to asking is the same for every name, so
 * that nothing is learnt from it. So it says what is true in every case, shows one field, and offers
 * another code once a minute has passed, which is as often as one is sent.
 */
import { type ReactNode, type SyntheticEvent, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { linkButtonClasses } from '@/ui/link';

/** How long after one code another may be asked for: the pace at which the server sends them. */
export const RESEND_AFTER_MS = 60_000;

/** The seconds left until `at`, counted down once a second while any are left. */
function useSecondsUntil(at: number): number {
  const [now, setNow] = useState(() => Date.now());
  const over = now >= at;
  useEffect(() => {
    if (over) return;
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(timer);
  }, [over]);
  return Math.max(0, Math.ceil((at - now) / 1_000));
}

export interface EmailCodeProps {
  /** What is true whoever asked: that a code is on its way if the name may have one. */
  sentence: ReactNode;
  /** When the last code was asked for, on this device's clock. */
  sentAt: number;
  /** Signs in with the code. */
  onSubmit: (code: string) => void;
  /** Asks for another code. */
  onResend: () => void;
  /** A change in the system's mail service cannot offer another send that it cannot perform. */
  canResend?: boolean;
  /** The sign-in is under way. */
  pending: boolean;
  /** Another code is being asked for. */
  resending: boolean;
  /** How many attempts were refused so far; each one selects what was typed, to be typed again. */
  failures: number;
  /** What is wrong with the code, said under its field. */
  fieldError?: string | undefined;
  /** What the sign-in was refused with otherwise, drawn by the caller. */
  refusal?: ReactNode;
  /** Other ways on, under the button. */
  children?: ReactNode;
}

export function EmailCode({
  sentence,
  sentAt,
  onSubmit,
  onResend,
  canResend = true,
  pending,
  resending,
  failures,
  fieldError,
  refusal,
  children,
}: EmailCodeProps) {
  const { t } = useTranslation();
  const [code, setCode] = useState('');
  const [missing, setMissing] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  const wait = useSecondsUntil(sentAt + RESEND_AFTER_MS);

  // A code that was refused is typed again from the start.
  useEffect(() => {
    if (failures > 0) field.current?.select();
  }, [failures]);

  const submit = (event: SyntheticEvent) => {
    event.preventDefault();
    if (pending) return;
    const typed = code.trim();
    if (typed === '') {
      setMissing(true);
      field.current?.focus();
      return;
    }
    onSubmit(typed);
  };

  return (
    <form noValidate className="flex flex-col gap-4" onSubmit={submit}>
      <InlineAlert tone="info">{sentence}</InlineAlert>
      <Input
        ref={field}
        label={t('auth.code.label')}
        mono
        autoFocus
        autoComplete="one-time-code"
        autoCapitalize="characters"
        autoCorrect="off"
        spellCheck={false}
        value={code}
        error={missing && code.trim() === '' ? t('auth.code.missing') : fieldError}
        onChange={(event) => {
          setCode(event.target.value);
          setMissing(false);
        }}
      />
      {refusal}
      <Button type="submit" variant="primary" block pending={pending}>
        {t('auth.login.submit')}
      </Button>
      <div className="flex flex-col items-start gap-2 text-sm">
        {canResend && <button
          type="button"
          className={linkButtonClasses}
          disabled={wait > 0 || resending}
          onClick={onResend}
        >
          {wait > 0 ? t('auth.code.resendIn', { count: wait }) : t('auth.code.resend')}
        </button>}
        {children}
      </div>
    </form>
  );
}
