// The inference page's data: one read, one set of writes, one place they land.
//
// The LLM page renders from this. **No decisions here either.** This is
// fetching, in-flight state, and where a response lands. Which provider a
// category offers, what a probe class means, what removing one costs — all of
// that is in `connect.ts`, `classify.ts` and `removal.ts`, as functions over
// plain data.
//
// Per-workload routing (`routes`, `mode`, `orphaned`, `saveRoutes`) is gone
// (keys rework, issue #2306, phase 5b) — a company now has one default
// `{provider, model}` and agents may pin their own. See `docs/key-reworks/`.

import { useCallback, useEffect, useState } from "react";

import { toast } from "sonner";

import { ApiError } from "@/api/types";
import type { OpenCompanyClient } from "@/api/client";
import {
  addProvider,
  deleteProvider,
  editProvider,
  getInferenceStatus,
  probeDraft,
  restartInference,
  setDefaultProvider,
  setManagedEnabled,
  setManagedKey,
  setProviderEnabled,
  testManaged,
  testProvider,
} from "@/api/inference";
import type {
  AddProviderInput,
  EditProviderInput,
  InferenceStatus,
  ProbeResult,
  ProviderMutation,
} from "@/api/inference";
import type { Provider } from "./types";

/** What the page is doing. */
export type InferenceLoad = "loading" | "ready" | "unavailable" | "error";

/** What the page holds. */
export interface InferenceState {
  load: InferenceLoad;
  status: InferenceStatus | null;
  providers: Provider[];
  /** The slug currently mid-request, so one row's controls settle rather than the page. */
  busySlug: string | null;
}

/** The mutations the page can perform. */
export interface InferenceActions {
  reload: () => Promise<void>;
  add: (input: AddProviderInput) => Promise<ProviderMutation>;
  edit: (slug: string, input: EditProviderInput) => Promise<ProviderMutation>;
  /** `confirmInUse` resends after a `409 in_use` named what depends on this row. */
  remove: (slug: string, confirmInUse?: boolean) => Promise<ProviderMutation>;
  /** `confirmInUse` resends after a `409 in_use` — only turning a provider off can strand anything. */
  setEnabled: (slug: string, enabled: boolean, confirmInUse?: boolean) => Promise<ProviderMutation>;
  /** Sets the company default to `{provider, model}` — a model is always required (Q2). */
  makeDefault: (slug: string, model: string) => Promise<ProviderMutation>;
  /**
   * @deprecated keys-rework #2306, decision X6: the console no longer calls
   * `PUT …/inference/managed/key` to add or replace a key — every provider,
   * TinyHumans included, goes through `add`/`edit` instead. Kept only for
   * clearing a **pre-row** legacy credential (`ProviderList`'s deprecated
   * "Remove key" on the legacy Managed row), which has no indexed row to
   * `edit` yet. Removable once item 10 drops the fallback chains.
   */
  saveManagedKey: (key: string) => Promise<ProviderMutation>;
  setManagedOn: (enabled: boolean) => Promise<ProviderMutation>;
  testManagedChain: () => Promise<ProbeResult>;
  /**
   * Ask an endpoint what it publishes, **before** anything is written.
   *
   * The add dialog needs this to offer a model: a model is always required
   * (D-model), and the only honest moment to ask is with that endpoint's own
   * list in hand. Nothing is stored — the draft's key travels one way and is
   * never written by this.
   */
  probeDraftEndpoint: (draft: { baseUrl: string; key?: string; kind?: string }) => Promise<ProbeResult>;
  test: (slug: string, model?: string) => Promise<ProbeResult>;
  restart: () => Promise<void>;
}

/**
 * The inference page's state and its writes.
 *
 * Every mutation re-reads the status from its own response rather than firing a
 * second GET: the host answers each write with the whole status precisely so
 * that the console never has to reconcile a partial update against what it
 * already had.
 */
export function useInference(
  client: OpenCompanyClient,
  company: string | null,
): InferenceState & InferenceActions {
  const [load, setLoad] = useState<InferenceLoad>("loading");
  const [status, setStatus] = useState<InferenceStatus | null>(null);
  const [busySlug, setBusySlug] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const next = await getInferenceStatus(client, company);
      setStatus(next);
      setLoad("ready");
    } catch (err) {
      // A host that does not serve this route at all is not an error worth a
      // banner — the page simply is not available on that build.
      setLoad(err instanceof ApiError && err.status === 404 ? "unavailable" : "error");
    }
  }, [client, company]);

  useEffect(() => {
    setLoad("loading");
    void reload();
  }, [reload]);

  /**
   * Runs a provider write, parking the row it touches and landing the result.
   *
   * **The outcome is a toast, and a failure is never silent.** The error is
   * re-thrown as well as toasted: a form or confirm dialog that is still open
   * shows its own failure inline, where the field the operator has to correct
   * is — and, for a `409 in_use`, so the dialog can re-open with the refusal's
   * own `usedBy` and message (keys rework, issue #2306's confirmation
   * contract) rather than a generic toast being the only trace of it.
   */
  const write = useCallback(
    async (
      slug: string | null,
      run: () => Promise<ProviderMutation>,
      /**
       * Round-2 review, P2-1: some callers already show a failure inline in
       * the dialog that is still open (the default-model step, for one) —
       * `silentError` skips this hook's own toast for exactly those, so the
       * same refusal is not said twice in two different places at once.
       */
      opts?: { silentError?: boolean },
    ) => {
      setBusySlug(slug);
      try {
        const result = await run();
        setStatus(result.status);
        // The host's own sentence, which already names the provider it is about.
        toast.success(result.note);
        return result;
      } catch (err) {
        // A `409 in_use` is not "something went wrong" — it is the host asking
        // for confirmation, and the caller (a confirm dialog) shows its own
        // message and `usedBy` rather than a duplicate toast.
        const inUse = err instanceof ApiError && err.status === 409 && err.code === "in_use";
        if (!inUse && !opts?.silentError) {
          toast.error(err instanceof ApiError ? err.message : "That change could not be saved.");
        }
        throw err;
      } finally {
        setBusySlug(null);
      }
    },
    [],
  );

  return {
    load,
    status,
    providers: status?.providers ?? [],
    busySlug,
    reload,
    add: (input) => write(null, () => addProvider(client, company, input)),
    edit: (slug, input) => write(slug, () => editProvider(client, company, slug, input)),
    remove: (slug, confirmInUse) => write(slug, () => deleteProvider(client, company, slug, confirmInUse)),
    setEnabled: (slug, enabled, confirmInUse) =>
      write(slug, () => setProviderEnabled(client, company, slug, enabled, confirmInUse)),
    // silentError: the dialog already shows its own failure inline (P2-1).
    makeDefault: (slug, model) =>
      write(slug, () => setDefaultProvider(client, company, slug, model), { silentError: true }),
    saveManagedKey: (key) => write(null, () => setManagedKey(client, company, key)),
    setManagedOn: (enabled) => write(null, () => setManagedEnabled(client, company, enabled)),
    probeDraftEndpoint: (draft) => probeDraft(client, company, draft),
    testManagedChain: async () => {
      const result = await testManaged(client, company);
      // The test records health against the managed slug, and the row renders
      // it — so the status has to be re-read or the row keeps showing what it
      // knew before the operator asked.
      setStatus(await getInferenceStatus(client, company));
      // Same rule as a provider's test: the answer is the row's, not the page's.
      return result;
    },
    test: async (slug, model) => {
      setBusySlug(slug);
      try {
        const result = await testProvider(client, company, slug, model);
        // The test writes a health record, and the row renders it — so the
        // status has to be re-read or the row keeps showing what it knew before
        // the operator asked.
        setStatus(await getInferenceStatus(client, company));
        // **No page-level note.** A test's answer belongs to the row that asked
        // — with two providers connected, a line under the card says nothing
        // about which one was tested, which is the bug moving the control fixed.
        return result;
      } finally {
        setBusySlug(null);
      }
    },
    restart: async () => {
      const result = await restartInference(client, company);
      setStatus(result.status);
      toast.success(result.note);
    },
  };
}
