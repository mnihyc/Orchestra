/** Delivery facts are the admitted command's observation; opening never rereads an image. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { NativeImageReceipt, NativeImageReason } from '@/generated/api-types';
import { RecordedSettingSourceLink } from '@/features/settings/RecordedSettingSourceLink';
import { formatBytes } from '@/lib/format';
import { FactDisclosure, FactDisclosureBody } from '@/ui/FactDisclosure';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { ScrollRegion } from '@/ui/ScrollRegion';

export function NativeImageFacts({ receipt, thread }: { receipt?: NativeImageReceipt | null; thread: string }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const words = (reason: NativeImageReason) => {
    const keys = { allowance_exceeded: 'composer.imageFacts.exceeded', paths_setting: 'composer.imageFacts.paths', not_supported: 'composer.imageFacts.unsupported', size_unreported: 'composer.imageFacts.unknownSize', materialization_unavailable: 'composer.imageFacts.unavailable' } as const;
    return t(keys[reason]);
  };
  const images = receipt?.images ?? [];
  const fragment = images.some((image) => image.delivery === 'path' && image.reason === 'allowance_exceeded') ? t('composer.imageFacts.pathExceeded')
    : receipt?.native_image_bytes === 0 && images.some((image) => image.delivery === 'path') ? t('composer.imageFacts.paths') : undefined;
  return <FactDisclosure open={open} onOpenChange={setOpen} fragment={fragment} groups={[{ key: 'native-image', kind: 'recorded_outcome', facts: [
    { key: 'allowance', label: t('composer.imageFacts.allowance'), value: receipt === undefined || receipt === null ? undefined : formatBytes(receipt.native_image_bytes, locale),
      source: open && receipt !== undefined && receipt !== null ? <RecordedSettingSourceLink source={receipt.source} setting="files.native_image_bytes" target={{ kind: 'node', id: thread }} /> : undefined },
    { key: 'at', label: t('composer.imageFacts.at'), value: open && receipt !== undefined && receipt !== null ? <RelativeTime at={receipt.resolved_at} /> : undefined },
  ] }]}>
    {open && receipt !== undefined && receipt !== null && <>
      {images.length > 10 && <Input label={t('composer.imageFacts.search')} hideLabel placeholder={t('composer.imageFacts.search')} value={search} onChange={(event) => setSearch(event.target.value)} />}
      <ScrollRegion kind="table" label={t('composer.imageFacts.images')} className="max-h-(--list-max-height) overflow-y-auto">
        <FactDisclosureBody groups={images.filter((image) => image.name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())).map((image, index) => ({ key: String(index), title: image.name, facts: [
          { key: 'delivery', label: t('composer.imageFacts.delivery'), value: t(image.delivery === 'native' ? 'composer.imageFacts.native' : image.delivery === 'path' ? 'composer.imageFacts.path' : 'composer.imageFacts.unavailable') },
          { key: 'bytes', label: t('composer.imageFacts.bytes'), value: image.bytes === null ? undefined : formatBytes(image.bytes, locale) },
          ...(image.reason === null ? [] : [{ key: 'reason', label: t('composer.imageFacts.reason'), value: words(image.reason) }]),
        ] }))} />
      </ScrollRegion>
    </>}
  </FactDisclosure>;
}
