// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  addConnection,
  clientFor,
  editConnection,
  getConnection,
  hostAddressEditable,
  probe,
  resetConnections,
} from "@/connections/registry";
import type { Transport, TransportRequest, TransportResponse } from "@/api/transport";
import { findProfile, readProfiles } from "@/connections/profileStore";
import {
  addressLabel,
  canonicalAddress,
  hostEditable,
  validAddress,
} from "@/components/manage-hosts";
import { scopedKey } from "@/connections/types";
import type { Connection } from "@/connections/types";

/**
 * Modifying a host, which the switcher gained alongside adding one.
 *
 * The property under test is the same one the whole registry is built around:
 * a connection id is the namespace every browser-local key hangs off, so
 * "this host moved" must not be expressible as "forget it and add it again".
 */

beforeEach(() => {
  resetConnections();
  window.localStorage.clear();
});

function host(overrides: Partial<Connection> = {}): Connection {
  return {
    id: "c1",
    defaultCompany: null,
    label: "Acme",
    baseUrl: "https://acme.test",
    credential: { kind: "cookie" },
    status: "live",
    identity: null,
    companies: [],
    connector: { kind: "remote" },
    ...overrides,
  };
}

describe("editConnection", () => {
  it("renames a host without disturbing anything scoped to it", () => {
    const id = addConnection({ baseUrl: "https://acme.test" });
    const before = scopedKey("oc-tour", { connection: id, company: "acme" });

    editConnection(id, { label: "Acme production" });

    expect(getConnection(id)?.label).toBe("Acme production");
    expect(scopedKey("oc-tour", { connection: id, company: "acme" })).toBe(before);
    // And it survives the reload, or the rename lasts until the next one.
    expect(findProfile("https://acme.test", null)?.label).toBe("Acme production");
  });

  it("keeps the id when a host moves, which is the whole point", () => {
    // THE regression. Re-addressing by forgetting and re-adding mints a fresh
    // id, and every scoped key under the old one is orphaned silently.
    const id = addConnection({ baseUrl: "https://acme.test" });

    editConnection(id, { baseUrl: "https://acme.example.com" });

    expect(getConnection(id)?.baseUrl).toBe("https://acme.example.com");
    expect(readProfiles()).toHaveLength(1);
    expect(findProfile("https://acme.example.com", null)?.id).toBe(id);
  });

  it("rebuilds the client, so requests go to the new address", () => {
    // A patched connection would render the new address beside a client still
    // addressing the old one — a console that lies about where it is looking.
    const id = addConnection({ baseUrl: "https://acme.test" });
    editConnection(id, { baseUrl: "https://acme.example.com" });
    expect(clientFor(id)?.baseUrl).toBe("https://acme.example.com");
  });

  it("drops what the last probe concluded about the old address", () => {
    const id = addConnection({ baseUrl: "https://acme.test" });
    editConnection(id, { baseUrl: "https://acme.example.com" });

    const moved = getConnection(id);
    expect(moved?.status).toBe("connecting");
    expect(moved?.identity).toBeNull();
    expect(moved?.companies).toEqual([]);
    expect(moved?.error).toBeUndefined();
  });

  it("normalises a trailing slash, so a save is not a move", () => {
    const id = addConnection({ baseUrl: "https://acme.test" });
    editConnection(id, { baseUrl: "https://acme.test/" });
    expect(getConnection(id)?.baseUrl).toBe("https://acme.test");
  });

  it("keeps the old name when the field is blank", () => {
    const id = addConnection({ baseUrl: "https://acme.test", label: "Acme" });
    editConnection(id, { label: "   " });
    expect(getConnection(id)?.label).toBe("Acme");
  });

  it("does nothing for a host that is already gone", () => {
    expect(() => editConnection("no-such-host", { label: "x" })).not.toThrow();
  });
});

const OLD = "https://old.acme.test";
const NEW = "https://new.acme.test";

/**
 * A transport that can hold the old address's `/spec` open indefinitely.
 *
 * The bug this fixture exists for needs the two probes to *overlap*, which no
 * amount of awaiting produces on its own: the old probe has to still be in
 * flight at the moment the connection is re-addressed. Only `/spec` is gated,
 * not every request — the old probe has to be able to run to completion once
 * released, since what is under test is that its **writes** are discarded
 * rather than that it is somehow prevented from finishing.
 */
class GatedTransport implements Transport {
  readonly urls: string[] = [];
  private waiting: (() => void)[] = [];

  async request(req: TransportRequest): Promise<TransportResponse> {
    this.urls.push(req.url);
    if (req.url.startsWith(OLD) && req.url.endsWith("/spec")) {
      await new Promise<void>((resolve) => this.waiting.push(resolve));
    }
    const old = req.url.startsWith(OLD);
    const body = req.url.endsWith("/spec")
      ? JSON.stringify({ display_name: old ? "Old host" : "New host" })
      : JSON.stringify([{ id: old ? "old-co" : "new-co" }]);
    return {
      status: 200,
      statusText: "OK",
      url: req.url,
      text: body,
      header: () => null,
    };
  }

  /** Lets every held request answer, as a slow host eventually does. */
  release(): void {
    for (const resume of this.waiting.splice(0)) resume();
  }

  subscribe(): () => void {
    throw new Error("no streaming");
  }
}

/**
 * A probe that was already running when its host moved.
 *
 * Two failures in one, and both are silent. The registry suppresses a second
 * probe for an id while one is in flight — without which a re-render storm
 * probes forever — so `editConnection`'s own `void probe(id)` did nothing, and
 * the new address was never contacted at all. Meanwhile the old probe still
 * held the *old* client, so when it finally answered it patched that host's
 * identity, company list and status onto the row now pointing somewhere else:
 * a console naming one host while addressing another.
 */
describe("a host that moves while a probe is in flight", () => {
  it("ignores the old probe's answers and probes the address it moved to", async () => {
    const transport = new GatedTransport();
    const id = addConnection({ baseUrl: OLD, transport });

    const inFlight = probe(id);
    // Not `await Promise.resolve()`: the probe reaches the transport several
    // microtasks in, and the move has to land while it is genuinely held.
    await vi.waitFor(() => expect(transport.urls.length).toBeGreaterThan(0));

    editConnection(id, { baseUrl: NEW });
    transport.release();
    await inFlight;

    // The replacement probe is started by the old one's finalizer, so it is
    // running unawaited by the time `inFlight` settles.
    await vi.waitFor(() => expect(getConnection(id)?.status).toBe("live"));

    const moved = getConnection(id);
    // The new address was contacted at all, which is the half that used to be
    // skipped outright.
    expect(transport.urls.some((url) => url.startsWith(NEW))).toBe(true);
    // And nothing the old address said survived onto the row.
    expect(moved?.identity?.displayName).toBe("New host");
    expect(moved?.label).toBe("New host");
    expect(moved?.companies).toEqual(["new-co"]);
  });
});

describe("hostAddressEditable", () => {
  it("lets an operator retype an address they typed in the first place", () => {
    expect(hostAddressEditable({ kind: "remote" })).toBe(true);
    expect(hostAddressEditable({ kind: "cloud", tenant: "acme" })).toBe(true);
  });

  it("withholds the ones this application assigns itself", () => {
    // Both bind a port that is different on every launch, so an address saved
    // here would be overwritten by the next one — and point at nothing until
    // it was.
    expect(hostAddressEditable({ kind: "local" })).toBe(false);
    expect(
      hostAddressEditable({ kind: "ssh", target: { destination: "vps", remotePort: 8080 } }),
    ).toBe(false);
  });

  it("refuses to move a local host even when asked directly", () => {
    const id = addConnection({ baseUrl: "http://127.0.0.1:9331", connector: { kind: "local" } });
    editConnection(id, { baseUrl: "https://elsewhere.test" });
    expect(getConnection(id)?.baseUrl).toBe("http://127.0.0.1:9331");
  });
});

describe("the manage page's own reading of a row", () => {
  it("offers no edit on a host whose name this client does not own", () => {
    // `adoptLocalHosts` re-applies the roster's name on every refresh, so an
    // edit offered here would be reverted by the next poll.
    expect(hostEditable(host({ connector: { kind: "local" } }))).toBe(false);
    expect(hostEditable(host())).toBe(true);
  });

  it("says something about a same-origin row rather than nothing", () => {
    expect(addressLabel(host({ baseUrl: "" }))).toBe("This origin");
    expect(addressLabel(host())).toBe("https://acme.test");
  });

  it("insists on a scheme, which is what decides where a request goes", () => {
    expect(validAddress("https://acme.test")).toBe(true);
    expect(validAddress(" http://10.0.0.4:8080 ")).toBe(true);
    // Resolved against the console's own origin, so the row would fail with an
    // address nobody typed.
    expect(validAddress("acme.test")).toBe(false);
    expect(validAddress("")).toBe(false);
    // Not a transport this console speaks.
    expect(validAddress("ftp://acme.test")).toBe(false);
  });
});

describe("canonicalAddress", () => {
  // Whether two rows are the same host. Getting it wrong mints a second
  // connection id for one host, and every browser-local key is scoped by id.

  it("reads the same-origin row as the origin it actually is", () => {
    // The bootstrap row stores `""`, so the raw comparison called this origin's
    // own url a different host and offered to add it a second time.
    expect(canonicalAddress("")).toBe(canonicalAddress(window.location.origin));
    expect(canonicalAddress("   ")).toBe(canonicalAddress(window.location.origin));
  });

  it("ignores the spellings a host does not distinguish", () => {
    const canonical = canonicalAddress("https://acme.test");
    expect(canonicalAddress("https://acme.test/")).toBe(canonical);
    expect(canonicalAddress("https://ACME.test")).toBe(canonical);
    // The scheme's own default port is not part of the address.
    expect(canonicalAddress("https://acme.test:443")).toBe(canonical);
    expect(canonicalAddress("  https://acme.test  ")).toBe(canonical);
    expect(canonicalAddress("http://acme.test:80")).toBe(canonicalAddress("http://acme.test"));
  });

  it("keeps the differences that are real ones", () => {
    // A different scheme, a non-default port and a path prefix each name a
    // different place to send a request.
    expect(canonicalAddress("http://acme.test")).not.toBe(canonicalAddress("https://acme.test"));
    expect(canonicalAddress("https://acme.test:8443")).not.toBe(
      canonicalAddress("https://acme.test"),
    );
    expect(canonicalAddress("https://acme.test/oc")).not.toBe(canonicalAddress("https://acme.test"));
    // …but a bare authority's own trailing slash is not one of them.
    expect(canonicalAddress("https://acme.test/oc/")).toBe(canonicalAddress("https://acme.test/oc"));
  });

  it("hands back something comparable for a value that will not parse", () => {
    // `validAddress` is what refuses these, in the field that names them.
    expect(canonicalAddress(" acme.test// ")).toBe("acme.test");
  });
});
