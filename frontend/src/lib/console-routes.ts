/**
 * The console's route table: every surface `AppShell` can render, and the
 * allow-list `useHashView` validates a hash against.
 *
 * # The rule
 *
 * **`NAV` is presentation. `VIEWS` is routing.** A view routes because it is a
 * member of `View` below and therefore has a `ROUTABLE` entry — never because
 * it happens to have a sidebar row. Taking a row out of `NAV` (issue #302 for
 * Inbox and Finances, #1140 for the Tasks board, #1141 for Team, #1172 for
 * Pages) hides a surface; it does not retire its address. Retiring an address
 * is a separate, deliberate act: drop the view from the union here, delete its
 * render block in `app-shell.tsx`, and send the old address somewhere real with
 * the shell's `REWRITE_RETIRED`.
 *
 * # Why this file exists
 *
 * Routing used to be `[...NAV.map((i) => i.view), ...HIDDEN_VIEWS]` — the nav
 * list plus a hand-maintained second list of views deliberately absent from it.
 * That made "hidden" and "routable" two halves of one treatment that had to be
 * applied together, and issue #1311 is what happens when only one half lands:
 * #1172 commented Pages out of `NAV`, wrote a comment promising `#/pages`
 * "stays live", and never added `pages` to the second list. The address
 * collapsed onto Overview for four months while the shell's `view === "pages"`
 * block, the `lazy()` import above it and the whole sandboxed-iframe surface
 * behind it (`docs/spec/runtime/pages.md`) sat unreachable.
 *
 * `ROUTABLE` is a `Record<View, true>`, so it is complete *by construction*:
 * a view in the union with no entry is a compile error, and `npm run typecheck`
 * is the first step of every console CI lane. There is no longer a second list
 * to forget — a surface the shell can render is a surface an address can reach.
 *
 * Keeping the table in a plain `.ts` module is also what makes it testable:
 * the unit lane (`frontend/vitest.config.ts`) collects the `.test.ts` files
 * under `test/unit` with no React plugin, so `test/unit/task-route.test.ts`
 * — which needs the shell's route table — had to *duplicate*
 * the shell's route table verbatim to reach it — which is precisely why nothing
 * noticed #1311. `test/unit/console-routes.test.ts` imports the real thing.
 */

export type View =
  | "overview"
  | "company"
  | "chat"
  | "conversation"
  | "inbox"
  | "tasks"
  | "ledgers"
  | "team"
  | "workspace"
  | "memory"
  | "approvals"
  | "workflows"
  | "pages"
  | "finances"
  | "settings"
  | "feedback";

/**
 * Every routable view, one entry per member of `View` — the compiler enforces
 * that, which is the point (see the header).
 *
 * The entries with no `NAV` row in `app-shell.tsx` are annotated with why they
 * have none. They are parked rather than retired: their host routes, stores and
 * e2e specs are untouched, and re-listing one in `NAV` is all it takes to bring
 * it back. Issue #1337 is the open question of what a parked surface should
 * *say* to an operator who arrives on one; this file only decides that they
 * still answer.
 */
const ROUTABLE: Record<View, true> = {
  overview: true,
  company: true,
  chat: true,
  /**
   * No nav row: the surface the Chat workspace replaces. Everything it can do
   * chat can do in one screen — including the teammate budget controls
   * `MembersPane` ported from Team (issue #360) — but it keeps answering
   * `#/conversation` until chat covers the last of what it still does better (a
   * desk's persisted transcript).
   */
  conversation: true,
  /** No nav row: parked by issue #302, host routes and per-agent store intact. */
  inbox: true,
  /**
   * No nav row, and the load-bearing entry of issue #1140. The board page is
   * gone — bare `#/tasks` is rewritten onto the board's ledger by the shell's
   * `REWRITE_RETIRED` — but `#/tasks/<id>` is the card detail (the timeline,
   * the plan brief, the discussion, the attempts, the steer controls), linked
   * from chat, from an approval card, from a workflow run's rows and from every
   * card on the board. Ledgers deliberately reproduces none of it. Drop this
   * entry and `useHashView` discards the head *and* its sub-page, so every one
   * of those links quietly lands on Overview instead of the card it named.
   */
  tasks: true,
  ledgers: true,
  /**
   * No nav row, for a narrower reason than it used to have (issue #1141). Bare
   * `#/team` is rewritten to `#/company`, whose Cards half is the grid Team
   * used to draw. What this entry keeps alive is `#/team/<agentId>`: the
   * teammate detail page, deliberately a page rather than a modal so it can be
   * linked, and linked to today from the org chart's seats, its "Not on a desk"
   * chips and the chat member pane.
   */
  team: true,
  workspace: true,
  memory: true,
  approvals: true,
  workflows: true,
  /**
   * No nav row (issues #1171, #1172) — and this entry is the half of that
   * removal which went missing until issue #1311. Pages is the agent-authored
   * dashboard surface: a sandboxed iframe, a per-document capability and the
   * `oc:graphql` bridge (`docs/spec/runtime/pages.md`), hardened as recently as
   * #1122 and #1221. Without an entry here `#/pages` is an unknown hash, so the
   * shell's `view === "pages"` block is dead code and `nav_visible = false` in a
   * `page.toml` — documented as keeping a page "reachable only by direct URL" —
   * has no URL to be reachable by.
   */
  pages: true,
  /**
   * Un-parked. Issue #302 took the nav row off a flat Finances page; what has a
   * row again is a *section* — the same ledger projection as its Overview, plus
   * Invoicing (Chargebee) and Wallet (PayPal), which are live provider surfaces
   * the host had no HTTP route for until `server::ops::finance`. Its sub-pages
   * ride the second hash segment (`#/finances/wallet`), so this one entry
   * routes all three; see docs/spec/runtime/finance-console.md.
   */
  finances: true,
  settings: true,
  /** No nav row: linked from the sidebar footer instead. */
  feedback: true,
};

/**
 * The allow-list `useHashView` validates a hash's first segment against.
 *
 * Derived from `ROUTABLE`, so it can never be a subset of the views the shell
 * renders. Order is irrelevant — the hook only ever asks whether a head is a
 * member.
 */
export const VIEWS: View[] = Object.keys(ROUTABLE) as View[];
