import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, Plus, TriangleAlert } from "lucide-react";
import { toast } from "sonner";

import {
  clearSearch,
  connectSearchProvider,
  getSearch,
  removeSearchProvider,
  replaceSearchProviderKey,
  setSearchDefault,
  testSearchProvider,
  updateSearchProvider,
  type SearchStatus,
} from "@/api/search";
import type { OpenCompanyClient } from "@/api/client";
import type { UsedBy } from "@/api/types";
import { AdminOnlyNotice } from "@/components/admin-only-notice";
import { PageHeader } from "@/components/page-header";
import { useCanManage } from "@/hooks/use-can-manage";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { GrantNamespace } from "@/components/grant-namespace";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { AddProviderDialog } from "@/search-providers/AddProviderDialog";
import {
  ProviderConnectDialog,
  type ConnectIntent,
} from "@/search-providers/ProviderConnectDialog";
import { ProviderList } from "@/search-providers/ProviderList";
import { COPY, labelOf } from "@/search-providers/catalogue";
import {
  TEST_RESULT_MS,
  confirmCopy,
  describeProbe,
  describeTest,
  type TestState,
} from "@/search-providers/classify";
import { guardedOutcome } from "@/search-providers/in-use";
import { hasUsedBy } from "@/lib/used-by";
import type {
  ConfirmTarget,
  ProbeClass,
  SearchProvider,
} from "@/search-providers/types";

interface Props {
  client: OpenCompanyClient;
  company: string | null;
}

/** The message out of a rejected request, whatever it was rejected with. */
function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * Connections → Search: which index this company's teammates search.
 *
 * Every teammate that holds the `search` grant gets a `web_search` tool. This
 * page decides what is behind it: the platform's own account (metered, capped,
 * nothing to configure) or a search account of the company's own.
 *
 * # Why this is a list
 *
 * There used to be one provider slot and one key slot. Switching provider
 * without re-pasting the key left the old provider's key authenticating against
 * the new one — and this page, the status route and the harness all agreed the
 * company was correctly configured until an agent's first search came back 401.
 * Each provider holds its own credential now, and the list is what makes that
 * visible. See `docs/modules/search/current-state.md`.
 *
 * # Why "Default" is a stronger word here than on the LLM page
 *
 * There, the default serves *unset workloads* and other providers serve the
 * workloads routed to them. Here every provider's search is aliased to the one
 * `web_search` tool, so the default is the **only** provider in use and the rest
 * are stored credentials standing ready. That is the single sub-line under the
 * list, and it is the only explanatory sentence on the page.
 *
 * # Two problems this page cannot fix
 *
 *   - not granted  — a key is stored and STILL nothing reaches a teammate,
 *                    because the manifest does not grant `search`.
 *   - not in build — the host was compiled without the agent harness.
 *
 * Both are said above the list, so an operator does not fill it in and wonder
 * why nothing happened.
 *
 * # No decisions live here
 *
 * Row sub-lines, control sets, add-dialog contents and probe copy are all
 * functions in `@/search-providers`, each unit-tested. What is left is layout
 * plus handlers.
 */
export function SearchView({ client, company }: Props) {
  const [status, setStatus] = useState<SearchStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busySlug, setBusySlug] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [intent, setIntent] = useState<ConnectIntent | null>(null);
  const [confirm, setConfirm] = useState<ConfirmTarget | null>(null);
  // The host's own `in_use` sentence from a first, unconfirmed attempt on the
  // current `confirm` target (`docs/key-reworks/in-use-guards.md` §2/§3):
  // `null` until a guarded attempt is refused, at which point the dialog
  // re-renders showing this instead of its generic question, and the next
  // attempt sends `confirmInUse: true`. Reset whenever the dialog closes, so
  // a later confirm target opens generic again rather than showing a stale
  // reason from an unrelated row.
  const [confirmNotice, setConfirmNotice] = useState<string | null>(null);
  // The `usedBy` that came with `confirmNotice`'s refusal, when the row
  // changed between the dialog opening and the click landing (round-3
  // review, P1-2) — fresher than whatever `openConfirm` read at open time,
  // so it wins in `shownUsedBy` below.
  const [confirmRefusalUsedBy, setConfirmRefusalUsedBy] = useState<UsedBy | undefined>(undefined);
  const [tests, setTests] = useState<Record<string, TestState>>({});
  // What the last check learnt about each provider, keyed by slug. Sourced from
  // things that already happen — the connect probe and the manual Test — rather
  // than from a poller, which would cost a request per provider per interval
  // across every company on the host AND spend the company's money to learn it.
  const [health, setHealth] = useState<Record<string, ProbeClass>>({});

  // Whether this viewer may change where this company's teammates search.
  //
  // Governs the whole page: every write route and the probe are
  // `AdminScopedCompany`. A search key is billed to whoever's account it belongs
  // to, and the provider choice decides which index — and which retention
  // policy — every agent's queries are handed to.
  const canManage = useCanManage(client, company);

  const timers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  useEffect(() => {
    const pending = timers.current;
    return () => {
      Object.values(pending).forEach(clearTimeout);
    };
  }, []);

  const load = useCallback(async () => {
    try {
      const next = await getSearch(client, company);
      setStatus(next);
      setLoadError(null);
    } catch (err) {
      setLoadError(reason(err));
    }
  }, [client, company]);

  useEffect(() => {
    void load();
  }, [load]);

  /**
   * Which configuration of each row a check belongs to.
   *
   * Bumped whenever the row's configuration changes. A Test captures it before
   * it awaits and discards its result if it moved — otherwise a check started
   * against the old key and finishing after "Replace key" wrote the old key's
   * verdict back onto the new one, and `forgetHealth` clearing the map first
   * made no difference because the late write landed after it.
   */
  const generations = useRef<Record<string, number>>({});
  /**
   * The same, for every row at once. Disconnect all has to invalidate a check
   * on a row that has never been changed and so has no entry above.
   */
  const epoch = useRef(0);

  /**
   * Drops what a probe once said about a row, and anything still on its way.
   *
   * Health is the diagnosis of a *configuration*, so it stops meaning anything
   * the moment the configuration changes. Left behind, a row that was tested
   * with a rejected key still read "key rejected" after the key was replaced —
   * a red mark on a credential nothing had ever checked, and the same stale
   * state survived remove-and-reconnect. The row's test result and its clear
   * timer go too, and any check still in flight is invalidated. Omit `slug` to
   * forget all of them, which is what disconnecting everything means.
   */
  const forgetHealth = useCallback((slug?: string) => {
    if (slug === undefined) epoch.current += 1;
    const slugs =
      slug === undefined
        ? Object.keys({ ...generations.current, ...timers.current })
        : [slug];
    for (const each of slugs) {
      generations.current[each] = (generations.current[each] ?? 0) + 1;
      clearTimeout(timers.current[each]);
      delete timers.current[each];
    }
    setHealth((prior) => {
      if (slug === undefined) return {};
      const next = { ...prior };
      delete next[slug];
      return next;
    });
    setTests((prior) => {
      if (slug === undefined) return {};
      const next = { ...prior };
      delete next[slug];
      return next;
    });
  }, []);

  /**
   * Runs one write, settling only the row it belongs to.
   *
   * Every call passes a `done` sentence: an action whose only feedback is the
   * list quietly re-rendering leaves the operator unsure whether it fired, and
   * on a page where one of the actions moves which account gets billed that is
   * not a small doubt.
   */
  const run = useCallback(
    async (
      slug: string,
      done: string,
      work: () => Promise<SearchStatus>,
      /** Whether success makes this row's last probe result meaningless. */
      changesConfiguration = false,
    ) => {
      setBusySlug(slug);
      try {
        setStatus(await work());
        if (changesConfiguration) forgetHealth(slug === "__all__" ? undefined : slug);
        toast.success(done);
      } catch (err) {
        toast.error(reason(err));
      } finally {
        setBusySlug(null);
      }
    },
    [forgetHealth],
  );

  /**
   * Opens a confirm dialog on a fresh read (round-3 review, P1-2 — the same
   * rule as the LLM page's `openConfirm`): the target's `usedBy` seeds from
   * whatever this page already has loaded, so the dialog shows something
   * immediately, and a re-fetch runs in the background so `shownUsedBy` below
   * has the live answer by the time the operator actually clicks.
   */
  const openConfirm = useCallback(
    (target: ConfirmTarget) => {
      setConfirmNotice(null);
      setConfirmRefusalUsedBy(undefined);
      setConfirm(target);
      void load();
    },
    [load],
  );

  /**
   * Runs the confirm dialog's pending action, which the host may refuse with
   * `409 in_use` when `confirmInUse` undersells a row that changed underneath
   * the dialog (`docs/key-reworks/in-use-guards.md` §2). One implementation
   * shared by every kind `AlertDialog` in this file can confirm — the toggle,
   * remove, remove-key and disconnect-all targets — so the "reopen with the
   * host's reason, then resend confirmed" state machine cannot drift between
   * them; the decisions themselves live in `@/search-providers/in-use`, which
   * is what is actually under test. `disconnect-all` is never refused by the
   * host today (it carries no guard of its own — see `docs/key-reworks/
   * in-use-guards.md` §1's table, which files search's guard under single-row
   * removal/disable and the default, not the bulk clear), but routing it
   * through the same function costs nothing and keeps one code path rather
   * than two.
   *
   * `confirmInUse` is the caller's own `confirmInUseNow` at click time — sent
   * only when the dialog actually showed usage, never blind.
   */
  const runGuarded = useCallback(
    async (
      slug: string,
      done: string,
      confirmInUse: boolean,
      work: (confirmInUse: boolean) => Promise<SearchStatus>,
      /** Whether success makes this row's last probe result meaningless. */
      changesConfiguration: boolean,
    ) => {
      setBusySlug(slug);
      try {
        const next = await work(confirmInUse);
        setStatus(next);
        if (changesConfiguration) forgetHealth(slug === "__all__" ? undefined : slug);
        toast.success(done);
        setConfirm(null);
        setConfirmNotice(null);
        setConfirmRefusalUsedBy(undefined);
      } catch (err) {
        const outcome = guardedOutcome(err, confirmInUse);
        if (outcome.action === "reopen") {
          setConfirmNotice(outcome.message);
          setConfirmRefusalUsedBy(outcome.usedBy);
          return;
        }
        setConfirm(null);
        setConfirmNotice(null);
        setConfirmRefusalUsedBy(undefined);
        toast.error(reason(err));
      } finally {
        setBusySlug(null);
      }
    },
    [confirmNotice, forgetHealth],
  );

  /** Records a finished test on the row, and clears it after ten seconds. */
  const settleTest = useCallback((slug: string, state: TestState) => {
    setTests((prior) => ({ ...prior, [slug]: state }));
    clearTimeout(timers.current[slug]);
    timers.current[slug] = setTimeout(() => {
      setTests((prior) => ({ ...prior, [slug]: { kind: "idle" } }));
    }, TEST_RESULT_MS);
  }, []);

  const onTest = useCallback(
    async (provider: SearchProvider) => {
      const slug = provider.slug;
      const generation = generations.current[slug] ?? 0;
      const startedIn = epoch.current;
      // Whether the row changed while this check was out. A stale verdict is
      // not written at all — not as health, and not as a test result.
      const stale = () =>
        (generations.current[slug] ?? 0) !== generation ||
        epoch.current !== startedIn;
      setTests((prior) => ({ ...prior, [slug]: { kind: "testing" } }));
      try {
        const result = await testSearchProvider(client, company, { slug });
        if (stale()) return;
        setStatus(result.status);
        if (result.ok) {
          setHealth((prior) => {
            const next = { ...prior };
            delete next[slug];
            return next;
          });
          settleTest(slug, { kind: "done", ok: true, message: "ok" });
        } else {
          const probeClass = result.probeClass ?? "unknown";
          setHealth((prior) => ({ ...prior, [slug]: probeClass }));
          settleTest(slug, {
            kind: "done",
            ok: false,
            message: describeTest(probeClass, provider.label),
          });
        }
      } catch (err) {
        if (stale()) return;
        settleTest(slug, {
          kind: "done",
          ok: false,
          message: reason(err),
        });
      }
    },
    [client, company, settleTest],
  );

  /** The add/connect/replace/re-address submit, whichever the dialog is for. */
  const onConnectSubmit = useCallback(
    async (values: { apiKey?: string; endpoint?: string }) => {
      if (!intent) return;
      const slug = intent.slug;
      const label = labelOf(slug);
      setBusySlug(slug);
      try {
        if (intent.kind === "replace-key") {
          setStatus(
            await replaceSearchProviderKey(
              client,
              company,
              slug,
              values.apiKey ?? "",
            ),
          );
          // The old diagnosis was about the old key.
          forgetHealth(slug);
          toast.success(`${label} key replaced.`);
        } else if (intent.kind === "edit-endpoint") {
          setStatus(
            await updateSearchProvider(client, company, slug, {
              endpoint: values.endpoint,
            }),
          );
          // And this one was about the old address.
          forgetHealth(slug);
          toast.success(`${label} address saved.`);
        } else {
          const result = await connectSearchProvider(client, company, {
            slug,
            ...values,
          });
          setStatus(result.status);
          // A new connection has no history, and the slug may be one that was
          // removed and re-added — which used to inherit the removed row's
          // diagnosis. Cleared before the probe below records its own.
          forgetHealth(slug);
          const probeClass = result.probeClass;
          if (result.ok) {
            toast.success(`${label} connected.`);
          } else if (probeClass) {
            const advisory = describeProbe(probeClass, label);
            // Only an auth-class failure is destructive, and only it is an
            // error. Every other class kept the record AND the credential, so
            // colouring it red would be a lie about what happened: the save
            // succeeded and only reachability is in question.
            if (advisory.tone === "error") {
              toast.error(advisory.message);
            } else {
              setHealth((prior) => ({ ...prior, [slug]: probeClass }));
              toast.warning(advisory.message);
            }
          }
        }
        setIntent(null);
      } catch (err) {
        toast.error(reason(err));
      } finally {
        setBusySlug(null);
      }
    },
    [client, company, forgetHealth, intent],
  );

  const header = (
    <PageHeader
      title="Search"
      width="full"
      description={
        <>
          Where your teammates look things up. Every teammate that can search
          gets one <code>web_search</code> tool — this decides which index
          answers it, and whose account pays for the call.
        </>
      }
    />
  );

  if (loadError) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        <div className="w-full px-4 py-6">
          <Alert variant="destructive" data-testid="search-load-error">
            <TriangleAlert className="size-4" />
            <AlertDescription>
              Could not load search settings: {loadError}
            </AlertDescription>
          </Alert>
        </div>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        {header}
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          <Loader2 className="mr-2 size-4 animate-spin" /> Loading search…
        </div>
      </div>
    );
  }

  // Defaulted rather than assumed. These fields arrived with the provider list,
  // and a console that throws on a response from an older host turns a missing
  // field into a white screen — which is how a status page stops being able to
  // report that anything is wrong at all.
  const providers = status.providers ?? [];

  /**
   * What the dialog currently shows as depending on the target — the
   * refusal's own fresher answer once one has landed, else the live row (or,
   * for `disconnect-all`, whether any row holds the search default) from the
   * latest status read, else whatever `openConfirm` captured at click time.
   */
  const shownUsedBy = ((): UsedBy | undefined => {
    if (confirmRefusalUsedBy) return confirmRefusalUsedBy;
    if (!confirm) return undefined;
    if (confirm.kind === "disconnect-all") {
      return providers.some((p) => p.usedBy?.default) ? { default: true } : confirm.usedBy;
    }
    return providers.find((p) => p.slug === confirm.slug)?.usedBy ?? confirm.usedBy;
  })();
  /** Sent only when the dialog actually showed usage — never blind (round-3 review, P1-2). */
  const confirmInUseNow = hasUsedBy(shownUsedBy);

  return (
    <div className="flex min-h-0 flex-1 flex-col" data-testid="search-view">
      {header}
      <div className="min-h-0 w-full flex-1 space-y-6 overflow-y-auto px-4 py-6">
        {!canManage && (
          <AdminOnlyNotice
            testId="search-read-only"
            title="Only an admin can change where this company searches"
          >
            Whatever a teammate types into a search reaches the provider marked
            here, under that provider&rsquo;s own retention policy &mdash; and
            the calls are billed to whichever account the key belongs to. Both
            are the company&rsquo;s to decide, so an admin decides them. You can
            see which index answers today.
          </AdminOnlyNotice>
        )}

        {!status.inBuild && (
          <Alert variant="warning" data-testid="search-not-in-build">
            <TriangleAlert className="size-4" />
            <AlertDescription>
              This host was built without the agent tools, so these settings
              will be stored and have no effect. Rebuild with the{" "}
              <code>openhuman</code> feature.
            </AlertDescription>
          </Alert>
        )}

        {status.inBuild && !status.granted && (
          <GrantNamespace
            client={client}
            company={company}
            namespace="search"
            canManage={canManage}
            explanation="No teammate will get a search tool even once a provider is connected."
            onGranted={load}
            testId="search-not-granted"
          />
        )}

        {/* Keys rework (#2306), decision D-never-clear-default (X14): a
            disable or delete no longer clears `search/default`, so a marker
            naming a removed or switched-off provider can now sit
            indefinitely — this says so, instead of the page silently
            answering through whatever the fallback happens to resolve to. */}
        {status.defaultNotice && (
          <Alert variant="warning" data-testid="search-default-notice">
            <TriangleAlert className="size-4" />
            <AlertDescription>{status.defaultNotice}</AlertDescription>
          </Alert>
        )}

        <Card>
          <CardContent className="flex flex-wrap items-center justify-between gap-4">
            <div className="grid min-w-0 leading-tight">
              <p className="text-sm font-medium">Search providers</p>
              <p className="text-xs text-muted-foreground">
                Connect a search account of your own.
              </p>
            </div>
            <Button
              type="button"
              className="shrink-0"
              disabled={!canManage}
              data-testid="search-add"
              onClick={() => setAdding(true)}
            >
              <Plus className="size-4" />
              Add a provider
            </Button>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-0">
            <p className="px-4 pt-4 text-3xs font-medium tracking-wide text-muted-foreground uppercase">
              Connected
            </p>
            <ProviderList
              providers={providers}
              inBuild={status.inBuild}
              managedConfigured={status.managedConfigured ?? false}
              managedKeyConfigured={status.managedKeyConfigured ?? false}
              managedDailyCallCap={status.managedDailyCallCap ?? 0}
              canManage={canManage}
              busySlug={busySlug}
              health={(slug) => health[slug]}
              testState={(slug) => tests[slug] ?? { kind: "idle" }}
              onAdd={() => setAdding(true)}
              // Every on/off toggle asks first, unconditionally — the
              // operator's mid-project ask, not only where something depends
              // on the row (`docs/key-reworks/in-use-guards.md`). The Switch
              // itself does not move until the dialog is confirmed.
              onToggle={(provider, enabled) =>
                openConfirm({
                  kind: "toggle",
                  slug: provider.slug,
                  label: provider.label,
                  enabling: enabled,
                  usedBy: provider.usedBy,
                })
              }
              onTest={(provider) => void onTest(provider)}
              onReplaceKey={(provider) =>
                setIntent({ kind: "replace-key", slug: provider.slug })
              }
              onManagedReplaceKey={() =>
                setIntent({ kind: "replace-key", slug: "managed" })
              }
              onManagedRemoveKey={() =>
                openConfirm({
                  kind: "remove-key",
                  slug: "managed",
                  label: "Managed",
                })
              }
              // Destructive, so it asks first. Both of these are irreversible in
              // the only sense that matters here: the key is write-only and is
              // never shown back, so an operator who clears the wrong one cannot
              // retype it from the screen.
              onRemoveKey={(provider) =>
                openConfirm({
                  kind: "remove-key",
                  slug: provider.slug,
                  label: provider.label,
                  usedBy: provider.usedBy,
                })
              }
              onEditEndpoint={(provider) =>
                setIntent({
                  kind: "edit-endpoint",
                  slug: provider.slug,
                  endpoint: provider.endpoint,
                })
              }
              onMakeDefault={(provider) =>
                void run(
                  provider.slug,
                  `Teammates now search through ${provider.label}.`,
                  () => setSearchDefault(client, company, provider.slug),
                )
              }
              onRemove={(provider) =>
                openConfirm({
                  kind: "remove",
                  slug: provider.slug,
                  label: provider.label,
                  usedBy: provider.usedBy,
                })
              }
            />
          </CardContent>
        </Card>

        {/* The only explanatory sentence that survives the deletion pass, and it
            earns its place: it is the one thing on this page that says a
            connected-but-not-default provider is not being used. */}
        <p
          className="text-xs text-muted-foreground"
          data-testid="search-default-note"
        >
          {COPY.defaultIsTheOnlyOne}
        </p>

        {providers.length > 0 && canManage && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            data-testid="search-disconnect-all"
            onClick={() =>
              openConfirm({
                kind: "disconnect-all",
                label: "every provider",
                usedBy: providers.some((p) => p.usedBy?.default) ? { default: true } : undefined,
              })
            }
          >
            Disconnect all providers
          </Button>
        )}
      </div>

      {/* Every destructive action and on/off toggle passes through here. `remove`,
          `remove-key` and `disconnect-all` are irreversible in the sense that
          matters on this page: a key is write-only and is never shown back, so
          an operator who clears the wrong one has nothing on screen to retype.
          Any of the four may come back `409 in_use` on a first, unconfirmed
          attempt (`docs/key-reworks/in-use-guards.md` §2) — `runGuarded` keeps
          the dialog open and swaps in the host's own reason via `confirmNotice`
          rather than closing on a generic error, so a second, now-informed click
          can resend confirmed. */}
      <AlertDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (open || busySlug !== null) return;
          setConfirm(null);
          setConfirmNotice(null);
          setConfirmRefusalUsedBy(undefined);
        }}
      >
        <AlertDialogContent data-testid="search-confirm">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {confirmCopy(confirm, confirmNotice).title}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {confirmCopy(confirm ? { ...confirm, usedBy: shownUsedBy } : null, confirmNotice).body}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={busySlug !== null}>
              Cancel
            </AlertDialogCancel>
            <AlertDialogAction
              disabled={busySlug !== null}
              data-testid="search-confirm-action"
              onClick={(event) => {
                const pending = confirm;
                if (!pending) return;
                // Kept open on a stale-UI 409 so it can reopen with the
                // host's own reason — `runGuarded` closes it itself on
                // success or on an unrelated failure.
                event.preventBaseUIHandler();
                // All four change the configuration (or nothing, for a
                // cancelled toggle that never wrote anything), so all drop
                // what a probe last said about the row.
                if (pending.kind === "disconnect-all") {
                  void runGuarded(
                    "__all__",
                    "Disconnected. Searches go through the included account.",
                    confirmInUseNow,
                    (confirmInUse) => clearSearch(client, company, confirmInUse),
                    true,
                  );
                } else if (pending.kind === "remove") {
                  void runGuarded(
                    pending.slug,
                    `${pending.label} removed.`,
                    confirmInUseNow,
                    (confirmInUse) =>
                      removeSearchProvider(
                        client,
                        company,
                        pending.slug,
                        confirmInUse,
                      ),
                    true,
                  );
                } else if (pending.kind === "remove-key") {
                  void runGuarded(
                    pending.slug,
                    `${pending.label} key removed.`,
                    confirmInUseNow,
                    (confirmInUse) =>
                      replaceSearchProviderKey(
                        client,
                        company,
                        pending.slug,
                        "",
                        confirmInUse,
                      ),
                    true,
                  );
                } else {
                  void runGuarded(
                    pending.slug,
                    pending.enabling
                      ? `${pending.label} enabled.`
                      : `${pending.label} disabled.`,
                    confirmInUseNow,
                    (confirmInUse) =>
                      updateSearchProvider(
                        client,
                        company,
                        pending.slug,
                        { enabled: pending.enabling },
                        confirmInUse,
                      ),
                    false,
                  );
                }
              }}
            >
              {confirmCopy(confirm, confirmNotice).action}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AddProviderDialog
        open={adding}
        onOpenChange={setAdding}
        providers={providers}
        onChoose={(slug) => {
          setAdding(false);
          setIntent({ kind: "connect", slug });
        }}
      />
      <ProviderConnectDialog
        intent={intent}
        busy={busySlug !== null}
        onCancel={() => setIntent(null)}
        onSubmit={(values) => void onConnectSubmit(values)}
      />
    </div>
  );
}
