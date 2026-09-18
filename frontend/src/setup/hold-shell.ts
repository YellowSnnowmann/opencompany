/**
 * Whether `AppShell` should render a neutral loader rather than the ordinary
 * console.
 *
 * `SetupController`'s roster read is what decides whether first-run setup opens
 * (`SetupController.tsx`), and `setupChecked` is `false` until it lands —
 * indistinguishable, from here, from a roster that landed and found the company
 * genuinely unstaffed. Rendering the console on that unresolved answer shows an
 * empty, interactive shell for the length of a network call and then drops the
 * setup dialog over it.
 *
 * The read is bounded on both axes (`SETUP_ROSTER_TIMEOUT_MS`, and a `catch`
 * that settles `checked` regardless), so this hold always ends.
 */
export function shouldHoldShellPending(input: { setupChecked: boolean }): boolean {
  return !input.setupChecked;
}
