# Motion timing

- Always convert CSS time values to milliseconds with `cssTimeMs` from
  `desktop/src/motion-time.ts` before using them in JavaScript animations or timers.
- Never use bare `parseFloat` for CSS durations or delays. Production CSS
  minification can rewrite `200ms` to `.2s`; both must resolve to 200 milliseconds.
- Preserve valid `0ms` / `0s` values. Use a fallback only for invalid or missing values.
- When changing motion timing, check the built CSS units as well as source values.
