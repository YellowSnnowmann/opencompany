// @vitest-environment jsdom
//
// The connection scope on the hash (issue #1358).
//
// What the reported bug actually was: selecting a host wrote no history entry
// and the hash named a page without naming whose. On the "Can't connect" screen
// of an unreachable host, Back therefore looked completely inert — while
// popping entries belonging to the host still working, so switching back landed
// on Overview rather than the Tasks board it had been rewound from.
//
// The three properties these pin are the three halves of that, and each one
// fails on its own:
//
//   - a switch is a push, so Back undoes it;
//   - the scope survives an ordinary view navigation, so the entries Back
//     returns through keep naming their host;
//   - the address is repaired towards the host on screen, never away from a
//     host the address already names — the second is what would undo a Back.

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useHashView } from "@/hooks/use-hash-view";
import {
  hashWithHost,
  readHostParam,
  useHostAddress,
  useHostRoute,
  withHostParam,
  type HostRoute,
} from "@/hooks/use-host-route";

let container: HTMLDivElement;
let root: Root;

/** Puts the address at `hash` without recording a history entry. */
function at(hash: string) {
  window.history.replaceState(null, "", hash);
}

/** Dispatches what the browser dispatches after Back moves the hash. */
async function back(hash: string) {
  await act(async () => {
    window.history.replaceState(null, "", hash);
    window.dispatchEvent(new HashChangeEvent("hashchange"));
  });
}

beforeEach(() => {
  (globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  at("#/overview");
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  at("#/overview");
});

describe("reading and writing the scope", () => {
  it("reads the host the address names, and null when it names none", () => {
    at("#/ledgers/tasks");
    expect(readHostParam()).toBeNull();
    at("#/ledgers/tasks?host=conn-a");
    expect(readHostParam()).toBe("conn-a");
  });

  it("coexists with a value-less flag, and leaves it bare", () => {
    // `useHashFlag` writes `?new`, not `?new=`. Round-tripping the query to
    // change the host must not rewrite the flag standing next to it.
    at("#/ledgers/goals?new");
    expect(readHostParam()).toBeNull();
    expect(hashWithHost("conn-a")).toBe("#/ledgers/goals?new&host=conn-a");
  });

  it("replaces an existing scope, and removes it when given null", () => {
    at("#/ledgers/tasks?host=conn-a");
    expect(hashWithHost("conn-b")).toBe("#/ledgers/tasks?host=conn-b");
    expect(hashWithHost(null)).toBe("#/ledgers/tasks");
  });

  it("gives a landing with no path a readable one rather than `#?host=`", () => {
    at("#");
    expect(hashWithHost("conn-a")).toBe("#/?host=conn-a");
  });

  it("carries the scope onto a new path and drops every other key", () => {
    // The rule the two differ by: the host is a scope and survives a view
    // change; `?new` belongs to the screen it was opened over and does not.
    at("#/ledgers/goals?new&host=conn-a");
    expect(withHostParam("tasks")).toBe("#/tasks?host=conn-a");
    at("#/ledgers/goals?new");
    expect(withHostParam("tasks")).toBe("#/tasks");
  });
});

describe("useHostRoute", () => {
  let route: HostRoute | null = null;

  function Probe({ fallback }: { fallback: string | null }) {
    route = useHostRoute(fallback);
    return null;
  }

  async function mount(fallback: string | null) {
    await act(async () => {
      root.render(createElement(Probe, { fallback }));
    });
  }

  it("opens the host the address names, over the fallback", async () => {
    at("#/ledgers/tasks?host=conn-b");
    await mount("conn-a");
    expect(route?.selected).toBe("conn-b");
  });

  it("falls back when the address names no host", async () => {
    at("#/ledgers/tasks");
    await mount("conn-a");
    expect(route?.selected).toBe("conn-a");
  });

  it("pushes a scoped entry when a host is selected", async () => {
    at("#/ledgers/tasks?host=conn-a");
    await mount("conn-a");
    await act(async () => {
      route?.selectHost("conn-b", "conn-a");
    });
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-b");
    expect(route?.selected).toBe("conn-b");
  });

  it("stamps the entry being left when it carries no scope yet", async () => {
    // The console writes no scope while it holds one host, so the entry a first
    // switch leaves behind names nobody — and Back from the host just opened
    // would be inert, which is the gesture the issue was reported from.
    at("#/ledgers/tasks");
    await mount("conn-a");
    await act(async () => {
      route?.selectHost("conn-b", "conn-a");
    });
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-b");
    // `replaceState` rewrote the entry left behind before the push, so Back
    // now returns to a `#/ledgers/tasks?host=conn-a`.
    await back("#/ledgers/tasks?host=conn-a");
    expect(route?.selected).toBe("conn-a");
  });

  it("follows Back onto an entry that names another host", async () => {
    at("#/ledgers/tasks?host=conn-b");
    await mount("conn-a");
    expect(route?.selected).toBe("conn-b");
    await back("#/ledgers/tasks?host=conn-a");
    expect(route?.selected).toBe("conn-a");
  });

  it("leaves the selection alone on an address that names no host", async () => {
    // Every ordinary view navigation looks like this for the moment before the
    // scope is stamped back on, and every address in a one-host console looks
    // like it permanently. Reading it as "no host chosen" would bounce the
    // console to the bootstrap connection on each click.
    at("#/ledgers/tasks?host=conn-b");
    await mount("conn-a");
    await back("#/tasks");
    expect(route?.selected).toBe("conn-b");
  });

  it("resettles onto another host without recording an entry", async () => {
    at("#/ledgers/tasks?host=conn-b");
    await mount("conn-a");
    await act(async () => {
      route?.resettleHost("conn-a");
    });
    expect(route?.selected).toBe("conn-a");
    // Forgetting a host is not somewhere Back should return to, so the address
    // is left for `useHostAddress` to correct rather than pushed.
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-b");
  });
});

describe("useHostAddress", () => {
  function Probe({
    activeId,
    ids,
    scoped,
  }: {
    activeId: string | null;
    ids: string;
    scoped: boolean;
  }) {
    useHostAddress(activeId, ids, scoped);
    return null;
  }

  async function mount(props: { activeId: string | null; ids: string; scoped: boolean }) {
    await act(async () => {
      root.render(createElement(Probe, props));
    });
  }

  it("names the host on screen once there is a choice to disambiguate", async () => {
    at("#/ledgers/tasks");
    await mount({ activeId: "conn-a", ids: "conn-a,conn-b", scoped: true });
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-a");
  });

  it("writes nothing while the console holds one host", async () => {
    at("#/ledgers/tasks");
    await mount({ activeId: "conn-a", ids: "conn-a", scoped: false });
    expect(window.location.hash).toBe("#/ledgers/tasks");
  });

  it("never overwrites a scope that names a live host", async () => {
    // THE assertion. This runs on every `hashchange`, so a Back that restored
    // `?host=conn-b` would be undone one tick later if this repaired towards
    // the host still rendered — which is the render `active` is one behind on.
    at("#/ledgers/tasks?host=conn-b");
    await mount({ activeId: "conn-a", ids: "conn-a,conn-b", scoped: true });
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-b");
  });

  it("replaces a scope naming a host this client no longer holds", async () => {
    at("#/ledgers/tasks?host=conn-gone");
    await mount({ activeId: "conn-a", ids: "conn-a,conn-b", scoped: true });
    expect(window.location.hash).toBe("#/ledgers/tasks?host=conn-a");
  });

  it("repairs a scope dropped by a bare hash assignment", async () => {
    at("#/ledgers/tasks");
    await mount({ activeId: "conn-a", ids: "conn-a,conn-b", scoped: true });
    // A view writing `location.hash` directly (`TaskDetailRoute`, `ChatView`)
    // drops the query. The event it fires is what puts the scope back.
    await back("#/tasks/abc");
    expect(window.location.hash).toBe("#/tasks/abc?host=conn-a");
  });
});

describe("useHashView carries the scope across a view change", () => {
  let navigate: ((view: string, sub?: string) => void) | null = null;
  const VIEWS = ["overview", "ledgers", "tasks"] as const;

  function Probe() {
    const [, , go] = useHashView<(typeof VIEWS)[number]>(VIEWS, "overview");
    navigate = go as (view: string, sub?: string) => void;
    return null;
  }

  it("keeps the host when navigating to another view", async () => {
    at("#/ledgers/tasks?host=conn-b");
    await act(async () => {
      root.render(createElement(Probe));
    });
    await act(async () => {
      navigate?.("overview");
    });
    expect(window.location.hash).toBe("#/overview?host=conn-b");
  });

  it("keeps the host when canonicalizing an address the shell cannot render", async () => {
    // An unknown head resolves to the fallback and the URL is rewritten to say
    // so — with `replaceState`, which fires no event, so the scope has to ride
    // along here or it is gone with nothing to notice.
    at("#/finances?host=conn-b");
    await act(async () => {
      root.render(createElement(Probe));
    });
    expect(window.location.hash).toBe("#/overview?host=conn-b");
  });
});
