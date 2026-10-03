import { lazy, Suspense, type ReactNode } from "react";

import type { OpenCompanyClient } from "@/api/client";
import { RouteLoading } from "@/components/route-loading";
import type { CompanyFeed } from "@/hooks/use-company";
import { cn } from "@/lib/utils";
import { FeedbackView } from "@/views/FeedbackView";
import { PeopleView } from "@/views/PeopleView";
import { AppearanceView } from "@/views/settings/AppearanceView";
import { ApprovalsSettingsView } from "@/views/settings/ApprovalsSettingsView";
import { SettingsView } from "@/views/SettingsView";
import {
  SETTINGS_PAGE_GROUPS,
  SETTINGS_PAGES,
  resolveSettingsPage,
  type SettingsPage,
} from "@/views/settings-pages";

// The table itself lives in `settings-pages.ts` so that prose pointing at a
// sub-page can name one without importing this section and everything under it.
export { SETTINGS_PAGES, type SettingsPage };

// Recharts is heavy and only used here — load the usage dashboard on demand.
const UsageView = lazy(() => import("@/views/UsageView").then((m) => ({ default: m.UsageView })));

// Same rule, same reason: the Observatory pulls its own charting and DAG code,
// and an operator who never opens it should not pay for it. `app-shell.tsx`
// lazies the same module for the run-detail route; two `lazy()` calls over one
// import share a chunk, so this costs nothing beyond the second boundary.
const ObservatoryView = lazy(() =>
  import("@/views/observatory/ObservatoryView").then((m) => ({ default: m.ObservatoryView })),
);

interface Props {
  client: OpenCompanyClient;
  company: string | null;
  feed: CompanyFeed;
  /** The hash's second segment, e.g. `people` in `#/settings/people`. */
  sub: string | null;
  /**
   * The shell's re-read tick, forwarded to the Observatory index.
   *
   * The same value `app-shell.tsx` hands the run-detail route: one counter for
   * both signals a re-read should follow — a workflow run moved, or a workflow
   * node started or settled. Passed through rather than defaulted to a constant
   * because a constant is an index that never refreshes, which on the one page
   * built to watch runs in flight is the whole feature.
   */
  eventTick: number;
  onFlag: () => void;
  /** Start the reset (archive + start clean) flow for the active company (#1807). */
  onResetCompany?: (id: string, name: string) => void;
  /**
   * The company/host switcher, heading the rail. It headed the floating
   * sidebar first; which company a window points at is set once and rarely,
   * which is what this page is for, so it moved here.
   */
  switcher?: ReactNode;
}

/**
 * Settings, as a section rather than a page.
 *
 * Everything that configures the company rather than running it lives here,
 * behind a sub-sidebar: the connection and lifecycle controls, who can sign
 * in, what its teammates actually did, and what it spends. Each is its own
 * route (`#/settings/people`), so a sub-page is linkable and survives a
 * refresh exactly as a top-level view does.
 *
 * Everything with an outside service at the other end of it used to be here
 * too — apps, tool servers, the model, skills, hosting, search. All six are
 * the Connections section now; `views/connection-pages.ts` argues why, and
 * `console-route-rewrites.ts` keeps their old `#/settings/…` addresses
 * resolving. A new row that names an outside service belongs there, not
 * here.
 */
export function SettingsSection({
  client,
  company,
  feed,
  sub,
  eventTick,
  onFlag,
  onResetCompany,
  switcher,
}: Props) {
  const page = resolveSettingsPage(sub);
  const activePage = SETTINGS_PAGES.find((item) => item.id === page)!;

  return (
    <div className="flex min-h-0 flex-1">
      <nav
        aria-label="Settings"
        className="hidden w-60 shrink-0 flex-col gap-0.5 overflow-y-auto border-r p-3 lg:flex"
      >
        {/* No "Settings" caption. It was a visual label for a rail that is
            already unmistakable: you arrive here from the Settings row in the
            sidebar footer, the page beside it says "General settings", and
            every group below carries its own heading. A word repeated three
            times on one screen is furniture. Nothing is lost for a screen
            reader either — the caption was deliberately a `div` rather than an
            `h2` (issue #1392), so it was never in the document outline, and
            the `nav`'s own `aria-label` still names this landmark. */}
        {switcher && (
          <div data-testid="settings-switcher" className="pb-2">
            {switcher}
          </div>
        )}
        {SETTINGS_PAGE_GROUPS.map((group) => (
          <section key={group.id} aria-labelledby={`settings-group-${group.id}`}>
            {/* Named by `aria-labelledby`, which resolves against any element,
                so the group keeps its accessible name without sitting in the
                document outline ahead of the sub-page's `h1` (issue #1392). */}
            <div
              id={`settings-group-${group.id}`}
              className="px-2 pb-1 pt-3 text-xs font-medium tracking-wide text-muted-foreground uppercase first:pt-1"
            >
              {group.label}
            </div>
            {SETTINGS_PAGES.filter((item) => item.group === group.id).map((item) => (
              // One line per row, and the row's own `title` carries what the
              // second line used to say (issue #2131). The hint was rendered
              // under every label here, and at `w-60` most of them wrapped:
              // "Approvals, connection, lifecycle, domain, mail" is three
              // lines, "What your teammates actually did" is two, and eight
              // rows of that is a wall rather than a list you can scan. The
              // count is `SETTINGS_PAGES.length`, so read it there rather than
              // trusting this sentence after the next page lands. The labels
              // are the navigation; the hint is a gloss, and a gloss that
              // triples the height of the thing it explains has stopped
              // helping.
              <a
                key={item.id}
                href={`#/settings/${item.id}`}
                title={item.hint}
                aria-current={page === item.id ? "page" : undefined}
                className={cn(
                  "flex items-center gap-2.5 rounded-lg px-2 py-2 text-left transition-colors",
                  page === item.id ? "bg-rail-selected text-rail-selected-foreground" : "hover:bg-rail-hover",
                )}
              >
                <item.icon
                  className={cn(
                    "size-4 shrink-0",
                    page === item.id ? "text-rail-selected-foreground" : "text-muted-foreground",
                  )}
                />
                <span className="min-w-0 truncate text-sm font-medium">{item.label}</span>
              </a>
            ))}
          </section>
        ))}
      </nav>

      {/* Below `lg` the rail collapses to a scrolling row of chips, so the
          sub-pages stay reachable without a second drawer. The breakpoint is
          `lg`, not `sm`: from 768–1023px the app sidebar is still on, and a
          second `w-60` rail here would squeeze the settings pane below the
          width its widest card (SMTP) needs, clipping it on both sides
          (issue #1383). */}
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {/* On the macOS desktop, `ContentSurface` overlays every page's top
            28px with an absolutely-positioned, pointer-events-enabled drag
            band (`WindowDragBar`, z-20) so the window stays movable without a
            native title bar — content-surface.tsx explains the trade-off it
            accepted: that band wins the click over whatever a page draws
            underneath it. This row is the one page top that actually sits in
            that band below `lg`, so without a higher stacking order its links
            are unreachable at 880–1023px window widths on macOS. `relative
            z-30` gives it its own stacking context above the drag band without
            touching `WindowDragBar` itself, whose absolute-overlay contract
            other pages (the graph, the workflow editor) still rely on. */}
        {/* Both `hint` readers below survive #2131, which was about the
            desktop rail. This row is a different surface with a different
            problem: the chips carry the label alone, so the `title` is the only
            gloss a chip has, and the line under them describes the *active*
            page rather than repeating itself under every one of them. Neither
            is a second line per row, which is the thing that was removed. */}
        <div className="relative z-30 border-b lg:hidden">
          {switcher && <div className="px-2 pt-2">{switcher}</div>}
          <div className="flex gap-1 overflow-x-auto p-2">
            {SETTINGS_PAGES.map((item) => (
              <a
                key={item.id}
                href={`#/settings/${item.id}`}
                title={item.hint}
                aria-current={page === item.id ? "page" : undefined}
                className={cn(
                  "shrink-0 rounded-full px-3 py-1 text-xs font-medium transition-colors",
                  page === item.id ? "bg-rail-selected text-rail-selected-foreground" : "text-muted-foreground",
                )}
              >
                {item.label}
              </a>
            ))}
          </div>
          <p className="px-3 pb-2 text-xs text-muted-foreground">{activePage.hint}</p>
        </div>

        {page === "general" && (
          <SettingsView
            client={client}
            company={company}
            feed={feed}
            onFlag={onFlag}
            onResetCompany={onResetCompany}
          />
        )}
        {page === "people" && <PeopleView client={client} company={company} />}
        {/* Both were cards on General. See their own files for why each left. */}
        {page === "approvals" && <ApprovalsSettingsView client={client} company={company} />}
        {page === "appearance" && <AppearanceView />}
        {/* The same page `#/feedback` renders, re-parented rather than
            rewritten. That top-level address still resolves — the flag dialog
            and the board's own links point at it — so nothing that names it
            breaks; this is where the rail reaches it from. */}
        {page === "feedback" && <FeedbackView client={client} company={company} />}
        {/* The run index, rendered here rather than bounced to `#/observatory`.
            The row on this rail used to be a doorway — the address was rewritten
            away before this dispatch ever saw it — because the view reads its
            own query keys off the hash and they were keyed on the head being
            `observatory`. `readObservatoryHash` answers to both heads now, so
            `?tab=analytics` is addressable from here.

            `runId={null}` always: a single run is `#/observatory/<runId>`, a
            top-level route `app-shell.tsx` still owns, because `useHashView`
            carries two segments and this page is already using the second.
            `eventTick` is the same counter that route is handed, so the index
            re-reads on a run moving whichever of the two addresses is open. */}
        {page === "observatory" && (
          <Suspense fallback={<RouteLoading title="Observatory" label="Loading observatory…" />}>
            <ObservatoryView
              client={client}
              company={company}
              runId={null}
              eventTick={eventTick}
            />
          </Suspense>
        )}
        {/* OAuth, MCP Servers, Inference and Skills were all here. They are the
            Connections section now (`#/connections/apps`, `/mcp`, `/inference`,
            `/skills`) — each is read repeatedly and changes as the company's
            work does, and a settings rail is where an operator changes
            configuration once. Every one of those addresses still resolves,
            rewritten by `console-route-rewrites.ts`. Hosting and Search went
            with them, which emptied the Integrations group and retired it —
            `connection-pages.ts` carries the argument. */}
        {/* Billing was here. It moved to Finance → Invoicing and Finance → Wallet
            (docs/spec/runtime/finance-console.md): a credential form belongs
            beside the data it unlocks, and "Billing" read as *what OpenCompany
            charges me* — which is Usage, two rows down. */}
        {/* Observatory has a row on this rail but renders nothing here: the row
            is a doorway, and `#/settings/observatory` is rewritten onto
            `#/observatory` before it ever reaches this dispatch.

            Not squeamishness about a nested route — it is that the Observatory
            owns four query keys of its own (`tab`, `agent`, `turn`, `step`) and
            reads them straight off `window.location`, keyed on the hash's head
            being `observatory` (`views/observatory/hash.ts`). Rendered under
            `#/settings/…` that head is `settings`, so `writeObservatoryQuery`
            goes silent and the analytics tab, the open agent thread and the
            expanded turn all stop being addressable. A surface with its own
            address grammar has to keep its own address. */}
        {page === "usage" && (
          <Suspense fallback={<RouteLoading title="Usage" label="Loading usage…" />}>
            <UsageView client={client} company={company} />
          </Suspense>
        )}
      </div>
    </div>
  );
}
