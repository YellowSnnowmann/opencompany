import { EllipsisVertical, Plus, RefreshCw } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { Monogram } from "./AddProviderDialog";
import { cn } from "@/lib/utils";
import { categoryOf, endpointHost } from "./catalogue";
import { defaultBadgeLabel, isFullDefault, providerMenu, rowNeedsModel } from "./connect";
import { healthLabel, testOutcome } from "./classify";
import type { TestState } from "./classify";
import type { ManagedState } from "@/api/inference";
import type { DefaultChoice, Provider, ProviderHealth } from "./types";

/**
 * Whether the separate legacy Managed row renders (keys rework, issue #2306,
 * slice 2a, decision Q3: exactly one TinyHumans row is ever shown).
 *
 * `false` once `providers` already lists a `tinyhumans` row — an added row, or
 * entry zero on a managed config — because that row is then the one TinyHumans
 * row this page shows. `legacyRow` absent (an older host) reads as
 * `configured`, which is what every host did before this rework.
 *
 * @deprecated keys-rework #2306: the legacy row this gates is transitional —
 * it exists only for a company whose managed chain resolves through the
 * company account or the instance identity with **no** indexed row yet.
 * Removable once every such company has an ordinary `tinyhumans` row (item 10,
 * dropping the fallback chains, is the thing that would make that universal —
 * see `docs/key-reworks/not-handled.md`).
 */
export function showsLegacyManagedRow(managed: ManagedState | undefined): boolean {
  return managed?.configured === true && managed.legacyRow !== false;
}

/**
 * The one sub-line the managed row shows, given what its chain resolves to.
 *
 * It used to say **"Always on"**, inherited from a design where the same company
 * runs the managed backend. Here the managed tier needs a credential and can
 * resolve to nothing, so that badge was a claim of availability the row could
 * not back — the failure a five-state cognition model exists to prevent.
 *
 * The two "on" states that bill different accounts are kept apart, because that
 * is the decision the operator is on this page to make: connecting their own
 * account moves the bill for every turn, and a row that says only "on" hides
 * that it has not happened.
 *
 * @deprecated keys-rework #2306: describes the legacy row only — see
 * {@link showsLegacyManagedRow}. An ordinary `tinyhumans` row uses
 * {@link rowSubline} like every other provider.
 */
export function managedRow(source: ManagedState["source"] | undefined): string {
  switch (source) {
    case "provider_key":
      return "Using the key saved for inference";
    case "company_account":
      return "Billed to this company's TinyHumans account";
    case "instance":
      return "Billed to whoever runs this server";
    case "none":
      return NO_CREDENTIAL_RESOLVES;
    // An older host did not say, and "unknown" is not "working" — so this says
    // what managed is rather than claiming a state nobody established.
    default:
      return "TinyHumans chooses a model for each task";
  }
}

/**
 * The legacy row's own sub-line, given what the host said (round-2 review,
 * P1-3, decision X5): "Key added — choose a model" whenever
 * `managed.needsModel` is set, never {@link managedRow}'s connected-sounding
 * text — a chain with no chosen model is not "connected", the same rule an
 * ordinary row's own {@link rowSubline} applies to `model`/`modelAmbiguous`.
 *
 * @deprecated keys-rework #2306: describes the legacy row only.
 */
export function legacyManagedSubline(managed: Pick<ManagedState, "source" | "needsModel">): string {
  return managed.needsModel ? "Key added — choose a model" : managedRow(managed.source);
}

/**
 * Whether the legacy row shows its live on/off switch, or the "Needs a
 * model" affordance instead (round-2 review, P1-3). There is no live switch
 * while there is no model to serve a turn with — switching a chain with
 * nothing behind it "on" would be a control that cannot do what it claims.
 *
 * @deprecated keys-rework #2306: describes the legacy row only.
 */
export function legacyManagedShowsSwitch(managed: Pick<ManagedState, "needsModel">): boolean {
  return !managed.needsModel;
}

/**
 * The sentence for a company nothing can answer for.
 *
 * It is the best sentence on this whole surface and **it could not render**: the
 * managed row is gated on `managed.configured`, the host derives that as
 * `source.resolves()`, so `source === "none"` implies no row — and the one state
 * that needed this sentence was the one state that could never show it. It is a
 * constant now so the dead-end states that *are* reachable can say it — the
 * empty list below is the one still standing; the Routing tab's own unset
 * banner this comment used to also name is gone with the tab (phase 5b).
 */
export const NO_CREDENTIAL_RESOLVES = "No credential resolves — agents cannot think";

/**
 * The legacy row's name.
 *
 * @deprecated keys-rework #2306: "Managed" is the legacy pre-row identity —
 * deliberately distinct from "TinyHumans", the ordinary catalogue row this
 * row's own credential moves into the moment one exists
 * ({@link showsLegacyManagedRow}). Not a stray leftover string to rename.
 */
export const MANAGED_LABEL = "Managed";

/** The slug its credential and its health are keyed on. */
export const MANAGED_SLUG = "tinyhumans";

/**
 * Check this provider, and say so in place.
 *
 * **On the row, not in the overflow menu**, because the answer belongs to the
 * row: with two providers connected, a result rendered under the card says
 * nothing about which one was tested. Moving the control is what fixes the
 * attribution; putting the answer beside it is the point of moving it.
 *
 * Not gated on `canManage`. The host leaves this route on `ScopedCompany`
 * deliberately — it probes what is already stored and names no destination of
 * its own — so a member may ask, and the console must not offer less than the
 * host allows.
 *
 * The result is in an `aria-live` region. It clears itself after ten seconds,
 * and a result that disappears is invisible to a screen reader unless it is
 * announced when it arrives.
 */
function TestControl({
  label,
  slug,
  state,
  onTest,
}: {
  label: string;
  slug: string;
  state: TestState;
  onTest: () => void;
}) {
  const outcome = testOutcome(state);
  return (
    <>
      {/* Polite, and always present rather than mounted with the result — a
          region that appears at the same moment as its text is frequently
          missed by the announcement. */}
      <span
        aria-live="polite"
        className={cn(
          "truncate text-xs",
          outcome?.tone === "ok" && "text-status-done-text",
          outcome?.tone === "error" && "text-status-blocked-text",
        )}
        data-testid={`inference-provider-${slug}-test-result`}
      >
        {outcome?.message ?? ""}
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        disabled={state.kind === "testing"}
        // Names the provider, so a screen reader hears which of several rows
        // this button belongs to.
        aria-label={`Test ${label}`}
        // The cost warning lives on the control it applies to, not above the
        // fold: this sends one real request and a provider may charge for it.
        title={`Test ${label}. Sends one real request; your provider may charge for it.`}
        data-testid={`inference-provider-${slug}-test`}
        onClick={onTest}
      >
        <RefreshCw className={cn("size-4", state.kind === "testing" && "animate-spin")} />
      </Button>
    </>
  );
}

/**
 * The Connected list: what this company can reach a model through.
 *
 * One row per provider, and each row is **a mark, a name, one sub-line and a
 * control**. Nothing else. The page this replaces carried several paragraphs
 * explaining what bring-your-own-key meant, what Test cost and what Reset did;
 * almost all of it said what the control beside it already said.
 *
 * ## Managed is a badge, not a disabled toggle
 *
 * A locked switch reads as switchable-but-broken and invites a fight the
 * operator cannot win. A badge says the same thing and is honest about it.
 *
 * ## No decisions live here
 *
 * Which sub-line a row gets, what a health state is called, whether a category
 * carries a slug — all of it is a function in this file's pure neighbours or in
 * `rowSubline` below, each with a unit test. What is left is layout.
 */
export function ProviderList({
  providers,
  managed,
  defaultChoice,
  canManage,
  busySlug,
  onToggle,
  onEdit,
  onChooseModel,
  onTest,
  onRemove,
  onRemoveKey,
  onReplaceKey,
  onMakeDefault,
  onAdd,
  onManagedToggle,
  onManagedTest,
  onManagedReplaceKey,
  testState,
}: {
  providers: readonly Provider[];
  /** What the managed chain resolves to. `undefined` when the host did not say. */
  managed?: ManagedState;
  /** The stored company default, so rows can show "Default · <model>" and "Needs a model". */
  defaultChoice?: DefaultChoice | null;
  canManage: boolean;
  /** The slug currently mid-request, so its own controls settle rather than the whole list. */
  busySlug?: string | null;
  onToggle: (provider: Provider, enabled: boolean) => void;
  onEdit: (provider: Provider) => void;
  /**
   * The row's own "Needs a model" chip (round-2 review, P1-7c) — not the same
   * as `onEdit`, because entry zero and the row a bare-slug default names
   * cannot be reached through the ordinary edit dialog at all (entry zero's
   * `providerMenu` offers no "Edit" for exactly this reason: the host refuses
   * it with a 400, "changed through the inference config"). The caller routes
   * those two cases to "Set as default" instead.
   */
  onChooseModel: (provider: Provider) => void;
  onTest: (provider: Provider) => void;
  onRemove: (provider: Provider) => void;
  onRemoveKey: (provider: Provider) => void;
  onReplaceKey: (provider: Provider) => void;
  onMakeDefault: (provider: Provider) => void;
  /**
   * The same action the header card's button performs, passed in rather than
   * reimplemented — one way to add a provider, not two that can drift.
   */
  onAdd: () => void;
  /** Switch the legacy managed row on or off — never its credential. */
  onManagedToggle: (enabled: boolean) => void;
  onManagedTest: () => void;
  /** Open the managed key dialog, to add or replace step 1 of its chain. */
  onManagedReplaceKey: () => void;
  /** What each row's Test is doing, keyed by slug. */
  testState: (slug: string) => TestState;
}) {
  // Nothing connected at all: no records, and no managed chain behind them. The
  // card would otherwise be a heading over blank space, which reads as a page
  // that failed to load rather than a company that has not started.
  if (providers.length === 0 && !managed?.configured) {
    return (
      <div
        className="flex flex-col items-start gap-3 px-4 py-6"
        data-testid="inference-providers-empty"
      >
        <p className="text-sm">
          <span className="font-medium">No providers connected yet.</span>{" "}
          <span className="text-muted-foreground">Connect one to get started.</span>
        </p>
        {/* The state A2 is actually in, said rather than implied. Managed does
            not resolve (that is the condition for this branch), so this company
            has no way to think at all — which is a stronger statement than "not
            connected yet" and is the one that makes Add the obvious next step. */}
        <p className="text-xs text-status-blocked-text" data-testid="inference-providers-dead-end">
          {NO_CREDENTIAL_RESOLVES}.
        </p>
        <Button type="button" disabled={!canManage} onClick={onAdd}>
          <Plus className="size-4" />
          Add a provider
        </Button>
      </div>
    );
  }

  return (
    <ul className="divide-y divide-border" data-testid="inference-providers">
      {/* Always first and always present. It is not in `providers` because it is
          not a record — it is the fallback every company has whether or not it
          has configured anything. */}
      {/* Present only for the transitional case: the legacy chain actually
          resolves (company account or instance identity) and no `tinyhumans`
          row exists yet (`showsLegacyManagedRow` — keys rework, issue #2306,
          decision Q3: exactly one TinyHumans row is ever shown). Once a row
          exists it appears below like any other provider and this one is
          gone. @deprecated keys-rework #2306: see `showsLegacyManagedRow`. */}
      {managed && showsLegacyManagedRow(managed) && (
        <li className="flex items-center gap-3 px-4 py-3" data-testid="inference-provider-managed">
          <Monogram label={MANAGED_LABEL} />
          <span className="grid min-w-0 flex-1 leading-tight">
            <span className="truncate text-sm font-medium">{MANAGED_LABEL}</span>
            <span className="truncate text-xs text-muted-foreground">
              {legacyManagedSubline(managed)}
            </span>
          </span>

          <Health slug={MANAGED_SLUG} health={managed.health} />

          <TestControl
            label={MANAGED_LABEL}
            slug={MANAGED_SLUG}
            state={testState(MANAGED_SLUG)}
            onTest={onManagedTest}
          />

          {!legacyManagedShowsSwitch(managed) ? (
            // No live switch while there is no model to serve a turn with
            // (round-2 review, P1-3) — the same "Needs a model" affordance an
            // ordinary row shows, opening the ordinary TinyHumans add flow
            // rather than the deprecated key route directly (X6).
            canManage ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-6 border-status-blocked px-2 text-xs text-status-blocked-text"
                data-testid="inference-provider-managed-needs-model"
                onClick={onManagedReplaceKey}
              >
                Needs a model
              </Button>
            ) : (
              <Badge
                variant="outline"
                className="border-status-blocked text-status-blocked-text"
                data-testid="inference-provider-managed-needs-model"
              >
                Needs a model
              </Badge>
            )
          ) : (
            // @deprecated keys-rework #2306, decision X3: once a `tinyhumans`
            // row exists this whole li is gone and the row's own ordinary
            // toggle (below, on every provider) is the one and only switch —
            // this one exists solely for the pre-row transitional state, and
            // `ProvidersTab` confirms before calling `onManagedToggle`, same as
            // every other provider's toggle.
            <Switch
              checked={managed.enabled !== false}
              disabled={!canManage || busySlug === MANAGED_SLUG}
              aria-label="Managed enabled"
              data-testid="inference-provider-managed-toggle"
              onCheckedChange={(next) => onManagedToggle(next)}
            />
          )}

          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  disabled={!canManage || busySlug === MANAGED_SLUG}
                  aria-label="Managed actions"
                  data-testid="inference-provider-managed-menu"
                >
                  <EllipsisVertical className="size-4" />
                </Button>
              }
            />
            <DropdownMenuContent align="end">
              {/* No Test here. One affordance per action — the icon button on
                  the row is discoverable and its answer lands where it belongs.
                  Decision X6: "Add a key"/"Replace key" here open the ordinary
                  TinyHumans add flow (`onManagedReplaceKey` → the same catalogue
                  row every other provider connects through), never the deprecated
                  `PUT …/inference/managed/key` route directly — the console no
                  longer calls it to add or replace a key. */}
              {/* Round-2 review, P1-4: the legacy "Remove key" item is gone —
                  it was the one remaining console call to the deprecated
                  `PUT …/inference/managed/key` route (decision X6 says never),
                  and P1-3 already turns this row into "Needs a model" the
                  moment its key has nothing chosen yet, which is every state
                  this row renders in at all. There is no longer a state where
                  removing the key without the ordinary add flow is the useful
                  action here. */}
              <DropdownMenuItem onClick={onManagedReplaceKey}>
                {managed.source === "provider_key" ? "Replace key" : "Add a key"}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </li>
      )}

      {providers.map((provider) => (
        <ProviderRow
          key={provider.id}
          provider={provider}
          defaultChoice={defaultChoice}
          canManage={canManage}
          busy={busySlug === provider.slug}
          onToggle={onToggle}
          onEdit={onEdit}
          onChooseModel={onChooseModel}
          onTest={onTest}
          onRemove={onRemove}
          onRemoveKey={onRemoveKey}
          onReplaceKey={onReplaceKey}
          onMakeDefault={onMakeDefault}
          testState={testState}
        />
      ))}
    </ul>
  );
}

/**
 * The one sub-line a row gets.
 *
 * One fact, chosen by what the row is: a keyed provider is identified by the
 * fact that it holds a key, a local runtime by where it runs, a CLI login by
 * whose credential it borrows, and a keyless cloud endpoint by its host. Three
 * facts stacked would be a table, and an operator is scanning for the row rather
 * than reading it.
 */
export function rowSubline(provider: Provider): string {
  // **The split that is the most confusing thing in this area, named.** Entry
  // zero is the company's own `inference/config` blob surfaced as a list row: it
  // serves turns, it cannot be edited or removed from here, and nothing on the
  // row said so.
  if (provider.origin === "entryZero") return "This company's original configuration";
  const category = categoryOf(provider.kind);
  // Decision X5 (orchestrator, 2026-09-15): no category's row is ever shown as
  // set without a model — never claim health or readiness a turn cannot
  // actually get. Checked **before** the category sub-lines (round-2 review,
  // P2-4: a local runtime or a keyless cloud row used to return early here,
  // so "Runs on this machine" and a bare endpoint host both claimed "set" for
  // a row with no model chosen yet). `modelAmbiguous` gets its own chip
  // instead of this line, because "Needs a model" there already says it and
  // doing both would repeat it.
  if (!provider.model && !provider.modelAmbiguous) {
    if (category === "local") return "Runs on this machine — choose a model";
    if (category === "cli") return "Uses a login another CLI already holds — choose a model";
    if (provider.keyConfigured) return "Key added — choose a model";
    return "Choose a model";
  }
  if (category === "local") return "Runs on this machine";
  if (category === "cli") return "Uses a login another CLI already holds";
  if (provider.keyConfigured) return "•••• configured";
  return endpointHost(provider.baseUrl) || "no key";
}

function ProviderRow({
  provider,
  defaultChoice,
  canManage,
  busy,
  onToggle,
  onEdit,
  onChooseModel,
  onTest,
  onRemove,
  onRemoveKey,
  onReplaceKey,
  onMakeDefault,
  testState,
}: {
  provider: Provider;
  defaultChoice?: DefaultChoice | null;
  canManage: boolean;
  busy: boolean;
  onToggle: (provider: Provider, enabled: boolean) => void;
  onEdit: (provider: Provider) => void;
  onChooseModel: (provider: Provider) => void;
  onTest: (provider: Provider) => void;
  onRemove: (provider: Provider) => void;
  onRemoveKey: (provider: Provider) => void;
  onReplaceKey: (provider: Provider) => void;
  onMakeDefault: (provider: Provider) => void;
  testState: (slug: string) => TestState;
}) {
  // **Entry zero has nowhere to store an `enabled` flag**, so it is always on and
  // the host refuses to switch it off — "cannot be switched off from the list;
  // reset the inference config instead". The console could not tell which row
  // that was, so it rendered a live switch whose only outcome was a 400.
  const entryZero = provider.origin === "entryZero";
  // The menu's own "Set as default" gates on `isDefault`, but that field is
  // the host's resolved *fallback* answer — a company that has never chosen
  // reports its first enabled row here too. Passing `isFullDefault` instead
  // means a row that IS the resolved fallback but has no model yet still
  // offers the action, so an operator can give it one.
  const menu = providerMenu({ ...provider, isDefault: isFullDefault(provider, defaultChoice) });
  return (
    <li
      className="flex items-center gap-3 px-4 py-3"
      data-testid={`inference-provider-${provider.slug}`}
    >
      <Monogram label={provider.label} slug={provider.slug} />
      <span className="grid min-w-0 flex-1 leading-tight">
        <span className="truncate text-sm font-medium">{provider.label}</span>
        <span className="truncate text-xs text-muted-foreground">{rowSubline(provider)}</span>
      </span>

      {/* A word plus its model, not a sentence — "Default · acme/test-model",
          or plain "Default" for a resolved-but-unmarked fallback with nothing
          stored yet. What a default is, is not something this page has to
          explain; moving it is a menu item. */}
      {provider.isDefault && (
        <Badge
          variant="secondary"
          className="max-w-48 truncate"
          data-testid={`inference-provider-${provider.slug}-default`}
        >
          {defaultBadgeLabel(provider, defaultChoice)}
        </Badge>
      )}

      {/* This row's own model is ambiguous (two or more distinct stored ids,
          never guessed), or it is the provider a bare-slug default names with
          no model chosen yet. Either way the row cannot serve a turn until an
          admin picks one — named here rather than left to `rowSubline` alone,
          which one fixed testid could not since several rows can need this at
          once. */}
      {rowNeedsModel(provider, defaultChoice) &&
        (canManage ? (
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-6 border-status-blocked px-2 text-xs text-status-blocked-text"
            data-testid={`inference-provider-${provider.slug}-needs-model`}
            onClick={() => onChooseModel(provider)}
          >
            Needs a model
          </Button>
        ) : (
          <Badge
            variant="outline"
            className="border-status-blocked text-status-blocked-text"
            data-testid={`inference-provider-${provider.slug}-needs-model`}
          >
            Needs a model
          </Badge>
        ))}

      <Health slug={provider.slug} health={provider.health} />

      <TestControl
        label={provider.label}
        slug={provider.slug}
        state={testState(provider.slug)}
        onTest={() => onTest(provider)}
      />

      <Switch
        checked={provider.enabled}
        disabled={!canManage || busy || entryZero}
        title={
          entryZero
            ? "This company's original provider is changed through its inference config, not from this list."
            : undefined
        }
        aria-label={`${provider.label} enabled`}
        data-testid={`inference-provider-${provider.slug}-toggle`}
        onCheckedChange={(next) => onToggle(provider, next)}
      />

      {/* No trigger at all when there is nothing behind it. An overflow button
          that opens an empty menu is a control that reports a capability the row
          does not have — which is what entry zero had, three times over. */}
      {menu.length > 0 && (
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              variant="ghost"
              size="icon"
              disabled={!canManage || busy}
              aria-label={`${provider.label} actions`}
              data-testid={`inference-provider-${provider.slug}-menu`}
            >
              <EllipsisVertical className="size-4" />
            </Button>
          }
        />
        {/* Which items, decided in `providerMenu` — per kind, and derived from
            the same `credentialAsk` the connect dialog uses, so a local runtime
            or a CLI login is never offered a key it does not have. */}
        <DropdownMenuContent align="end">
          {menu.map((action) => (
            <DropdownMenuItem
              key={action.id}
              variant={action.destructive ? "destructive" : undefined}
              data-testid={`inference-provider-${provider.slug}-${action.id}`}
              onClick={() => {
                switch (action.id) {
                  case "edit":
                    return onEdit(provider);
                  case "setDefault":
                    return onMakeDefault(provider);
                  case "replaceKey":
                    return onReplaceKey(provider);
                  case "removeKey":
                    return onRemoveKey(provider);
                  case "remove":
                    return onRemove(provider);
                }
              }}
            >
              {action.label}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      )}
    </li>
  );
}

/**
 * What was last learnt about reaching this provider.
 *
 * **Silent when nothing has been learnt**, which is honest: a row that has never
 * been checked is not a row that is working, and a green tick by default is the
 * state the design this is ported from is in — where a provider whose key was
 * revoked an hour ago looks identical to one that works.
 *
 * Silent when it is `ok`, too, and that is the deletion pass applied to a status
 * column: a list where every healthy row says "ok" spends a column saying
 * nothing, and the one row that is not healthy is harder to find for it.
 */
function Health({ slug, health }: { slug: string; health?: ProviderHealth }) {
  if (!health || health.state === "ok") return null;
  return (
    <span
      className="truncate text-xs text-status-blocked-text"
      data-testid={`inference-provider-${slug}-health`}
    >
      {healthLabel(health.state)}
    </span>
  );
}
