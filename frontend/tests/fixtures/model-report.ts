/** A discovery report offers facts without opening native search or a larger context. */
export const MODEL_REPORT = {
  context_max: 1_000_000,
  web_search: ['live'],
  report: {
    output_max: 128_000,
    lifecycle: 'deprecated',
    deprecated_at: '2026-10-01T00:00:00Z',
    retires_at: '2027-01-01T00:00:00Z',
    line: 'future-line',
    capabilities: {
      image_input: { supported: true },
      pdf_input: { supported: true },
      effort: { supported: true, max: { supported: true } },
      thinking: { supported: true, types: { adaptive: { supported: true } } },
      structured_outputs: { supported: true },
      context_management: { supported: true },
      citations: { supported: false },
      batch: { supported: true },
      server_tools: { web_search: { supported: true }, code_execution: { supported: false } },
      future_capability: { supported: true, opaque: ['kept'] },
    },
    future_member: { kept: true },
  },
};

/** JSON may name ordinary prototype members; these stay literal provider data, never catalogue values. */
export const FUTURE_MODEL_REPORT = {
  ...MODEL_REPORT,
  report: {
    ...MODEL_REPORT.report,
    lifecycle: '__proto__',
    capabilities: JSON.parse('{"__proto__":{"supported":true},"constructor":{"supported":false},"toString":{"supported":true}}') as Record<string, unknown>,
  },
};
