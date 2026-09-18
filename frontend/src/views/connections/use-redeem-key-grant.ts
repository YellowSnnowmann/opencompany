import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import type { OpenCompanyClient } from "@/api/client";
import { type CompanyCredentialMutation, finishCredentialLink } from "@/api/credential";
import { ApiError } from "@/api/types";
import { takeKeyLink, takeKeyLinkRefusal } from "@/lib/pending-key-link";

/**
 * Redeems a returning TinyHumans key grant (`POST …/credential/link/finish`).
 *
 * Non-visual on purpose. The grant comes back as a top-level navigation: `App`
 * takes the code off the URL before the first render, strips the address bar
 * because it is a live single-use credential, and parks it in a module-local
 * box that a reload empties. Whatever calls this hook is the only thing that
 * spends it — so a page must call it **unconditionally**, never behind state
 * that is null while the credential read is in flight or stays null when it
 * fails. The Account page calls it at the top of `ApiKeyView`, which is its
 * only caller.
 *
 * Returns whether a redemption is in flight.
 */
export function useRedeemKeyGrant(
  client: OpenCompanyClient,
  company: string | null,
  onConnected?: (result: CompanyCredentialMutation) => void,
): boolean {
  const [busy, setBusy] = useState(false);
  // StrictMode double-invokes effects, and the code is single-use: a second
  // call would spend nothing and report the host's "expired" refusal over a
  // connection that in fact succeeded.
  const redeeming = useRef(false);
  // The latest `client`/`company`/`onConnected` this hook was rendered with —
  // read at redemption *completion*, not closed over at redemption *start*.
  // `ConnectionsSection` remounts `ApiKeyView` on a company change, so that
  // transition already gets a fresh hook instance; a host switch that keeps
  // the same company selected does not remount anything and instead re-renders
  // this hook with a new `client`. Without this, the in-flight redemption's
  // closure kept the *old* `client`/`company`/`onConnected` for its whole
  // `await`, and reported success — and ran the (also stale) `onConnected`,
  // which the mounted view treats as "the current scope connected" — against
  // whichever scope happened to be selected when the promise resolved
  // (CodeRabbit review).
  const latest = useRef({ client, company, onConnected });
  latest.current = { client, company, onConnected };
  // Whether `ApiKeyView` is still mounted — a company change remounts it, but
  // navigating away from the Account page entirely just unmounts it with
  // nothing left to remount into. `latest.current` alone does not catch that:
  // it still holds whatever `client`/`company` this hook last rendered with,
  // which still equals `startClient`/`startCompany` after unmount, so a
  // response that settles post-unmount would otherwise still pass the scope
  // check below and pop a toast (and, on success, run `onConnected`) for a
  // view nobody can see (CodeRabbit review).
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const finish = useCallback(async (state: string, code: string) => {
    const { client: startClient, company: startCompany } = latest.current;
    setBusy(true);
    try {
      const result = await finishCredentialLink(startClient, startCompany, state, code);
      // The redemption itself always ran against `startClient`/`startCompany`
      // — that part is correct no matter what changes underneath it. What
      // must not happen is announcing that result, or handing it to
      // `onConnected`, against a *different* scope than the one that earned
      // it, or once nothing is mounted to show it to: discard rather than let
      // a stale grant surface as "connected" on whatever host/company is
      // current by the time the request returns, or after the view is gone.
      if (
        !mounted.current ||
        latest.current.client !== startClient ||
        latest.current.company !== startCompany
      ) {
        return;
      }
      toast.success("Connected to TinyHumans.", { description: result.note });
      latest.current.onConnected?.(result);
    } catch (err) {
      // Same guard as the success path: an unmounted or rescoped view must
      // not surface a stale failure toast either.
      if (
        !mounted.current ||
        latest.current.client !== startClient ||
        latest.current.company !== startCompany
      ) {
        return;
      }
      // The host's own words where it sent them: "that connection attempt has
      // expired" tells an operator to click again, which a generic failure
      // does not.
      toast.error(
        err instanceof ApiError ? err.message : "Couldn't finish connecting to TinyHumans.",
      );
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (redeeming.current) return;
    if (takeKeyLinkRefusal()) {
      redeeming.current = true;
      // Cancelling on the hub's consent screen lands here too, which is why this
      // is not worded as an error. Nothing was created either way.
      toast.info("No key was created.", {
        description: "The TinyHumans connection was cancelled or refused.",
      });
      return;
    }
    const pending = takeKeyLink();
    if (!pending) return;
    redeeming.current = true;
    void finish(pending.state, pending.code);
  }, [finish]);

  return busy;
}
