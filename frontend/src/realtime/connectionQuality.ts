/** A person's recent link compared with its own measured usual, rather than an absolute distance. */
export interface ConnectionQualitySettings {
  recent_samples: number;
  usual_samples: number;
  slow_increase_percent: number;
  slow_extra_ms: number;
}

/** Mirrors the registry default until the authenticated Meta supplies the installation's value. */
export const DEFAULT_CONNECTION_QUALITY: ConnectionQualitySettings = {
  recent_samples: 3, usual_samples: 20, slow_increase_percent: 100, slow_extra_ms: 200,
};

export function connectionQualitySettings(input: unknown): ConnectionQualitySettings {
  const values = typeof input === 'object' && input !== null ? input as Record<string, unknown> : {};
  const number = (name: keyof ConnectionQualitySettings, minimum: number) => {
    const value = values[name];
    return typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value) && value >= minimum ? value : DEFAULT_CONNECTION_QUALITY[name];
  };
  return { recent_samples: number('recent_samples', 1), usual_samples: number('usual_samples', 1),
    slow_increase_percent: number('slow_increase_percent', 0), slow_extra_ms: number('slow_extra_ms', 0) };
}

interface Measurement { roundTripMs: number | null; usualRoundTripMs: number | null; slow: boolean }
const median = (values: readonly number[]): number | null => {
  const ordered = [...values].sort((one, other) => one - other), middle = Math.floor(ordered.length / 2);
  const upper = ordered[middle];
  return upper === undefined ? null : ordered.length % 2 === 1 ? upper : ((ordered[middle - 1] ?? upper) + upper) / 2;
};

export class LinkHistory {
  private usual: number[] = [];
  private recent: number[] = [];
  private lastUsual: number | null = null;

  /** A reconnect has new packet samples but keeps this reader's measured normal line. */
  reconnect(): Measurement { this.recent = []; return { roundTripMs: null, usualRoundTripMs: this.lastUsual, slow: false }; }
  /** A replacement reader inherits no former reader's measurements. */
  clear(): void { this.usual = []; this.recent = []; this.lastUsual = null; }

  observe(milliseconds: number, settings: ConnectionQualitySettings): Measurement {
    const previous = this.usual.slice(-settings.usual_samples);
    this.recent = [...this.recent, milliseconds].slice(-settings.recent_samples);
    const enough = previous.length >= Math.min(settings.recent_samples, settings.usual_samples);
    const normal = enough ? median(previous) : null;
    const current = median(this.recent);
    const threshold = normal === null ? null : Math.max(normal * (1 + settings.slow_increase_percent / 100), normal + settings.slow_extra_ms);
    const slow = threshold !== null && current !== null && this.recent.length >= settings.recent_samples && current > threshold;
    // A sustained route change gradually becomes this person's new usual.
    this.usual = [...previous, milliseconds].slice(-settings.usual_samples);
    this.lastUsual = normal;
    return { roundTripMs: current, usualRoundTripMs: normal, slow };
  }
}
