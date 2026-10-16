# Motion timing

- Always convert CSS time values to milliseconds with `cssTimeMs` from
  `desktop/src/motion-time.ts` before using them in JavaScript animations or timers.
- Never use bare `parseFloat` for CSS durations or delays. Production CSS
  minification can rewrite `200ms` to `.2s`; both must resolve to 200 milliseconds.
- Preserve valid `0ms` / `0s` values. Use a fallback only for invalid or missing values.
- When changing motion timing, check the built CSS units as well as source values.

# UI verification

- Verify UI-only changes in the existing running web UI preview. Do not build or restart the native app for UI-only testing.
- Use native dev builds to verify actual application functionality changes.
- When the user says they will verify a UI change, edit the source only; do not run UI tests, builds, or launch/restart applications.
