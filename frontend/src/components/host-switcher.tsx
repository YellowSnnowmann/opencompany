// Which host this console is looking at, in the sidebar header.
//
// It replaces the icon rail down the left edge (issue #1142), which showed one
// building glyph per host with a status dot and a `+`: no names, no keyboard
// access, and status reduced to a colour.
//
// ## The rule this component inherits
//
// Choosing a host is a **filter over N live things**, not a selector that makes
// one live. It changes which console is on screen and nothing else: no
// connection is torn down, no stream is closed, no storage is re-scoped.
// Selection lives in `App`'s local state and reaches here through
// `HostsContext`, which carries the long version of that rule.
//
// ## Why the trigger carries a dot
//
// The rail's amber dots were the only at-a-glance sign that something was wrong
// on a host you were *not* looking at. A dropdown hides its rows, so the signal
// has to survive on the trigger: the dot there is the **worst** state across
// every host, and `data-worst-status` says the same thing to a test. Without it
// removing the rail would be a net loss for anyone running more than one host.

import { Building2, Check, ChevronsUpDown, Plus, Settings2 } from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar";
import { isDesktopRuntime } from "@/api/transport";
import { hostShortcutLabel, useHosts } from "@/connections/HostsContext";
import type { Connection, ConnectionStatus } from "@/connections/types";
import { cn } from "@/lib/utils";

/**
 * How a connection's state reads, and what colour says so.
 *
 * The `label` is the load-bearing half and the dot is the shorthand, not the
 * other way round: a row that is anything but `live` prints its state (issue
 * #1167), because hue alone tells an operator that *something* differs without
 * saying what, and tells someone who cannot separate amber from green nothing.
 */
export const STATUS_COPY: Record<ConnectionStatus, { label: string; dot: string }> = {
  connecting: { label: "Connecting…", dot: "bg-muted-foreground/50" },
  live: { label: "Connected", dot: "bg-status-done" },
  degraded: { label: "Partly available", dot: "bg-status-blocked" },
  down: { label: "Unreachable", dot: "bg-destructive" },
  unauthenticated: { label: "Sign-in needed", dot: "bg-status-blocked" },
};

/**
 * How a host's state reads on its own row.
 *
 * {@link STATUS_COPY} keyed by status, except for the one case where the
 * status alone is not the whole answer: a cloud tenant that has been idle is
 * being *started*, not contacted, and that takes seconds rather than
 * milliseconds. "Connecting…" for that long reads as a hang, so the connector
 * supplies the word — which is why this takes a connection rather than a
 * status.
 *
 * Deliberately not a fifth `ConnectionStatus`. Waking is a connecting host and
 * ranks exactly like one on the trigger; a new status would need a place in the
 * severity ordering, and every `switch` over the union would have to grow a
 * branch for a state that behaves identically.
 */
export function statusCopy(connection: Connection): { label: string; dot: string } {
  const copy = STATUS_COPY[connection.status];
  return connection.waking ? { ...copy, label: "Waking…" } : copy;
}

/**
 * How loudly each state asks to be noticed.
 *
 * Highest wins on the trigger. `down` outranks `unauthenticated` because one is
 * a host that is gone and the other is a host that is there and waiting; and
 * `connecting` outranks `live` only so a roster still settling does not claim
 * to be entirely fine.
 */
const SEVERITY: Record<ConnectionStatus, number> = {
  live: 0,
  connecting: 1,
  degraded: 2,
  unauthenticated: 3,
  down: 4,
};

/** The state the trigger reports: the worst one any host is in. */
export function worstStatus(connections: Connection[]): ConnectionStatus | null {
  let worst: ConnectionStatus | null = null;
  for (const connection of connections) {
    if (worst === null || SEVERITY[connection.status] > SEVERITY[worst]) {
      worst = connection.status;
    }
  }
  return worst;
}

/**
 * Whether the switcher opens a menu, or is only a nameplate.
 *
 * One host is the ordinary web deployment, and a chevron over a menu of one is
 * furniture. It becomes a control when there is a choice to make.
 *
 * **The desktop is always interactive**, whatever the count. The old rule made
 * an exception only for zero, on the reasoning that a browser always has its
 * bootstrap connection so a real zero could only happen in the desktop — and
 * that a desktop holding nothing needs the "Add a host" this menu carries.
 * Issue #613 moved where that bites: the desktop no longer writes a same-origin
 * bootstrap row, so a desktop whose embedded host *did* start now holds exactly
 * **one** connection, which is its ordinary state rather than a rare one. Under
 * the old rule that state offered no menu, and "Add a host" is the only way to
 * reach a second host — so the working case became the one with no way out of
 * it, which is the dead end the zero exception existed to prevent.
 *
 * **A hub is always interactive too**, for the same reason and by the same
 * argument. A hub has no bootstrap connection either (its origin serves assets,
 * not a host), so it holds exactly the hosts someone added — and at zero "Add a
 * host" is the only way to add the first one.
 *
 * An ordinary same-origin browser console is unaffected: neither
 * `isDesktopRuntime()` nor `hub` is true there, so one host still draws a plain
 * nameplate with no chevron.
 *
 * This is the predicate `connectionRailVisible` used to be — same rule, moved
 * with the control it governs.
 *
 * It answers **two** of the three questions the switcher asks, and no longer
 * the third: whether the floating standalone switcher is drawn at all, and
 * whether the status dot earns its place. Whether the trigger *opens a menu*
 * is {@link hostSwitcherMenu}, which parted company with this rule once the
 * menu started carrying host management.
 */
export function hostSwitcherInteractive(count: number, hub = false): boolean {
  return count >= 2 || hub || isDesktopRuntime();
}

/**
 * Whether the trigger opens a menu, or is only a nameplate.
 *
 * This used to be {@link hostSwitcherInteractive} exactly, on the argument
 * that a chevron over a menu of one is furniture. That argument was sound
 * while the menu held nothing but the roster and "Add a host": with one host
 * there is nothing to switch to, and adding a second is the desktop's and the
 * hub's business.
 *
 * "Manage hosts" retires it. The menu now carries the *only* way to rename,
 * re-address or forget a host — and an ordinary browser console with exactly
 * one connection is precisely the arrangement whose host is most likely to
 * move, since that bootstrap row is a plain `remote` connector (see
 * `DEFAULT_CONNECTOR`) and therefore fully manageable. Under the old rule that
 * console got a nameplate with no menu, so its one host could not be reached
 * at all: a menu of one is not furniture once one is a menu of something to
 * *do*.
 *
 * So: any host at all opens a menu. The zero cases are unchanged and still
 * belong to the desktop and the hub, which need "Add a host" to have anything
 * at all. A single `local` host — the one connector this page cannot manage —
 * only ever exists on the desktop, which is already true here.
 */
export function hostSwitcherMenu(count: number, hub = false): boolean {
  return count >= 1 || hostSwitcherInteractive(count, hub);
}

interface Props {
  /**
   * Which chrome this is drawn in.
   *
   * `sidebar` is the home: the header row of the app shell. `standalone` is for
   * the screens that have no shell — a host that cannot be reached, a sign-in,
   * the desktop's "no host to show" — where the switcher is the only way back
   * to a host that works, and where its absence is what stranded an operator
   * when the rail was the one holding it.
   */
  variant?: "sidebar" | "standalone";
  /**
   * The company on screen, when there is one.
   *
   * The trigger names the company because that is what the person is looking
   * at; the host is the second line, and only when more than one is connected.
   * Absent on the standalone chrome, which has no company yet.
   */
  companyName?: string;
}

export function HostSwitcher({ variant = "sidebar", companyName }: Props) {
  const hosts = useHosts();
  const { connections, selected, hub } = hosts;

  const interactive = hostSwitcherInteractive(connections.length, hub);
  const menu = hostSwitcherMenu(connections.length, hub);
  const active = connections.find((c) => c.id === selected) ?? null;
  const worst = worstStatus(connections);

  // Nothing to switch and no shell to sit in: the ordinary single-host web
  // console gets exactly what it had before, which is nothing at all.
  if (variant === "standalone" && !interactive) return null;

  // Shown when it has something to say. On the ordinary single-host console the
  // switcher is a nameplate, and a permanently green dot beside it is furniture
  // — that console's host being unreachable is a full-screen error, not a
  // detail to notice in the corner. The dot earns its place once there is a
  // host you are *not* looking at, or once something is actually wrong.
  const dot =
    worst && (interactive || worst !== "live") ? (
      <span
        data-testid="host-switcher-status"
        className={cn(
          "absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2",
          // The ring is a CUT-OUT of the ground, and the two variants stand on
          // different grounds (issue #1178). In the shell the trigger has no
          // fill at rest, so half this ring lands on the window chrome; the
          // standalone console draws its own `bg-sidebar` card behind it.
          variant === "sidebar" ? "ring-chrome" : "ring-sidebar",
          STATUS_COPY[worst].dot,
        )}
      />
    ) : null;

  const glyph = (
    <div className="relative flex aspect-square size-8 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground">
      <Building2 className="size-4" />
      {dot}
    </div>
  );

  const primary = companyName ?? active?.label ?? "No host";
  // The host on the second line, but only when it says something the first line
  // does not. Most hosts serve one company and are named after it, so repeating
  // the name in both rows would spend the only line the trigger has on nothing.
  const secondary = companyName
    ? connections.length > 1 && active && active.label !== companyName
      ? active.label
      : "Current company"
    : worst
      ? STATUS_COPY[worst].label
      : "Not connected";

  const nameplate = (
    <>
      {glyph}
      <div className="grid flex-1 text-left leading-tight">
        <span className="truncate text-sm font-semibold">{primary}</span>
        <span className="truncate text-xs text-muted-foreground">{secondary}</span>
      </div>
    </>
  );

  // Read off the closed trigger, so host count and cross-host health stay
  // answerable — by an operator and by a test — without opening anything. That
  // is what the rail gave away for free and what a dropdown otherwise hides.
  const triggerData = {
    "data-testid": "host-switcher",
    "data-host-count": connections.length,
    "data-worst-status": worst ?? "none",
  };

  if (!menu) {
    return (
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton
            size="lg"
            className="cursor-default hover:bg-transparent"
            {...triggerData}
          >
            {nameplate}
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    );
  }

  const renderMenu = (side: "right" | "bottom") => (
    <DropdownMenuContent className="min-w-72 rounded-lg" align="start" side={side} sideOffset={4}>
      {/* `DropdownMenuLabel` is Base UI's `Menu.GroupLabel`, and it throws
            outside a `Menu.Group` — the group is what it labels. */}
      <DropdownMenuGroup>
        <DropdownMenuLabel>Hosts</DropdownMenuLabel>
        {connections.map((connection, index) => {
          const status = statusCopy(connection);
          const shortcut = hostShortcutLabel(index);
          return (
            <DropdownMenuItem
              key={connection.id}
              data-testid={`host-row-${connection.id}`}
              data-status={connection.status}
              aria-current={connection.id === selected}
              // Native `title`, not a tooltip component: the reason a host is
              // unreachable has to be readable without extra machinery on a
              // row that must render while its host is down.
              title={`${connection.label} — ${status.label}${
                connection.error ? `\n${connection.error}` : ""
              }`}
              onClick={() => hosts.onSelect(connection.id)}
              className="gap-2"
            >
              <span className={cn("size-2 shrink-0 rounded-full", status.dot)} />
              <span className={cn("flex-1 truncate", connection.id === selected && "font-medium")}>
                {connection.label}
              </span>
              {connection.status === "live" ? (
                // Nothing to say out loud on a host that is simply working, so
                // the state stays where a screen reader can still reach it.
                <span className="sr-only">{status.label}</span>
              ) : (
                // In words, not in hue (issue #1167). A colour is no help to
                // anyone who cannot tell these two apart, and even where it is
                // read correctly "amber" does not say *what* is wrong —
                // leaving an operator to discover an unreachable host by
                // landing on its failure. This is the row telling them first.
                <span
                  data-testid={`host-row-state-${connection.id}`}
                  className="shrink-0 text-xs text-muted-foreground"
                >
                  {status.label}
                </span>
              )}
              {shortcut && <DropdownMenuShortcut>{shortcut}</DropdownMenuShortcut>}
              {connection.id === selected && <Check className="size-4 shrink-0" />}
            </DropdownMenuItem>
          );
        })}
        {connections.length === 0 && (
          <p className="px-1.5 py-1 text-xs text-muted-foreground">No hosts yet.</p>
        )}
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
      <DropdownMenuItem
        data-testid="host-switcher-add"
        onClick={() => hosts.setAddingHost(true)}
        className="gap-2"
      >
        <Plus className="size-4" />
        Add a host
      </DropdownMenuItem>
      {/* The other half of adding one. Kept out of the rows themselves: a row
          is a *filter* — clicking it puts that host on screen — and hanging an
          edit and a delete off the same row makes a menu whose click targets
          disagree about what a row is for. The page says what each host is,
          which the menu deliberately does not. */}
      <DropdownMenuItem
        data-testid="host-switcher-manage"
        onClick={() => hosts.setManagingHosts(true)}
        className="gap-2"
      >
        <Settings2 className="size-4" />
        Manage hosts
      </DropdownMenuItem>
    </DropdownMenuContent>
  );

  if (variant === "standalone") {
    return (
      <DropdownMenu>
        <DropdownMenuTrigger
          // No `aria-label`: the name has to come from the content, the same
          // way it does in the sidebar. A label saying "Switch host" would
          // override the host's own name and its status — which on this chrome
          // is the whole reason the control is on screen.
          render={
            <button
              type="button"
              className="flex w-64 max-w-[calc(100vw-2rem)] items-center gap-2 rounded-lg border bg-sidebar p-2 text-left shadow-sm transition hover:bg-sidebar-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
            />
          }
          {...triggerData}
        >
          {nameplate}
          <ChevronsUpDown className="ml-auto size-4 shrink-0 text-muted-foreground" />
        </DropdownMenuTrigger>
        {renderMenu("bottom")}
      </DropdownMenu>
    );
  }

  return (
    <SidebarHeaderTrigger triggerData={triggerData} nameplate={nameplate} renderMenu={renderMenu} />
  );
}

/**
 * The switcher over a screen that has no app shell.
 *
 * A host that cannot be reached, a sign-in, a setup wizard, a desktop holding
 * no host at all: none of these mount a sidebar, and all of them are exactly
 * when an operator needs to get to a host that works. The rail used to be on
 * screen behind every one of them for free, because it lived outside the
 * console; the switcher has to be put there deliberately.
 *
 * It draws nothing on an ordinary single-host console, so those screens are
 * unchanged there.
 */
export function ConsoleChrome({ children }: { children: React.ReactNode }) {
  return (
    <div className="relative min-h-svh">
      <div className="absolute top-4 left-4 z-50">
        <HostSwitcher variant="standalone" />
      </div>
      {children}
    </div>
  );
}

/**
 * The sidebar chrome for the trigger.
 *
 * Split out only because `useSidebar` throws outside a `SidebarProvider`, and
 * the standalone variant renders on screens that have no sidebar at all.
 */
function SidebarHeaderTrigger({
  triggerData,
  nameplate,
  renderMenu,
}: {
  triggerData: Record<string, string | number>;
  nameplate: React.ReactNode;
  /** Mobile has no room to the right of the sidebar, so the menu drops instead. */
  renderMenu: (side: "right" | "bottom") => React.ReactNode;
}) {
  const { isMobile } = useSidebar();
  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <DropdownMenu>
          {/* The sidebar button renders *as* the trigger, rather than the
              trigger rendering as the button. Under React 18 a function
              component cannot be handed a ref, so the outer form leaves Base UI
              without an anchor to measure and the menu opens invisible at the
              top-left corner of the window. This way the DOM element is the
              trigger's own, and the sidebar's classes are merged onto it. */}
          <SidebarMenuButton
            size="lg"
            className="data-[popup-open]:bg-sidebar-accent data-[popup-open]:text-sidebar-accent-foreground"
            render={<DropdownMenuTrigger />}
            {...triggerData}
          >
            {nameplate}
            <ChevronsUpDown className="ml-auto size-4 shrink-0" />
          </SidebarMenuButton>
          {renderMenu(isMobile ? "bottom" : "right")}
        </DropdownMenu>
      </SidebarMenuItem>
    </SidebarMenu>
  );
}
