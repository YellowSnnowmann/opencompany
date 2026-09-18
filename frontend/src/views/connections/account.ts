// The decisions the Account page makes, none of which needs React, a host or a
// browser to exercise.
//
// The organising rule is the inference rework's (`docs/modules/inference/
// architecture.md`): what the page *decides* — which tier answers, whether
// there is a key of this company's own to remove, what the balance line says —
// lives here with a unit test each, and what is left in the component is
// layout. A decision written inline in JSX can only be checked by mounting the
// page, and the ones below are exactly the ones that have been got wrong.

import type { CompanyBilling, CompanyCredentialStatus } from "@/api/credential";

/** The name the one row carries, and the two letters its mark is drawn from. */
export const ACCOUNT_LABEL = "TinyHumans";

/**
 * How much the page knows about the account right now.
 *
 * `error` is the host refusing to answer — a secret store it could not read —
 * and it is deliberately a value of its own rather than folded into "nothing is
 * set". See {@link accountShape}.
 */
export type AccountLoad = "loading" | "ready" | "error";

/**
 * What the account row is.
 *
 * `unknown` is not `empty`, and that is the distinction the host paid for.
 * `company_key::resolve` propagates a store read error rather than falling
 * through to the instance identity, because "we cannot read the store" and "no
 * key is set" are different answers that call for opposite actions — and a
 * console that renders the first as the second would tell an admin to set a key
 * they have already set. The page has to spend a state on it.
 *
 * `rejected` is the same argument one step further out: a key that is stored is
 * not a key that works, and the two look identical to everything except the one
 * call that presents it. See {@link keyVerdict}.
 */
export type AccountShape = "unknown" | "empty" | "connected" | "rejected";

/**
 * Whether the stored credential actually works, as far as anything on this page
 * has been told.
 *
 * `source` cannot answer this. It reports which credential tier is **stored**,
 * which is a fact about the secret store and not about the account behind it —
 * a revoked key resolves to `company` for as long as its characters sit in the
 * store. The one call on this page that presents the key to the hub is the
 * billing read, so that read is the only evidence there is either way.
 *
 * Both non-`unknown` verdicts are earned, and neither is inferred from the
 * other's absence:
 *
 * - `working` needs a summary to have come back. A key with no failure reported
 *   against it is not a key known to work — the hub may simply not have been
 *   asked, on a host with no hub, or on an older host that says nothing.
 * - `rejected` needs the host to have said `rejected` **and** the tier to be
 *   this company's own. A refusal against a fallback platform identity is not
 *   this company's key to replace, and a row telling its admin to replace one
 *   would send them to rotate a credential they do not hold.
 */
export type KeyVerdict = "unknown" | "working" | "rejected";

/** @see {@link KeyVerdict} */
export function keyVerdict(
  status: CompanyCredentialStatus | null,
  billing: CompanyBilling | null,
): KeyVerdict {
  if (billing?.summary !== undefined) return "working";
  if (status?.source === "company" && billing?.unavailableReason === "rejected") {
    return "rejected";
  }
  return "unknown";
}

/**
 * Which of the four states the row is in.
 *
 * Keyed on `source`, which is what
 * [`resolve`](../../../../src/company/company_key.rs) returned — not on
 * `configured`, which is `key_configured` and answers the narrower question
 * "has this company pasted one". The two differ in exactly the case this page
 * exists to describe: a hosted tenant with no key of its own still has a
 * working identity, and a row built on `configured` would call that
 * "not configured" while the server's account quietly pays for every turn.
 *
 * `source` alone cannot separate a stored key from a working one, which is what
 * `billing` is here for — a third argument rather than an optional one, so that
 * no call site can leave the evidence out and get a confident answer back. The
 * load guards still outrank it: a page that does not know whose account this is
 * has nothing to say about whether the account works.
 */
export function accountShape(
  load: AccountLoad,
  status: CompanyCredentialStatus | null,
  billing: CompanyBilling | null,
): AccountShape {
  if (load !== "ready" || status === null) return "unknown";
  if (status.source === "none") return "empty";
  return keyVerdict(status, billing) === "rejected" ? "rejected" : "connected";
}

/**
 * What the row says where the stored key is this company's own and the hub
 * refused it.
 *
 * "is set" rather than "is connected": the key exists, which is what separates
 * this from the empty state and tells its admin that replacing is the move
 * rather than setting one for the first time. No payer is named, for the reason
 * {@link accountSubline} gives, and the hub's own words are nowhere in it.
 */
export const REJECTED_SUBLINE =
  "This company's key is set, but TinyHumans is refusing it — replace it to reconnect.";

/**
 * The one sub-line the account row shows: which tier actually answers.
 *
 * One fact, not three stacked — an operator is scanning for the row rather than
 * reading it. The two "working" states are kept apart because that is the
 * decision somebody is on this page to make, and a row that says only
 * "connected" hides which account is standing behind the company.
 *
 * **Identity, not a billing verdict.** These lines said "Billed to …", and
 * `source` cannot carry that: it comes from `company_key::resolve`, which
 * reports only which TinyHumans identity won. What an agent's *thinking* costs
 * is decided by `inference/config` and `inference/key`, which are set
 * independently on the LLM page — so a company with `source: "company"` can
 * think on its own OpenRouter key and be billed nothing here, and one on the
 * instance's identity can be paying a provider directly. Naming a payer from
 * this value would point an operator investigating spend at the wrong account,
 * which is the same overclaim this page's pass exists to remove, one row down.
 * The billing move is stated where it is conditional and true: on the header
 * card beside the Connect button, and in {@link REMOVAL_AND_THINKING}.
 *
 * **A verdict outranks the tier.** Where {@link keyVerdict} has evidence the
 * stored key is refused, that is what the row says — an identity claim built on
 * `source` describes a credential that no longer acts as anything.
 */
export function accountSubline(
  load: AccountLoad,
  status: CompanyCredentialStatus | null,
  billing: CompanyBilling | null,
): string {
  if (load !== "ready" || status === null) {
    return "The host could not say — this is not the same as having no key";
  }
  // Before the switch, not after it: `source` answers "which tier is stored"
  // and would otherwise claim this company is acting as its own account while
  // the only call that presented the key came back refused.
  if (keyVerdict(status, billing) === "rejected") return REJECTED_SUBLINE;
  switch (status.source) {
    case "company":
      return "Acting as this company's own TinyHumans account";
    case "attested":
    case "static":
      return "Acting as the account of whoever runs this server";
    case "none":
      // Deliberately narrow. "Agents cannot think" is what this page used to
      // say here, and it is **false** on a company whose LLM page holds a key
      // of its own: that one outranks this credential in the managed chain, and
      // a provider of its own never consults it — so such a company thinks
      // perfectly well with no TinyHumans account at all. What is always true
      // is the absence itself.
      return "No TinyHumans account for this company";
    default:
      // An older or newer host naming a tier this build does not know. Saying
      // what the row *is* beats claiming a state nobody established.
      return "The account this company acts and spends through";
  }
}

/**
 * Whether there is a key of **this company's own** to take away.
 *
 * The instance's platform identity is not this row's to remove, and offering a
 * Remove that would clear nothing is the control-that-cannot-act the LLM page's
 * pass deleted a toggle over. Gated on the resolved tier rather than on
 * `configured` for the reason {@link accountShape} gives.
 */
export function canRemoveKey(status: CompanyCredentialStatus | null): boolean {
  return status?.source === "company";
}

/** What the header card offers. */
export interface HeaderActions {
  /** "Connect to TinyHumans" — the dialog that takes an API key. */
  key: boolean;
}

/**
 * Whether the header card shows its one option, "Connect to TinyHumans".
 *
 * The API-key dialog works on every host. The "Sign in with TinyHumans" grant
 * option was removed from this page at the operator's request (2026-09-14).
 *
 * Not offered once this company has a key of its own. The connected row
 * carries Replace and Remove, and a Connect button above a row that already
 * says "connected" is a second path to the same write, asked for by nobody.
 *
 * This decides what is **shown**. A returning grant is still redeemed by
 * `useRedeemKeyGrant`, which `ApiKeyView` calls whatever this says.
 */
export function headerActions(
  status: CompanyCredentialStatus | null,
  canManage: boolean,
): HeaderActions {
  // No status is not "no key". `refresh` drops `status` to `null` both while
  // the read is in flight and when it fails, and in the failed case the row
  // beneath these buttons is already saying the host could not answer. Offering
  // a key field under that sentence opens a blind overwrite of a write-only
  // credential the console has just admitted it cannot see, and the value it
  // replaces cannot be read back from anywhere.
  if (!canManage || status === null || canRemoveKey(status)) {
    return { key: false };
  }
  return { key: true };
}

/**
 * What removing this company's key actually does — first paragraph of the
 * confirmation.
 *
 * It offers **both** fallbacks rather than picking one, because the console
 * genuinely cannot tell which applies: `GET …/credential` reports the tier that
 * *won*, and while this company's own key is set that is always `company`,
 * whether or not an instance identity sits behind it.
 *
 * What it no longer claims is that the billing stops — that half is
 * {@link REMOVAL_AND_THINKING}'s, and it is not a flat "it stops" either.
 * `set_key("")` clears `tinyhumans/key` and nothing else; what happens to the
 * turns depends on what the managed chain finds next
 * (`src/server/ops/company_key.rs`, `src/company/inference.rs`).
 */
export const REMOVAL_CONSEQUENCE =
  "Apps connected as this company stop being reachable, and the identity the platform presents " +
  "on its behalf is gone. The company falls back to the identity of whoever runs this server, if " +
  "this instance carries one — and to no account at all if it does not.";

/**
 * What the removal costs the company's *thinking*, said before the press rather
 * than found when its agents stop answering.
 *
 * This sentence has been wrong in both directions, and #2266 is why. The grant
 * used to copy the key into `inference/key` as well, so removal here genuinely
 * did leave a second copy thinking on the same account. It no longer copies:
 * the key lands only in `tinyhumans/key`, and a managed turn *resolves* through
 * it — `provider/tinyhumans/key`, then the legacy `inference/key`, then this,
 * then the instance identity, then nothing. So removing it takes the rung a
 * managed company was standing on, and the honest sentence is the fallback,
 * not a reassurance.
 *
 * Still conditional at the top: a TinyHumans key pasted on the LLM page
 * outranks this one and goes on working, which is the state an operator most
 * needs to be able to tell from the others.
 */
export const REMOVAL_AND_THINKING =
  "Thinking goes with it where this company's models are set to TinyHumans: those turns resolve " +
  "through this same key, so removing it falls back to the identity of whoever runs this server " +
  "— and to nothing at all if this instance carries none. A TinyHumans key set on the LLM page " +
  "outranks this one and keeps working.";

/** The balance row, once there is an account of this company's own to ask about. */
export interface BalanceLine {
  /** The figure as drawn, or `null` when the hub would not answer. */
  amount: string | null;
  /** The one sub-line: the plan, or why there are no figures. */
  detail: string;
  /** Whether the figure should read as a warning rather than a fact. */
  low: boolean;
}

/**
 * What the balance row says, or `null` for no row at all.
 *
 * No row unless the company has an account of its own: `$0.00` under a company
 * that never set a key would be a made-up fact about a wallet that does not
 * exist. The host keys `configured` here off `load` rather than `resolve` for
 * the same reason — the instance's balance is not this company's to show.
 *
 * `unavailable` is **not** a zero balance. They look identical on a row and
 * mean opposite things, one "top up" and one "try again", so the figure is
 * dropped rather than invented.
 */
export function balanceLine(billing: CompanyBilling | null): BalanceLine | null {
  if (billing?.configured !== true) return null;

  if (billing.unavailable !== undefined) {
    return { amount: null, detail: unavailableDetail(billing.unavailableReason), low: false };
  }

  const summary = billing.summary;
  // Zero is a number worth showing, so the empty test is on `null` and never on
  // falsiness — `!balanceUsd` would hide exactly the figure somebody needs.
  const usd = typeof summary?.balanceUsd === "number" ? summary.balanceUsd : null;
  return {
    amount: usd === null ? "—" : `$${usd.toFixed(2)}`,
    detail: `on the ${summary?.plan ?? "free"} plan${
      summary?.activeSubscription ? " · subscription active" : ""
    }`,
    low: usd !== null && usd <= 0,
  };
}

/**
 * The one line under "Balance unknown", chosen from the host's classification.
 *
 * Every sentence is written here. Nothing the hub returned is interpolated: its
 * failure bodies are JSON meant for a log, they can carry anything, and a card
 * that renders one puts a machine's punctuation under somebody's balance while
 * telling them nothing they can act on.
 *
 * An absent reason is an older host that did not classify the failure, and it
 * gets the cautious sentence — the same one an unclassifiable failure gets.
 * "Said nothing" is not evidence of any particular cause.
 */
function unavailableDetail(reason: CompanyBilling["unavailableReason"]): string {
  switch (reason) {
    case "rejected":
      return "The key is set, but TinyHumans refused it — replace it to reconnect.";
    case "unreachable":
      return "The key is set; TinyHumans could not be reached just now.";
    case "noHub":
      return "The key is set; this host is not part of a TinyHumans ecosystem.";
    default:
      return "The key is set; the balance could not be read just now.";
  }
}
