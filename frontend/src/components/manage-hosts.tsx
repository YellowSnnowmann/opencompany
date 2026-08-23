// The page behind the switcher's "Manage hosts", where a connection can be
// renamed, re-addressed or forgotten.
//
// ## Why this is not in the menu
//
// The switcher's rows are a **filter**: clicking one puts that host's console
// on screen and changes nothing else. Hanging a rename and a delete off the
// same row would make a control whose click targets disagree about what a row
// is for — and a destructive one a keyboard user lands on while switching
// hosts. Adding a host already opens its own surface; modifying one gets the
// same treatment.
//
// ## Why editing exists at all
//
// Until now the only way to fix a host was to forget it and add it again. That
// mints a **new connection id** — and every browser-local key in the console is
// scoped by it (see `scopedKey` in `connections/types.ts`), so re-adding a host
// whose address changed silently resets its tour progress, its last-read
// channel and its drafts. An edit keeps the id, which is the whole point.
//
// ## What is deliberately read-only here
//
// A `local` host's name and address both belong to the instance roster the
// desktop core keeps: `adoptLocalHosts` re-applies them on every refresh, so an
// edit made here would be reverted by the next one, and forgetting the row
// drops the profile only for the instance to come back under a fresh id on the
// next poll. Those hosts are managed where they are started — the "On this
// computer" tab of "Add a host" — and this page says so rather than offering
// buttons that undo themselves.

import { ArrowLeft, Loader2, Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { statusCopy } from "@/components/host-switcher";
import { useHosts } from "@/connections/HostsContext";
import { hostAddressEditable } from "@/connections/registry";
import type { Connection } from "@/connections/types";
import { CONNECTOR_LABELS } from "@/connections/types";
import { cn } from "@/lib/utils";

/**
 * Whether this client owns the row, or the thing that started the host does.
 *
 * See the note at the top: a `local` host's name and address are re-applied
 * from the instance roster on every refresh.
 */
export function hostEditable(connection: Connection): boolean {
  return connection.connector.kind !== "local";
}

/** How a connection's address reads when it has one to show. */
export function addressLabel(connection: Connection): string {
  if (connection.baseUrl) return connection.baseUrl;
  // The web console's bootstrap row: same-origin, which is stored as the empty
  // string. Printing nothing there reads as a broken row.
  return "This origin";
}

/**
 * One address written the one way, so two spellings of one host compare equal.
 *
 * Duplicate detection is the whole reason this exists, and a raw string
 * comparison gets it wrong in four ways that all produce the same wrong
 * outcome — two rows for one host, one console offered twice under two
 * connection ids, and every browser-local key split across them:
 *
 * - **the same-origin bootstrap row stores `""`** (see {@link addressLabel}),
 *   so typing that origin's explicit url reads as a different host;
 * - **a trailing slash**, of which only one was trimmed, so `…//` slipped past;
 * - **hostname case**, which DNS does not distinguish and `URL` normalises;
 * - **an explicit default port**, so `https://acme.example:443` and
 *   `https://acme.example` read as two hosts.
 *
 * The path is kept — a host served under a prefix is a different host — but
 * its trailing slash is not, because `URL` mints one for a bare authority.
 * A value that will not parse is returned trimmed and de-slashed rather than
 * thrown away: {@link validAddress} is what refuses it, and refusing it there
 * gives the operator the error that names the field.
 */
export function canonicalAddress(value: string): string {
  const trimmed = value.trim();
  // Same-origin, which is what this console is already talking to.
  if (!trimmed) return canonicalAddress(window.location.origin);
  try {
    const url = new URL(trimmed);
    const path = url.pathname.replace(/\/$/, "");
    // `URL` already lowercases the protocol and the hostname and drops the
    // port when it is that protocol's default, so `origin` is the normalised
    // authority — no separate default-port table to keep in step.
    return `${url.origin}${path}`;
  } catch {
    return trimmed.replace(/\/+$/, "");
  }
}

/**
 * Whether `value` is an address this console could actually talk to.
 *
 * Deliberately stricter than the field being non-empty: an address saved with
 * no scheme resolves against the console's own origin, so the row would go
 * `down` with an error naming a URL nobody typed.
 */
export function validAddress(value: string): boolean {
  try {
    const { protocol } = new URL(value.trim());
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

/**
 * The hosts this console knows, with what can be done to each.
 *
 * A full-screen surface over the console rather than a route: the console is
 * keyed by connection and remounts when selection changes, and this page's own
 * actions change selection — forgetting the host on screen moves it. Overlaying
 * keeps whatever is underneath alive rather than tearing it down and rebuilding
 * it when the page closes.
 *
 * Mounted by `App` beside `ConsoleOrAddHost`, for the reason given there.
 */
export function ManageHostsPage() {
  const hosts = useHosts();
  const { managingHosts: open, setManagingHosts, connections, selected } = hosts;

  // Escape leaves, like every other layer in the console. Registered on the
  // window rather than on a focus trap: the page is a plain region, and an
  // operator who opened it from a menu expects the same key to close it.
  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setManagingHosts(false);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, setManagingHosts]);

  if (!open) return null;

  return (
    <div
      data-testid="manage-hosts"
      className="fixed inset-0 z-50 overflow-y-auto bg-background"
      role="region"
      aria-label="Manage hosts"
    >
      <div className="mx-auto w-full max-w-3xl space-y-6 p-6">
        <div className="flex items-center gap-3">
          <Button
            data-testid="manage-hosts-back"
            variant="ghost"
            size="icon"
            aria-label="Back"
            onClick={() => setManagingHosts(false)}
          >
            <ArrowLeft className="size-4" />
          </Button>
          <div className="flex-1">
            <h1 className="text-lg font-semibold">Hosts</h1>
            <p className="text-muted-foreground text-sm">
              Every OpenCompany server this console is connected to. Renaming one or
              pointing it at a new address keeps its history here; forgetting one does
              not.
            </p>
          </div>
          <Button
            data-testid="manage-hosts-add"
            variant="outline"
            onClick={() => {
              setManagingHosts(false);
              hosts.setAddingHost(true);
            }}
          >
            <Plus className="size-4" />
            Add a host
          </Button>
        </div>

        {connections.length === 0 ? (
          <p data-testid="manage-hosts-empty" className="text-muted-foreground text-sm">
            No hosts yet.
          </p>
        ) : (
          <ul className="space-y-2">
            {connections.map((connection) => (
              <HostRow
                key={connection.id}
                connection={connection}
                current={connection.id === selected}
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

/** One host: what it is, and the two things that can be done to it. */
function HostRow({ connection, current }: { connection: Connection; current: boolean }) {
  const hosts = useHosts();
  const [editing, setEditing] = useState(false);
  const status = statusCopy(connection);

  return (
    <li
      data-testid={`manage-host-${connection.id}`}
      data-status={connection.status}
      data-current={current}
      className="rounded-lg border p-3"
    >
      <div className="flex items-start gap-3">
        <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", status.dot)} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm font-medium">{connection.label}</span>
            {current && (
              <span className="text-muted-foreground rounded border px-1.5 text-xs">
                On screen
              </span>
            )}
          </div>
          <div className="text-muted-foreground truncate text-xs">
            {addressLabel(connection)}
          </div>
          {/* In words, not in hue — the same rule the switcher's rows follow
              (issue #1167), and the error beside it because this is the page
              someone lands on to fix exactly that. */}
          <div className="text-muted-foreground mt-1 text-xs">
            {CONNECTOR_LABELS[connection.connector.kind]} · {status.label}
            {connection.error ? ` · ${connection.error}` : ""}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {!current && (
            <Button
              data-testid={`manage-host-open-${connection.id}`}
              variant="ghost"
              size="sm"
              onClick={() => {
                hosts.onSelect(connection.id);
                hosts.setManagingHosts(false);
              }}
            >
              Open
            </Button>
          )}
          {hostEditable(connection) && (
            <>
              <Button
                data-testid={`manage-host-edit-${connection.id}`}
                variant="ghost"
                size="sm"
                aria-label={`Edit ${connection.label}`}
                onClick={() => setEditing((was) => !was)}
              >
                <Pencil className="size-4" />
              </Button>
              <ForgetHost connection={connection} />
            </>
          )}
        </div>
      </div>

      {/* A host this client did not start is the only one it can speak for. */}
      {hostEditable(connection) ? null : (
        <p
          data-testid={`manage-host-managed-${connection.id}`}
          className="text-muted-foreground mt-2 text-xs"
        >
          Runs on this computer. Rename, start and stop it under “On this computer” in
          Add a host — a change made here would be undone the next time this
          application reads its own roster.
        </p>
      )}

      {editing && (
        <EditHost
          connection={connection}
          onDone={() => setEditing(false)}
        />
      )}
    </li>
  );
}

/** The rename-and-re-address form for one host. */
function EditHost({ connection, onDone }: { connection: Connection; onDone: () => void }) {
  const hosts = useHosts();
  const [label, setLabel] = useState(connection.label);
  const [baseUrl, setBaseUrl] = useState(connection.baseUrl);
  const [saving, setSaving] = useState(false);
  const addressable = hostAddressEditable(connection.connector);

  // Canonical on both sides (see `canonicalAddress`): retyping the row's own
  // address in a different but equivalent spelling is not a move, and must not
  // re-seat a live client to say nothing.
  const typed = canonicalAddress(baseUrl);
  const moved = addressable && typed !== canonicalAddress(connection.baseUrl);
  // A second row at an address another connection already holds is two rows for
  // one host: the same profile, one id each, and a switcher that offers the
  // same console twice.
  const taken =
    moved &&
    hosts.connections.some(
      (other) => other.id !== connection.id && canonicalAddress(other.baseUrl) === typed,
    );
  const badAddress = moved && !validAddress(baseUrl);
  const unchanged = !moved && label.trim() === connection.label;

  function save() {
    setSaving(true);
    hosts.onEditHost(connection.id, {
      label,
      ...(addressable ? { baseUrl } : {}),
    });
    setSaving(false);
    onDone();
  }

  return (
    <div
      data-testid={`manage-host-form-${connection.id}`}
      className="mt-3 space-y-3 border-t pt-3"
    >
      <div className="space-y-1.5">
        <Label htmlFor={`host-label-${connection.id}`}>Name</Label>
        <Input
          id={`host-label-${connection.id}`}
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
      </div>
      <div className="space-y-1.5">
        <Label htmlFor={`host-url-${connection.id}`}>Address</Label>
        <Input
          id={`host-url-${connection.id}`}
          value={baseUrl}
          disabled={!addressable}
          onChange={(e) => setBaseUrl(e.target.value)}
        />
        {addressable ? (
          <p className="text-muted-foreground text-xs">
            Changing this re-contacts the host at the new address, keeping everything
            this console remembers about it.
          </p>
        ) : (
          <p className="text-muted-foreground text-xs">
            This application picks the address itself — the tunnel binds a different
            local port on every launch — so only the name is yours to set.
          </p>
        )}
        {badAddress && (
          <p data-testid={`manage-host-bad-url-${connection.id}`} className="text-destructive text-xs">
            Needs a full address, starting with http:// or https://.
          </p>
        )}
        {taken && (
          <p data-testid={`manage-host-taken-${connection.id}`} className="text-destructive text-xs">
            Another host here already uses that address.
          </p>
        )}
      </div>
      <div className="flex justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onDone}>
          Cancel
        </Button>
        <Button
          data-testid={`manage-host-save-${connection.id}`}
          size="sm"
          disabled={saving || unchanged || badAddress || taken}
          onClick={save}
        >
          {saving ? <Loader2 className="size-4 animate-spin" /> : null}
          Save
        </Button>
      </div>
    </div>
  );
}

/**
 * Forgetting a host, behind a confirmation.
 *
 * Confirmed because it is not a tidy-up: the connection id goes with it, and
 * with the id every browser-local key scoped under it. The wording says what is
 * and is not touched, because "delete" beside a server name reads as something
 * far worse than it is.
 */
function ForgetHost({ connection }: { connection: Connection }) {
  const hosts = useHosts();
  return (
    <AlertDialog>
      <AlertDialogTrigger
        render={
          <Button
            data-testid={`manage-host-forget-${connection.id}`}
            variant="ghost"
            size="sm"
            aria-label={`Forget ${connection.label}`}
          >
            <Trash2 className="size-4" />
          </Button>
        }
      />
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Forget {connection.label}?</AlertDialogTitle>
          <AlertDialogDescription>
            This console stops talking to it and forgets what it remembered about it —
            which channel you had open, how far through the tour you were. The host
            itself, and everything on it, is untouched. You can add it again at any
            time.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            data-testid={`manage-host-forget-confirm-${connection.id}`}
            className="bg-destructive text-white hover:bg-destructive/90"
            onClick={() => hosts.onRemoveHost(connection.id)}
          >
            Forget it
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
