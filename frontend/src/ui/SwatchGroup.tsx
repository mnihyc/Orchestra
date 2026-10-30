/** Named colour samples: a ring outside the sample marks the choice without covering it. */
import { RadioGroup } from 'radix-ui';
import { type ReactNode, useId } from 'react';

export interface SwatchOption {
  value: string;
  name: string;
  sample: ReactNode;
}

export interface SwatchGroupProps {
  label: string;
  value: string;
  onValueChange: (value: string) => void;
  options: readonly SwatchOption[];
  disabled?: boolean;
}

export function SwatchGroup({ label, value, onValueChange, options, disabled = false }: SwatchGroupProps) {
  const labelId = useId();
  return <div className="flex min-w-0 flex-col items-start gap-2">
    <span id={labelId} className="text-sm font-medium text-text">{label}</span>
    <RadioGroup.Root
      aria-labelledby={labelId}
      value={value}
      onValueChange={onValueChange}
      disabled={disabled}
      className="inline-flex w-fit max-w-full flex-wrap items-center gap-1"
    >
      {options.map((option) => <RadioGroup.Item
        key={option.value}
        type="button"
        value={option.value}
        aria-label={option.name}
        data-state={value === option.value ? 'checked' : 'unchecked'}
        className="swatch-option inline-flex size-8 shrink-0 items-center justify-center rounded-full p-0.5 transition-colors disabled:cursor-not-allowed disabled:opacity-65"
      >
        <span aria-hidden className="inline-flex">{option.sample}</span>
      </RadioGroup.Item>)}
    </RadioGroup.Root>
  </div>;
}
