/** WCAG relative luminance for the opaque sRGB colours used by the application and browser. */
function luminance(colour: string): number {
  const hex = /^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/iu.exec(colour);
  const rgb = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+))?\s*\)$/u.exec(colour);
  if (hex === null && rgb === null) throw new Error(`Expected an opaque sRGB colour, got ${colour}`);
  if (rgb?.[4] !== undefined && Number(rgb[4]) !== 1) throw new Error(`Resolve the background before comparing ${colour}`);
  const channel = (index: number) => {
    const raw = hex === null ? Number(rgb?.[index]) : Number.parseInt(hex[index] ?? '', 16);
    if (!Number.isFinite(raw) || raw < 0 || raw > 255) throw new Error(`Invalid sRGB channel in ${colour}`);
    const value = raw / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(2) + 0.0722 * channel(3);
}

export function contrast(foreground: string, background: string): number {
  const first = luminance(foreground), second = luminance(background);
  return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
}
