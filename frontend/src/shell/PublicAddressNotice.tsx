/** A changed origin is one line and one link; metadata never reloads another browser. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { Meta } from '@/generated/api-types';
import { useViewChoice } from '@/lib/viewChoices';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
export function PublicAddressNotice({ meta }: { meta: Meta | undefined }) {
  return meta === undefined ? null : <LoadedNotice meta={meta} />;
}
function LoadedNotice({ meta }: { meta: Meta }) {
  const { t } = useTranslation();
  const fingerprint = JSON.stringify([meta.public_url, meta.alternate_urls]);
  const [initial] = useState(fingerprint);
  const [dismissed, dismiss] = useViewChoice<boolean>('global', 'public-address-notice-dismissed', false, `${meta.public_address_revision}:${fingerprint}`);
  if (meta.public_url === '' || dismissed) return null;
  let primary: URL;
  try { primary = new URL(meta.public_url); if (!['http:', 'https:'].includes(primary.protocol)) return null; } catch { return null; }
  const different = primary.origin !== window.location.origin && !meta.alternate_urls.some((value) => { try { return new URL(value).origin === window.location.origin; } catch { return false; } });
  if (initial === fingerprint && !(different && meta.public_address_revision > 1)) return null;
  return <Banner onDismiss={() => dismiss(true)} action={<Button size="sm" asChild><a href={meta.public_url}>{t('common.open')}</a></Button>}>{t('publicAddress.changed')}</Banner>;
}
