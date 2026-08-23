// What the console knows about the hosts it is connected to.
//
// The console used to resolve exactly one host at startup and never reconsider.
// A desktop client holds several at once, so "which host" becomes a dimension
// of almost everything: which client issues a request, which event stream a
// frame arrived on, which browser-local state a view reads.
//
// The failure mode this file exists to prevent is the one block/buzz shipped:
// a single-valued `activeConnection` in app state, plus local storage keyed by
// *company* rather than by connection. Two connections that both host a company
// called `acme` then silently share their tour progress, their last-read
// channel, and their draft mail settings. Nothing errors; the state is just
// quietly wrong.

import type { ConsoleConfig } from "@/config";

/**
 * A connection's identity within this client.
 *
 * **Minted locally and never derived from the URL.** A host moves between
 * `localhost:8080`, a LAN address and a tunnel over one afternoon, and two
 * different hosts can occupy one URL over time. Deriving the id from the URL
 * would lose a connection's history the moment it moved, and would merge two
 * hosts that happened to share an address.
 *
 * Distinct from the server's own `instance_id` (see `/spec`), which identifies
 * the *host*. Two connection entries may legitimately point at one instance —
 * the same server reached with a personal session and with a device token, say
 * — so the client's key cannot be the server's.
 */
export type ConnectionId = string;

/** How this client proves who it is to one host. */
export type Credential =
  /** A browser session cookie. Same-origin only; the web build's normal case. */
  | { kind: "cookie" }
  /**
   * A paired device session, presented as a header.
   *
   * `ref` is a handle into the OS keychain, **never the secret**. Connection
   * records are persisted and passed around the UI; a token in one would end up
   * in local storage and in every React devtools inspection.
   */
  | { kind: "device"; ref: string }
  /**
   * A platform bearer, from `?token=`. Machine credentials, not a person.
   *
   * `token` is optional in a *persisted* profile: `saveProfile` redacts it, so
   * `localStorage` never holds the secret. A token-less `{ kind: "platform" }`
   * is a marker meaning "this host authenticates as the platform" — the live
   * token is re-derived from the URL on the next load.
   */
  | { kind: "platform"; token?: string }
  /**
   * A personal session this client carries itself, in `x-opencompany-session`.
   *
   * The credential a **hub console** uses: one deployment of this app serving
   * many hosts on *other* origins. A cookie cannot work there — the host sets
   * it `SameSite=Lax`, so the browser withholds it from every cross-site
   * request the console makes, and `SameSite=None` would only turn it into a
   * third-party cookie Safari discards outright. The host mints this form on
   * request instead; see `SESSION_CARRIER_HEADER` in its `users/cookie.rs`.
   *
   * `value` is the whole ready-made header value, `<company>.<token>`, exactly
   * as the host returned it. Kept assembled rather than split because the
   * company half may have been resolved through the single-company alias, where
   * this client never learned an id to rebuild it from.
   *
   * **Unlike every other credential here, this one is a secret that lives in
   * the page.** It is persisted, because the alternative is signing in again on
   * every reload, and it is therefore readable by anything achieving script
   * execution on the console's origin. That is a real reduction from the
   * `HttpOnly` cookie a same-origin console gets, and it is the reason this
   * kind is never chosen for a connection that could have used a cookie.
   */
  | { kind: "session"; value: string };

/**
 * Where a connection stands right now.
 *
 * Per connection, deliberately: the whole point of holding several is that one
 * being unreachable is not the app being broken. A global status would collapse
 * that back into the single-connection world.
 */
export type ConnectionStatus =
  | "connecting"
  | "live"
  /** Reachable, but something it advertised is not answering. */
  | "degraded"
  | "down"
  /** Reached, but this client's credential was refused. */
  | "unauthenticated";

/** What a host said about itself at `/spec`. */
export interface InstanceIdentity {
  /** The host's own stable id. Absent on a host predating it. */
  instanceId?: string;
  displayName?: string;
  /**
   * Flat feature names. **Absent means "assume REST only"** — an older host
   * omits the field entirely, so `undefined` and `[]` must not be conflated.
   */
  capabilities?: string[];
  storage?: string;
}

/**
 * Where a connection came from, when that is not "someone typed an address".
 *
 * Superseded by {@link Connector}, and kept only as the shape older profiles
 * were written in — `origin: "embedded"` is what a build predating connectors
 * left in `localStorage`, and `connectorOf` is what reads it forward.
 *
 * @deprecated Read `Connection.connector`.
 */
export type ConnectionOrigin = "embedded";

/**
 * Where a runtime runs, and how this client gets to it.
 *
 * The four answers an operator can give to "where does my company live", and
 * the reason this is a durable property of a connection rather than something
 * derived from its address when it is needed:
 *
 * - a `local` host binds an **ephemeral port on purpose** — a fixed one
 *   collides with a dev server — so its address is different on every launch;
 * - an `ssh` connection's address is a loopback port this client chose when it
 *   opened the tunnel, so it is different on every launch too, and is nobody's
 *   address once the tunnel closes;
 * - `cloud` and `remote` are both `https://…` and are told apart by nothing
 *   whatsoever in the url.
 *
 * Recognising last launch's row as *this* host is what stops a dead one
 * accumulating per run (issue #615), and only a persisted marker can do it.
 *
 * See `docs/spec/runtime/connectors.md`.
 */
export type Connector =
  /**
   * A host running inside this application, over a data root it owns.
   *
   * Desktop only, and the one connector where nothing has to be typed: the
   * core starts the host and reports its address over IPC.
   */
  | { kind: "local" }
  /**
   * A tenant of the hosted control plane.
   *
   * Distinguished from {@link Connector} `remote` by behaviour rather than by
   * address. A tenant container is woken on request by the manager's proxy, so
   * its first request after an idle period takes seconds — see
   * `wakingWindow` in `registry.ts`, which is why a cold tenant reads as
   * "Waking…" instead of as a host that is gone.
   */
  | { kind: "cloud"; tenant: string }
  /** A host someone else's process is serving, addressed directly. */
  | { kind: "remote" }
  /**
   * A host bound to loopback on another machine, reached through a tunnel this
   * application opens and supervises.
   *
   * Desktop only: a browser cannot start a process.
   */
  | { kind: "ssh"; target: SshTarget };

export type ConnectorKind = Connector["kind"];

/** Where the far end of an SSH tunnel is, and how to reach it. */
export interface SshTarget {
  /**
   * `user@host`, or a `Host` alias out of `~/.ssh/config`.
   *
   * An alias is the form to prefer. Anyone with a bastion, a jump host or a
   * non-default key has already written that file, and re-asking for its
   * contents in a dialog is asking to be told them wrong.
   */
  destination: string;
  /** The SSH port, when it is not 22. */
  port?: number;
  /** Where the host is listening on the far side. */
  remotePort: number;
  /**
   * A keychain handle for a key passphrase or password — **never the secret**.
   *
   * The same rule as `{ kind: "device"; ref }` above, for the same reason:
   * connection records are persisted and passed around the UI.
   */
  secretRef?: string;
}

/**
 * Where a host listens on the far side of a tunnel, when nobody says otherwise.
 *
 * The port `opencompany serve` binds. Kept in step with `DEFAULT_REMOTE_PORT`
 * in the shell's `ssh.rs`, which is what applies it when the console omits it.
 */
export const DEFAULT_REMOTE_PORT = 8080;

/** The connector a host typed into "Add a host" gets. */
export const DEFAULT_CONNECTOR: Connector = { kind: "remote" };

/**
 * Which connectors this runtime can offer.
 *
 * `local` and `ssh` both need a process started on this machine, which only
 * the desktop shell can do. Offering them in a browser would put a tab on
 * screen that cannot be honoured — and the browser build has no core to
 * report the failure through, so it would simply do nothing.
 */
export function availableConnectors(desktop: boolean): ConnectorKind[] {
  return desktop ? ["local", "cloud", "remote", "ssh"] : ["cloud", "remote"];
}

/**
 * What each connector is called wherever one has to be named to an operator.
 *
 * Beside {@link ConnectorKind} rather than in either surface that prints it:
 * "Add a host" names the connector an operator is choosing between, and
 * "Manage hosts" names the one a row already has. Two copies would drift, and
 * a host would be offered under one word and then listed under another.
 */
export const CONNECTOR_LABELS: Record<ConnectorKind, string> = {
  local: "On this computer",
  cloud: "TinyHumans Cloud",
  remote: "Another gateway",
  ssh: "Over SSH",
};

export interface Connection {
  id: ConnectionId;
  /**
   * The company this connection's client addresses when a caller names none.
   *
   * `null` selects the host's single-company aliases. Carried on the connection
   * because it is a property of how this client was configured for this host —
   * `?company=` names one, a prosumer host has none — not of the host itself.
   */
  defaultCompany: string | null;
  /** What to call this connection in the UI, before `/spec` answers. */
  label: string;
  /** Empty string means same-origin. */
  baseUrl: string;
  credential: Credential;
  status: ConnectionStatus;
  /** `null` until `/spec` answers, and on a host that has no identity surface. */
  identity: InstanceIdentity | null;
  /** Companies this connection serves, once discovered. */
  companies: string[];
  /** Why it is `down` or `unauthenticated`, for the connection's own row. */
  error?: string;
  /** Where this host runs. See {@link Connector}. */
  connector: Connector;
  /**
   * Whether `connecting` means "waking a hibernating tenant" rather than
   * "contacting a host".
   *
   * Session state, never persisted: it says what this probe is doing right
   * now. Only a `cloud` connection is ever `true`.
   */
  waking?: boolean;
}

/** The fields needed to construct a connection's client. */
export function connectionConfig(
  connection: Connection,
): Pick<ConsoleConfig, "baseUrl" | "company" | "operatorToken" | "sessionHeader"> {
  return {
    baseUrl: connection.baseUrl,
    company: connection.defaultCompany,
    operatorToken:
      connection.credential.kind === "platform"
        ? (connection.credential.token ?? null)
        : null,
    sessionHeader:
      connection.credential.kind === "session" ? connection.credential.value : null,
  };
}

/**
 * Identifies whose browser-local state a key belongs to.
 *
 * Both halves are required. Company alone is what buzz keyed on, and it is
 * wrong as soon as two connections serve a company of the same name; connection
 * alone is wrong as soon as one connection serves two companies.
 */
export interface LocalScope {
  connection: ConnectionId;
  /** `null` for a single-company host, which has no id to name. */
  company: string | null;
}

/**
 * A `localStorage` key for one connection's view of one company.
 *
 * Every browser-local key in the console goes through here, so there is one
 * place that decides what "whose state is this" means. `::` separates the two
 * halves because a connection id is generated from a fixed alphabet that
 * excludes it, so the split is unambiguous.
 */
export function scopedKey(prefix: string, scope: LocalScope): string {
  return `${prefix}:${scope.connection}::${scope.company ?? "single"}`;
}

/**
 * The scoped key, having first adopted whatever the pre-connection console left
 * under `legacyName`.
 *
 * Every one of these keys existed before connections did, keyed on company
 * alone. Simply moving to a scoped key would have silently discarded all of it:
 * an operator's tour progress and last-read channel would reset on upgrade, and
 * — worse — `readLegacyLocalNodes` would stop finding the retired workspace
 * scratchpad it exists to migrate, so notes someone typed would become
 * unreachable with nothing reporting it.
 *
 * Adoption is a copy, not a move, and it happens per connection. Old state
 * cannot be attributed to a host that did not exist when it was written, so
 * every connection that looks inherits it once and they diverge from there.
 * That is the honest reading of "this is what you had before".
 *
 * The legacy key is never written again, so a second run is a no-op.
 */
export function scopedKeyAdoptingLegacy(
  prefix: string,
  scope: LocalScope,
  legacyName: string,
): string {
  const key = scopedKey(prefix, scope);
  try {
    const store = window.localStorage;
    if (store.getItem(key) === null) {
      const inherited = store.getItem(legacyName);
      if (inherited !== null) store.setItem(key, inherited);
    }
  } catch {
    // Private mode or a blocked store. The caller degrades to no memory, which
    // is what it did before this key existed.
  }
  return key;
}
