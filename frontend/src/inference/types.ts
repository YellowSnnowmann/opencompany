// The shapes the inference surfaces pass around.
//
// Keys rework (issue #2306): a provider is a provider plus one chosen model —
// no more per-tier map presented to the operator, and no tier name ever shown
// as a model. `model` / `modelAmbiguous` / `DefaultChoice` are the wire fields
// the shared naming contract (`docs/key-reworks/README.md`) fixes; do not
// invent synonyms for them.
//
// Types only — no values, no behaviour. Kept apart from `routing.ts` and
// `classify.ts` so those stay readable as *decisions*, which is the whole reason
// they are separate from the components that render them.
//
// The credential rule, restated here because this is where someone would add a
// field to one of these: **no shape in this file carries a key.** The wire
// carries `keyConfigured: boolean` and nothing else. Four independent mechanisms
// keep credentials off the wire in this subsystem, and the easiest way to break
// all four at once is to put one on a record for convenience.
//
// `baseUrl` is the field that made that sentence briefly untrue. A URL can carry
// userinfo (`http://user:password@host/v1`), and this shape comes back from a
// `ScopedCompany` route every console reader can call. The host now refuses such
// an endpoint everywhere one can be set and redacts it everywhere one is said
// (`catalogue::endpoint_has_credentials` / `redact_endpoint`), so what arrives
// here is `http://***@host/v1` at worst — but a `baseUrl` is still a place a
// credential can hide, which is why it is called out rather than trusted.

import type { UsedBy } from "@/api/types";

/** How a provider expects its credential presented. */
export type AuthStyle = "bearer" | "anthropic" | "none";

/** Which of the three questions a provider answers. */
export type ProviderCategory = "cloud" | "local" | "cli";

/**
 * One configured way for this company to reach a model.
 *
 * `id` is identity and survives a rename; `slug` is the address an operator
 * reads and hand-edits in a routing entry. They are two fields because they
 * answer two questions.
 */
export interface Provider {
  /** Stable, opaque. Never shown. */
  id: string;
  /** Routing key. Unique per company. What a routing entry names. */
  slug: string;
  /** Display label. Never used in routing. */
  label: string;
  /** Provider kind — a catalogue slug, or a legacy manifest kind. */
  kind: string;
  /** Resolved OpenAI-compatible base URL. */
  baseUrl: string;
  /**
   * Abstract tier → concrete model id.
   *
   * @deprecated keys-rework #2306: a storage encoding only — the same id is
   * written under all four tier keys, never a per-tier selection. The console
   * reads and writes {@link Provider.model} instead; removable once nothing
   * renders this map directly.
   */
  models: Record<string, string>;
  /**
   * This row's one model, collapsed from the stored tier map (`ModelOnRow` on
   * the host, keys rework #2306). `null` when the row holds no model yet, or
   * when it holds more than one distinct id (see {@link modelAmbiguous}) —
   * never guessed. Optional because an older host does not send it.
   */
  model?: string | null;
  /**
   * Whether the row's stored tier map holds two or more distinct ids, so no
   * single model can be shown. The console never picks one on the row's
   * behalf; it shows "Needs a model" and asks. Optional because an older host
   * does not send it, and absent reads as `false`.
   */
  modelAmbiguous?: boolean;
  /** Whether this is available for routing. Distinct from deleted. */
  enabled: boolean;
  /** Whether a credential is stored — **never the credential**. */
  keyConfigured: boolean;
  /**
   * Whether an **unset** workload goes through this one.
   *
   * The resolved answer rather than the raw marker: a company that has never
   * said which provider is its default reports its first enabled one here,
   * because that is what it has always resolved to. So a row can say "Default"
   * without the console knowing whether it was chosen or inherited — and the
   * operator sees the same answer either way.
   *
   * Optional because an older host does not send it.
   */
  isDefault?: boolean;
  /**
   * Which slot this record lives in.
   *
   * `entryZero` is the pre-list company's single `inference/config` blob,
   * surfaced as element 0 of the list. It refuses edit, remove and disable with
   * three separate 400s — correct rules, and the console could not tell which row
   * they applied to, so it rendered all three controls live and every one of them
   * was a round trip to a refusal.
   *
   * Optional because an older host does not send it; absent reads as `indexed`,
   * which is what every row was treated as before.
   */
  origin?: "entryZero" | "indexed";
  /** The last thing the system learnt about reaching it, if anything. */
  health?: ProviderHealth;
  /**
   * What else depends on this row (keys rework, issue #2306): the company
   * default, agents pinned to it, other surfaces sharing its credential.
   * Absent means nothing does — the same as every field being omitted. Drives
   * the in-use confirmation on remove / clear key / disable; see
   * `docs/key-reworks/README.md`'s confirmation contract.
   */
  usedBy?: UsedBy;
}

/**
 * The company's stored `{provider, model}` default, as the status reports it
 * (keys rework, issue #2306; `store::DefaultChoice` on the host).
 *
 * `model: null` is a **bare-slug default from before this rework**: a provider
 * is named and no model is, which resolves through the legacy source and shows
 * the "choose one" banner. It is never rewritten by anything but an explicit
 * set-default. `null` for the whole field means no default is stored at all.
 */
export interface DefaultChoice {
  provider: string;
  model: string | null;
}

/**
 * What the system last learnt about reaching a provider.
 *
 * Sourced from things that already happen — the add-time probe, the manual
 * Test, and the turn path's own 401 — rather than from a poller. A poller costs
 * a request per provider per interval across every company on the host, to learn
 * something the next real turn learns for free.
 */
export interface ProviderHealth {
  /** `ok`, or the probe class of the last failure. */
  state: "ok" | ProbeClass;
  /** When it was learnt, ISO-8601. */
  at: string;
}

/** What a failed check means. See `classify.ts` for the copy each one gets. */
export type ProbeClass = "auth" | "model" | "quota" | "endpoint" | "timeout" | "unknown";
