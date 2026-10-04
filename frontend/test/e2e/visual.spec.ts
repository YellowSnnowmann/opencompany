import { expect, test, type Page } from "@playwright/test";

import { VISUAL, VISUAL_REASON } from "./capabilities";

/**
 * Full-page baselines for the console's top-level surfaces, in both themes.
 *
 * See {@link VISUAL} in `./capabilities.ts` for why this is a lane of its own,
 * why it is not a required check, and — most importantly — why the rest of this
 * suite should go on asserting named quantities rather than pixels. This file
 * covers the complement: the regression nobody had a quantity for, because
 * nobody knew to write one.
 *
 * # What is done to make a page comparable to itself
 *
 * A screenshot suite is worth exactly as much as its false-positive rate, and
 * everything below exists to hold that rate at zero:
 *
 *   - **The tour is skipped.** It opens a modal over a fresh console, which
 *     would be the only thing in every baseline. Every spec here does this.
 *   - **The theme is pinned before boot**, so the first paint is the one under
 *     test rather than a light frame that resolves to dark a tick later.
 *   - **Animations are disabled** by `toHaveScreenshot`, and web fonts are
 *     awaited through `document.fonts.ready` — a baseline recorded during the
 *     fallback-font flash differs from every later run by a whole page of
 *     metrics.
 *   - **Scrollbars are hidden.** The overlay scrollbar fades on a timer, so
 *     whether it is in the frame depends on how fast the machine was.
 *   - **`reducedMotion: "reduce"`** is set for this lane in
 *     `playwright.config.ts`, and it is not a duplicate of the line above.
 *     `animations: "disabled"` reaches CSS animations only; the media query
 *     is what quiets motion driven from JavaScript.
 *
 * # Masks, and the clock this deliberately does not freeze
 *
 * Every time-derived label the console paints is *relative to now* —
 * `relativeTime` in `../../src/views/workflows/run-health.ts`, `shortAgo` in
 * `../../src/lib/language.ts` — and is computed from a host-side row. Pinning
 * `Date.now()` in the page with `page.clock` would therefore make those labels
 * less stable rather than more: the host keeps real time, so a frozen client
 * clock turns "just now" into a distance that grows by a day every day. The
 * page is left on the real clock and anything genuinely volatile is masked
 * instead.
 *
 * {@link VOLATILE} is what carries that. Masked regions are filled with a flat
 * colour in both the baseline and the comparison, so the layout around them is
 * still under test and only the glyphs inside are excused.
 *
 * It holds one entry, and on purpose: mark the element at its call site with
 * `data-visual-volatile` rather than adding a CSS path here. A path stops
 * masking anything the day the markup it names changes, and a mask that matches
 * nothing looks exactly like a mask that was never needed — the run just goes
 * red somewhere unrelated. The attribute moves with the element.
 */
test.skip(!VISUAL, VISUAL_REASON);

/**
 * The surfaces worth a baseline: one per top-level destination in the shell,
 * which is the granularity at which a token or font regression shows up.
 *
 * Detail views are deliberately absent. They are reached by clicking through a
 * list whose contents depend on host state, so their baselines would be a test
 * of the fixture rather than of the rendering, and the shell, the cards and the
 * type scale they are made of are already covered by the index above them.
 *
 * `#/team` is absent for a different reason, and it is the reason to keep this
 * list short: `console-routes.ts` rewrites it to `#/company`, so recording both
 * produced two byte-identical PNGs. A duplicate baseline costs a megabyte and
 * catches nothing the original does not, while looking like coverage.
 */
/** One baselined destination: where it lives, and anything it must wait on. */
type Surface = {
  name: string;
  hash: string;
  /** Extra wait before the screenshot, for a surface that settles late. */
  settle?: (page: Page) => Promise<void>;
};

const SURFACES: Surface[] = [
  { name: "tasks", hash: "/#/company/work/tasks" },
  { name: "workflows", hash: "/#/workflows" },
  { name: "company", hash: "/#/company" },
  { name: "memory", hash: "/#/memory" },
  { name: "inbox", hash: "/#/inbox" },
  { name: "approvals", hash: "/#/approvals" },
  { name: "settings", hash: "/#/settings" },
];

/**
 * Regions excused from comparison. Everything here is painted from a value that
 * differs between two runs of the same code — not from styling that could
 * regress.
 */
const VOLATILE = ["[data-visual-volatile]"];

/** The single content sheet every surface paints inside (`content-surface.tsx`). */
const CONTENT_SURFACE = '[data-testid="content-surface"]';

/**
 * Selectors for the loading placeholders a surface mounts before its own read
 * answers. The views share no single loaded marker, so the inverse is the
 * signal: a surface has settled once none of these is inside it. Matching
 * nothing is the success case — only a surface still waiting on its first read
 * renders one, and the wait below is what keeps a slow-host `--update-snapshots`
 * run from recording a skeleton as a baseline.
 */
const LOADING_PLACEHOLDERS = [
  // Every view that reads before painting uses the shared Skeleton component.
  `${CONTENT_SURFACE} >> [data-slot="skeleton"]`,
  // Approvals pulses its own rows instead of the Skeleton component.
  `${CONTENT_SURFACE} >> [aria-label="Loading approvals"]`,
  // Settings' sub-pages say what they are waiting on beside a spinner, not
  // via the shared Skeleton — `Loader2` plus a label in `domain-settings.tsx`
  // and `policy-settings.tsx`. Each is named so a slow `--update-snapshots`
  // run cannot record a half-loaded Settings surface as a baseline.
  `${CONTENT_SURFACE} >> text=Loading domain…`,
  `${CONTENT_SURFACE} >> text=Loading email settings…`,
  `${CONTENT_SURFACE} >> text=Loading the current policy…`,
];

/** The first-run tour opens a modal over a fresh console and eats every click. */
async function skipTour(page: Page) {
  await page.addInitScript(() => {
    const real = Storage.prototype.getItem;
    Storage.prototype.getItem = function getItem(key: string) {
      return key.startsWith("oc-tour:") ? '{"skipped":true}' : real.call(this, key);
    };
  });
}

/**
 * Hide overlay scrollbars for the duration of the shot.
 *
 * They fade on a timer after the last scroll, so whether one is in the frame is
 * a race against how quickly the machine got here — the single most reliable
 * way to make a screenshot suite flake. `shell-scrollbars.spec.ts` is what
 * asserts they are correct; this lane only needs them absent.
 */
async function hideScrollbars(page: Page) {
  await page.addStyleTag({
    content: `
      *::-webkit-scrollbar { display: none !important; }
      * { scrollbar-width: none !important; }
    `,
  });
}

/** Pin the theme before the app boots, so the first paint is the one under test. */
async function open(page: Page, theme: "dark" | "light", hash: string) {
  await page.addInitScript((value) => {
    window.localStorage.setItem("theme", value);
  }, theme);
  await page.goto(hash);

  await expect(page.locator(CONTENT_SURFACE)).toBeVisible({ timeout: 30_000 });
  await expect(page.locator("html")).toHaveClass(new RegExp(`\\b${theme}\\b`));

  // A baseline recorded while a surface still shows its loading placeholder is
  // a baseline of the placeholder: the views mount their real content only
  // after their own read answers, which on a slow host outlive the shell.
  // Wait the placeholders out so a slow `--update-snapshots` run records the
  // same pixels a fast one would have had in the frame.
  for (const loading of LOADING_PLACEHOLDERS) {
    await expect(page.locator(loading)).toHaveCount(0, { timeout: 30_000 });
  }

  // A baseline recorded mid-flash — fallback metrics, Geist not yet applied —
  // differs from every later run by the whole page.
  await page.evaluate(() => document.fonts.ready);
  await hideScrollbars(page);
}

for (const theme of ["light", "dark"] as const) {
  for (const surface of SURFACES) {
    test(`${surface.name} renders as recorded (${theme})`, async ({ page }) => {
      await skipTour(page);
      await open(page, theme, surface.hash);
      await surface.settle?.(page);

      await expect(page).toHaveScreenshot(`${surface.name}-${theme}.png`, {
        fullPage: true,
        animations: "disabled",
        caret: "hide",
        mask: VOLATILE.map((selector) => page.locator(selector)),
        // Antialiasing of the same glyph differs by a pixel or two between two
        // runs on the same machine; a token that changed lightness moves
        // thousands. This threshold separates those two without being wide
        // enough to hide a missing card.
        maxDiffPixelRatio: 0.002,
      });
    });
  }
}
