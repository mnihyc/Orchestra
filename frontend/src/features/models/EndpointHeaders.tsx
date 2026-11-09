/** Familiar fixed headers edit the endpoint's existing nonsecret map; the raw map remains available. */
import { useTranslation } from 'react-i18next';
import { documentedHeaders } from './optionCatalogue';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';
import { HelpHint } from '@/ui/HelpHint';
import type { FormValue } from '@/ui/FieldForm';

export function EndpointHeaders({ protocol, value, onChange, disabled = false }: { protocol: string; value: FormValue | undefined; onChange: (value: Record<string, string>) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const names = documentedHeaders(protocol);
  if (names.length === 0 || value !== undefined && (typeof value !== 'object' || Object.values(value).some((item) => typeof item !== 'string'))) return null;
  const headers = value ?? {};
  return <fieldset className="flex min-w-0 flex-col gap-3" data-documented-headers><legend className="mb-2 flex items-center gap-1 text-sm font-medium">{t('providerFields.headers')}<HelpHint term={t('providerFields.headers')} what={t('providerFields.headersHelp')} /></legend>
    {names.map((name) => {
      const key = Object.keys(headers).find((key) => key.toLowerCase() === name.toLowerCase());
      return <div key={name} className="flex min-w-0 flex-col gap-2"><Switch presentation="compact" label={name} checked={key !== undefined} disabled={disabled} onCheckedChange={(on) => {
        const next = { ...headers };
        if (on) Object.defineProperty(next, key ?? name, { value: key === undefined ? '' : headers[key], enumerable: true, writable: true, configurable: true });
        else if (key !== undefined) Reflect.deleteProperty(next, key);
        onChange(next);
      }} />
        {key !== undefined && <Input label={t('providerFields.headerFor', { name })} hideLabel value={headers[key]} disabled={disabled} onChange={(event) => onChange({ ...headers, [key]: event.target.value })} />}
      </div>;
    })}
  </fieldset>;
}
