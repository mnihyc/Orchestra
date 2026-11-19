/** Create the first administrator.
 *
 * The one screen of a system that has no user yet. Whoever fills it in becomes the administrator of
 * the system and is signed in.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Link, useRouter } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { metaKey, PASSWORD_MIN_CHARS, setUp } from '@/api/auth';
import { ApiError } from '@/api/client';
import { AuthCard } from '@/features/auth/Frame';
import { useIdentityRules } from '@/features/auth/identityRules';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { linkClasses } from '@/ui/link';

export function Setup() {
  const { t } = useTranslation();
  const router = useRouter();
  const queryClient = useQueryClient();
  const rules = useIdentityRules();
  const form = useForm(
    { username: '', display_name: '', password: '', email: '' },
    {
      username: rules.username,
      display_name: rules.displayName,
      password: (value, values) => rules.password(value, [values.username, values.email]),
      email: rules.optionalEmail,
    },
  );

  const create = useMutation({
    mutationFn: setUp,
    onSuccess: () => router.navigate({ to: '/', replace: true }),
    onError: (error) => {
      form.refuse(error);
      // Somebody else was quicker: the system has its administrator, and this screen is over.
      if (error instanceof ApiError && error.is('conflict.exists')) void queryClient.invalidateQueries({ queryKey: metaKey });
    },
  });

  const taken = create.error instanceof ApiError && create.error.is('conflict.exists');
  const placed = create.error instanceof ApiError && create.error.faults().some((fault) => fault.path in form.values);
  const username = form.field('username');

  return (
    <AuthCard title={t('auth.setup.title')} lead={t('auth.setup.lead')}>
      <form
        noValidate
        className="flex flex-col gap-4"
        onSubmit={form.submit((values) => {
          if (create.isPending) return;
          create.mutate({
            username: values.username,
            display_name: values.display_name.trim(),
            password: values.password,
            ...(values.email.trim() === '' ? {} : { email: values.email.trim() }),
          });
        })}
      >
        <Input
          label={t('auth.fields.username')}
          hint={t('auth.fields.usernameHint')}
          autoComplete="username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          autoFocus
          {...username}
          onChange={(event) => form.set('username', event.target.value.toLowerCase())}
        />
        <Input
          label={t('auth.fields.displayName')}
          hint={t('auth.fields.displayNameHint')}
          autoComplete="name"
          {...form.field('display_name')}
        />
        <PasswordInput
          label={t('auth.fields.password')}
          hint={t('auth.fields.passwordHint', { min: PASSWORD_MIN_CHARS })}
          autoComplete="new-password"
          {...form.field('password')}
        />
        <Input
          label={t('auth.fields.email')}
          optional
          hint={t('auth.setup.emailHint')}
          type="email"
          autoComplete="email"
          inputMode="email"
          {...form.field('email')}
        />
        {taken ? (
          <InlineAlert
            tone="warning"
            action={
              <Link to="/login" className={linkClasses}>
                {t('auth.links.signIn')}
              </Link>
            }
          >
            {t('auth.setup.alreadyDone')}
          </InlineAlert>
        ) : (
          create.error !== null && !placed && <ErrorAlert error={create.error} />
        )}
        <Button type="submit" variant="primary" block pending={create.isPending}>
          {t('auth.setup.submit')}
        </Button>
      </form>
    </AuthCard>
  );
}
