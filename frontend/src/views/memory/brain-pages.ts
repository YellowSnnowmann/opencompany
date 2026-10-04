// The Brain section's sub-page table, and the helpers that read it.
//
// A leaf module, exactly as `views/connection-pages.ts`, `views/settings-pages.ts`
// and `views/finance/finance-pages.ts` are, and for the same reason: the nav
// table has to name these pages, and a static import of `MemoryView` from there
// would pull the whole memory browser — its virtual list and its dialogs —
// into a module the sidebar renders on every route.
//
// # Why Brain is two tabs
//
// It was one page: an engine picker, then the drop zone, then the browser,
// stacked in a column. An operator arriving to answer "does it already know
// this" — which is what this surface is for, and the reason it left the
// settings rail (issue #1416) — scrolled past a provider form and an upload
// target to reach it, every time. The three do not change at the same rate
// either: the engine is set once and then almost never, an upload happens when
// a document arrives, the browser is read constantly.
//
// The engine picker's Settings tab went with the OpenHuman memory v2 cutover:
// the engine is OpenHuman's `[memory]` config now, not a console choice, and
// the Overview reports whether memory is on.
//
// So they were split into three rail rows. That over-corrected. Three rows
// under a caption said Brain was three destinations, when it is one subject —
// what the company remembers — looked at three ways, and it spent three of the
// sidebar's scarce rows saying so. They are tabs in the page's own header now
// (`components/page-tabs.tsx`), and Brain is one row again.
//
// # The addresses did not change
//
// `#/company/brain/upload` still opens Upload. Unlike the pages that carry
// their tab in `?tab=`, Brain's tabs stay on the path segment they already
// owned: these were real addresses the sidebar deep-linked and operators
// bookmarked, and a query-string move would have retired every one of them to
// buy nothing. `PageTabs` is controlled, so what a page routes its tabs
// through is the page's own business.

import { FileUp, Brain as BrainIcon, type LucideIcon } from "lucide-react";

/**
 * The tabs across Brain's header. The id is the third hash segment —
 * `#/company/brain/upload`.
 *
 * Overview leads because it is what the section is *for*, and because a bare
 * `#/company/brain` has to land somewhere: the same rule Finance's Overview and
 * Connections' Apps follow, so a section is never an empty frame.
 */
export const BRAIN_PAGES = [
  {
    id: "overview",
    label: "Overview",
    icon: BrainIcon,
    hint: "Everything the company remembers",
  },
  {
    id: "upload",
    label: "Upload",
    icon: FileUp,
    hint: "Add documents for it to remember",
  },
] as const satisfies readonly { id: string; label: string; icon: LucideIcon; hint: string }[];

export type BrainPage = (typeof BRAIN_PAGES)[number]["id"];

export const DEFAULT_BRAIN_PAGE: BrainPage = "overview";

/** Whether a hash segment names a real tab. */
export function isBrainPage(sub: string | null): sub is BrainPage {
  return BRAIN_PAGES.some((page) => page.id === sub);
}

/** The tab a hash segment resolves to, defaulting to Overview. */
export function resolveBrainPage(sub: string | null): BrainPage {
  return isBrainPage(sub) ? sub : DEFAULT_BRAIN_PAGE;
}
