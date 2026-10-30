/** What a username, a name, a password and an address must look like before they are sent, said in words beside the field.
 *
 * The server holds every one of these rules and is asked in any case. They are repeated here so that
 * a form can say what is wrong when its reader leaves a field, without a request.
 */
import { useTranslation } from 'react-i18next';

import { PASSWORD_MIN_CHARS, USERNAME_PATTERN } from '@/api/auth';

/** Something before an at sign and a host with a dot after it: enough to catch a slip, and no more. */
const ADDRESS_SHAPE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** How long a password is, as its reader counts it. */
const lengthOf = (text: string): number => Array.from(text).length;

const same = (one: string, other: string): boolean =>
  one !== '' && one.toLocaleLowerCase() === other.trim().toLocaleLowerCase();

export interface IdentityRules {
  username: (value: string) => string | null;
  displayName: (value: string) => string | null;
  /** `others` are what the password must differ from: the username and the address. */
  password: (value: string, others: readonly string[]) => string | null;
  /** An address that must be given. */
  email: (value: string) => string | null;
  /** An address that may be left out. */
  optionalEmail: (value: string) => string | null;
}

export function useIdentityRules(): IdentityRules {
  const { t } = useTranslation();
  const email = (value: string) =>
    ADDRESS_SHAPE.test(value.trim()) ? null : t('auth.rules.emailShape');
  return {
    username: (value) =>
      value === '' ? t('auth.rules.usernameMissing') : USERNAME_PATTERN.test(value) ? null : t('auth.rules.usernameShape'),
    displayName: (value) => (value.trim() === '' ? t('auth.rules.nameMissing') : null),
    password: (value, others) => {
      if (value === '') return t('auth.rules.passwordMissing');
      if (lengthOf(value) < PASSWORD_MIN_CHARS) return t('auth.rules.passwordShort', { min: PASSWORD_MIN_CHARS });
      if (others.some((other) => same(value, other))) return t('auth.rules.passwordIsName');
      return null;
    },
    email: (value) => (value.trim() === '' ? t('auth.rules.emailMissing') : email(value)),
    optionalEmail: (value) => (value.trim() === '' ? null : email(value)),
  };
}
