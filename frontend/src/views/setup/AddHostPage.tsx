// Where a host comes from: this computer, the hosted platform, or an address
// somewhere else.
//
// ## Why this is a screen and not a dialog (issue #1531)
//
// It was a `Dialog` for as long as it was an afterthought hanging off the host
// switcher's last menu item, and the dialog is `sm:max-w-sm` — 24rem, sized for
// a confirmation. Four connector tabs, a roster of local instances with Start
// and Stop buttons, and a two-line explanation under every field do not fit in
// that: the tab strip clipped its own labels, and the card scrolled its title
// off the top to make room for a form.
//
// The deeper reason is that adding a host is not a confirmation. It is the
// *first* step of onboarding — you cannot pick a company, sign in, or run setup
// until there is a host to do it on — so it belongs with the rest of that flow,
// drawn in the same `OnboardingShell` card as `SetupWizard`, with the same
// bounded width, the same fixed header, and the same scrolling middle. A person
// arriving at a hub with nothing connected now walks one path: add a host, then
// build a company.
//
// ## It is a screen, not a route
//
// Like `SetupWizard`, which is a phase of `ConnectionConsole` rather than a
// hash route. Nothing about adding a host is worth linking to or restoring on
// reload, and a route would have to encode which of N consoles it interrupted.
// `App` swaps it in over the console while `HostsContext.addingHost` is set —
// keeping the console *mounted* behind it, so cancelling does not tear down a
// live connection's streams and re-boot it.

import { useState } from "react";
import { ArrowLeft, Loader2, Play, Square, Trash2 } from "lucide-react";

import type { LocalInstance } from "@/api/transport/desktop";
import { OnboardingShell } from "@/components/onboarding-shell";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useHosts } from "@/connections/HostsContext";
import type { ConnectorKind, SshTarget } from "@/connections/types";
import {
  CONNECTOR_LABELS,
  DEFAULT_REMOTE_PORT,
  availableConnectors,
} from "@/connections/types";

/**
 * The chooser, and the form for whichever connector is chosen.
 *
 * **Rendered beside the console, not inside it.** Creating a host on this
 * computer selects it, which remounts the console — and the switcher with it —
 * so a screen owned by either would take itself off screen at the moment it
 * succeeded, with the operator still working in it. Its open flag lives in
 * `HostsContext` for the same reason.
 */
export function AddHostPage() {
  const hosts = useHosts();
  const {
    connections,
    setAddingHost,
    localInstances,
    onAddLocal,
    onAddSsh,
    onStartLocal,
    onStopLocal,
    onDeleteLocal,
  } = hosts;
  const close = () => setAddingHost(false);
  // The two connectors that need a process on this machine are offered only
  // where one can be started. `onAddLocal` rather than `isDesktopRuntime()`:
  // `App` withholds these handlers on a shell too old to honour them, and a tab
  // whose button does nothing is worse than a tab that is not there.
  const tabs = availableConnectors(Boolean(onAddLocal)).filter(
    (kind) => kind !== "ssh" || onAddSsh,
  );
  const onAdd = (baseUrl: string, kind: ConnectorKind) => {
    hosts.onAdd(baseUrl, kind === "cloud" ? { kind, tenant: tenantOf(baseUrl) } : { kind: "remote" });
    close();
  };

  // What leaving lands on. With a host connected it is the console; on a first
  // run — a fresh desktop whose embedded host did not start, or a hub nobody
  // has added anything to — it is the empty screen that sent them here. The way
  // out is offered either way: a screen you can enter and not leave is a trap,
  // and the dialog this replaced could always be dismissed.
  const connected = connections.length > 0;

  return (
    <OnboardingShell
      testId="add-host"
      header={
        <div className="space-y-1">
          <h1 className="text-xl font-semibold tracking-tight">Add a host</h1>
          <p className="text-sm text-muted-foreground">
            A host is one OpenCompany server. Run one on this computer, use one hosted for
            you, or point at one somewhere else — over ssh if it is not on the open
            internet. Whichever you choose stays connected alongside the others, and it
            decides where a <em>new</em> company lives rather than moving one you have.
          </p>
        </div>
      }
      footer={
        // The one way out, in the one place every connector shares. Each panel
        // below carries only its own primary action, so nothing has to decide
        // between two buttons that do the same thing.
        <Button variant="ghost" size="sm" data-testid="add-host-back" onClick={close}>
          <ArrowLeft className="size-4" />
          {connected ? "Back to the console" : "Back"}
        </Button>
      }
    >
      {/*
        The desktop leads with the local half, because starting a host is the
        thing it can do that a browser cannot — and because a person who has
        just installed the application has no URL to type. A browser leads
        with the cloud, which is the only one of the four it can offer that
        nobody has to run themselves.
      */}
      <Tabs defaultValue={tabs[0]}>
        <TabsList className="w-full">
          {tabs.map((kind) => (
            <TabsTrigger key={kind} value={kind} data-testid={`add-host-${kind}`}>
              {CONNECTOR_LABELS[kind]}
            </TabsTrigger>
          ))}
        </TabsList>
        {onAddLocal ? (
          <TabsContent value="local" className="mt-4">
            <LocalInstances
              instances={localInstances}
              onAdd={onAddLocal}
              onStart={onStartLocal}
              onStop={onStopLocal}
              onDelete={onDeleteLocal}
            />
          </TabsContent>
        ) : null}
        <TabsContent value="cloud" className="mt-4">
          <CloudHost onAdd={(baseUrl) => onAdd(baseUrl, "cloud")} />
        </TabsContent>
        <TabsContent value="remote" className="mt-4">
          <RemoteHost
            onAdd={(baseUrl) => onAdd(baseUrl, "remote")}
            desktop={Boolean(onAddLocal)}
          />
        </TabsContent>
        {onAddSsh ? (
          <TabsContent value="ssh" className="mt-4">
            <SshHost onAdd={onAddSsh} onDone={close} />
          </TabsContent>
        ) : null}
      </Tabs>
    </OnboardingShell>
  );
}

/**
 * A panel's own actions.
 *
 * Inside the scrolling band rather than in the card's footer, because each
 * connector's primary button belongs to that connector's form — it is disabled
 * by that form's field and spins on that form's request. Lifting four different
 * buttons into one shared footer would mean lifting four forms' state with
 * them, to render a row that sits directly under the last field anyway.
 */
function PanelActions({ children }: { children: React.ReactNode }) {
  return <div className="mt-5 flex items-center justify-end gap-2">{children}</div>;
}

/**
 * Which tenant a cloud address names.
 *
 * The first label of the host name, which is the subdomain the control plane
 * gives a tenant. Only ever a *name* — it decides nothing about where requests
 * go, which is `baseUrl`'s job — so a url shaped some other way degrading to
 * the whole authority costs nothing but a longer word in a row.
 */
function tenantOf(baseUrl: string): string {
  try {
    const { hostname } = new URL(baseUrl);
    return hostname.split(".")[0] || hostname;
  } catch {
    return baseUrl;
  }
}

/** A tenant of the hosted platform, at the address it was given. */
function CloudHost({ onAdd }: { onAdd: (baseUrl: string) => void }) {
  const [url, setUrl] = useState("");
  return (
    <>
      <div className="space-y-2">
        <Label htmlFor="cloud-url">Company address</Label>
        <Input
          id="cloud-url"
          placeholder="https://acme.opencompany.cloud"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
        {/*
          Worth saying before the first probe rather than after it: a tenant
          that has been idle is not running, and the platform starts it when a
          request arrives. The row says "Waking…" for as long as that takes,
          and an operator who has not been told that reads it as a hang.
        */}
        <p className="text-muted-foreground text-xs">
          A company hosted for you. If it has been idle it is started on the next
          request, so the first connection can take a few seconds.
        </p>
      </div>
      <PanelActions>
        <Button disabled={!url.trim()} onClick={() => onAdd(url.trim())}>
          Add
        </Button>
      </PanelActions>
    </>
  );
}

/** The address of a host running somewhere this application did not start it. */
function RemoteHost({
  onAdd,
  desktop,
}: {
  onAdd: (baseUrl: string) => void;
  desktop: boolean;
}) {
  const [url, setUrl] = useState("");
  return (
    <>
      <div className="space-y-2">
        <Label htmlFor="connection-url">Host URL</Label>
        <Input
          id="connection-url"
          placeholder="https://acme.example.com"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
        {/*
          The most likely support question this tab generates, answered where
          it is cheapest to answer. A browser reaches a gateway from *this*
          origin, so the gateway has to allow it — and there is no wildcard,
          because the session is a credential. The desktop proxies from Rust,
          where no origin is involved and none of this applies.
        */}
        {desktop ? null : (
          <p className="text-muted-foreground text-xs">
            A gateway you run yourself. It has to allow this console&apos;s address in{" "}
            <code>OPENCOMPANY_CORS_ORIGINS</code>, or your browser will block every
            reply.
          </p>
        )}
      </div>
      <PanelActions>
        <Button disabled={!url.trim()} onClick={() => onAdd(url.trim())}>
          Add
        </Button>
      </PanelActions>
    </>
  );
}

/**
 * A host on another machine that is bound to loopback there, reached through a
 * tunnel this application opens.
 *
 * The destination leads and everything else has a default, because the shape
 * this is for is `acme-vps` — a `Host` alias out of `~/.ssh/config`. Anyone
 * with a bastion, a jump host or a non-default key has already written that
 * file, and a form that re-asks for its contents is one they will fill in
 * wrong.
 */
function SshHost({
  onAdd,
  onDone,
}: {
  onAdd: (target: SshTarget) => Promise<void>;
  onDone: () => void;
}) {
  const [destination, setDestination] = useState("");
  const [remotePort, setRemotePort] = useState(String(DEFAULT_REMOTE_PORT));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add(): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await onAdd({
        destination: destination.trim(),
        remotePort: Number(remotePort) || DEFAULT_REMOTE_PORT,
      });
      onDone();
    } catch (err) {
      // `ssh`'s own words, kept: "Host key verification failed" and
      // "Permission denied (publickey)" each name a specific thing to go and
      // fix, and a summary of either would name none of them.
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <div className="space-y-2">
        <Label htmlFor="ssh-destination">Machine</Label>
        <Input
          id="ssh-destination"
          placeholder="acme-vps, or deploy@10.0.0.4"
          value={destination}
          onChange={(e) => setDestination(e.target.value)}
        />
        <Label htmlFor="ssh-remote-port">Port it serves on there</Label>
        <Input
          id="ssh-remote-port"
          inputMode="numeric"
          placeholder={String(DEFAULT_REMOTE_PORT)}
          value={remotePort}
          onChange={(e) => setRemotePort(e.target.value)}
        />
        <p className="text-muted-foreground text-xs">
          The host stays reachable only from that machine; this application
          forwards it over your own ssh. Your key has to be one ssh can use
          without asking — an agent key, or one with no passphrase.
        </p>
        {error ? <p className="text-destructive text-xs">{error}</p> : null}
      </div>
      <PanelActions>
        <Button disabled={busy || !destination.trim()} onClick={() => void add()}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          Connect
        </Button>
      </PanelActions>
    </>
  );
}

/**
 * The hosts this machine runs: what is listening, what is not, and a name field
 * for one more.
 *
 * Stopped instances are listed rather than hidden, and this is the only place
 * they appear. A stopped instance has no address, so it cannot be a row in the
 * switcher — the menu lists connections, and a connection with nothing
 * listening at it is a permanent probe failure. Here it is a row with a Start
 * button, which is what it actually is.
 */
function LocalInstances({
  instances,
  onAdd,
  onStart,
  onStop,
  onDelete,
}: {
  instances: LocalInstance[];
  onAdd: (label: string) => Promise<void>;
  onStart?: (id: string) => Promise<void>;
  onStop?: (id: string) => Promise<void>;
  onDelete?: (id: string) => Promise<void>;
}) {
  const [label, setLabel] = useState("");
  /** Which instance has a command in flight, or `"new"` for the create. */
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  /**
   * Runs one command, keeping its failure on screen.
   *
   * Every one of these can fail for a reason the operator has to read — most
   * often the data root being held by an `opencompany serve` in a terminal —
   * and a rejected promise with no `catch` is a console that silently does
   * nothing when the button is pressed.
   */
  async function run(key: string, action: () => Promise<void>): Promise<void> {
    setBusy(key);
    setError(null);
    try {
      await action();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="space-y-4" data-testid="local-instances">
      <ul className="space-y-1">
        {instances.map((instance) => (
          <li
            key={instance.id}
            data-testid={`local-instance-${instance.id}`}
            data-running={instance.running}
            className="flex items-center justify-between gap-2 rounded-md border px-3 py-2"
          >
            <div className="min-w-0">
              <div className="truncate text-sm">{instance.label}</div>
              <div className="truncate text-xs text-muted-foreground">
                {instance.running ? instance.baseUrl : (instance.error ?? "Stopped")}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              {instance.running ? (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={!onStop || busy !== null}
                  onClick={() => void run(instance.id, () => onStop!(instance.id))}
                >
                  {busy === instance.id ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <Square className="size-4" />
                  )}
                  Stop
                </Button>
              ) : (
                <Button
                  variant="ghost"
                  size="sm"
                  disabled={!onStart || busy !== null}
                  onClick={() => void run(instance.id, () => onStart!(instance.id))}
                >
                  {busy === instance.id ? (
                    <Loader2 className="size-4 animate-spin" />
                  ) : (
                    <Play className="size-4" />
                  )}
                  Start
                </Button>
              )}
              {instance.id !== "default" && onDelete ? (
                <DeleteLocalInstance
                  instance={instance}
                  disabled={busy !== null}
                  onConfirm={() => run(instance.id, () => onDelete(instance.id))}
                />
              ) : null}
            </div>
          </li>
        ))}
      </ul>

      <div className="space-y-2">
        <Label htmlFor="local-instance-label">Name</Label>
        <Input
          id="local-instance-label"
          placeholder="Acme"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
        />
        <p className="text-xs text-muted-foreground">
          A new host on this computer, with its own data and its own companies. Nothing is shared
          with the ones above.
        </p>
      </div>

      {error ? (
        <p data-testid="local-instance-error" className="text-xs text-destructive">
          {error}
        </p>
      ) : null}

      <PanelActions>
        <Button
          data-testid="local-instance-add"
          disabled={!label.trim() || busy !== null}
          onClick={() =>
            void run("new", async () => {
              await onAdd(label.trim());
              setLabel("");
            })
          }
        >
          {busy === "new" ? <Loader2 className="size-4 animate-spin" /> : null}
          Run it here
        </Button>
      </PanelActions>
    </div>
  );
}

/** Deletes a desktop-created company only after spelling out the consequence. */
function DeleteLocalInstance({
  instance,
  disabled,
  onConfirm,
}: {
  instance: LocalInstance;
  disabled: boolean;
  onConfirm: () => Promise<void>;
}) {
  return (
    <AlertDialog>
      <AlertDialogTrigger
        render={
          <Button
            data-testid={`local-instance-delete-${instance.id}`}
            variant="ghost"
            size="sm"
            disabled={disabled}
            aria-label={`Delete ${instance.label}`}
          >
            <Trash2 className="size-4" />
          </Button>
        }
      />
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {instance.label} from this computer?</AlertDialogTitle>
          <AlertDialogDescription>
            This permanently deletes the company and all of its data in {instance.dataDir}.
            This cannot be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            data-testid={`local-instance-delete-confirm-${instance.id}`}
            className="bg-destructive text-white hover:bg-destructive/90"
            onClick={() => void onConfirm()}
          >
            Delete company
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
