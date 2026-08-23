import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { ChevronRight, Mail, Pencil, Sparkles, Users, Wallet, Wrench } from "lucide-react";
import { toast } from "sonner";

import { listPeople, me as fetchMe, type Person } from "@/api/auth";
import type { OpenCompanyClient } from "@/api/client";
import { setInboxEnabled } from "@/api/inbox";
import { listTasks } from "@/api/tasks";
import { ApiError, type AgentDetailDto } from "@/api/types";
import { TeammateAvatar } from "@/components/teammate-avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import {
  agentEdits,
  draftFrom,
  draftIsValid,
  emptyDraft,
  isEditable,
  summarizeGrants,
  tierLabel,
  type AgentDraft,
  type AgentFieldKey,
} from "@/lib/agent";
import { fetchBoardColumns } from "@/lib/board-columns";
import { avatarFor, roleSubtitle, toneFor } from "@/lib/team";
import { workloadByAssignee, type Workload } from "@/lib/team-workload";
import { cn } from "@/lib/utils";
import { AgentFields } from "@/views/team/AgentFields";

type Load = "loading" | "ready" | "missing" | "unsupported" | "error";

/**
 * Why a detail read failed, in the operator's terms rather than the wire's.
 *
 * A `404` from `GET …/team/{agentId}` is **two different facts**, and the status
 * cannot tell them apart: a host that predates this route 404s the path it does
 * not serve, and a host that serves it 404s a teammate that is gone. Saying "no
 * such teammate" to the first sends an operator looking for a deletion that
 * never happened; saying "this host is too old" to the second hides a real
 * removal behind a version complaint.
 *
 * The roster settles it, but only if the right question is asked. "Did `GET
 * …/team` answer?" is not enough — the roster route is the *older* one, so an
 * out-of-date host answers it perfectly. The question that separates the two is
 * whether the roster still **contains this agent**:
 *
 * | `GET …/team` | outcome |
 * |---|---|
 * | lists this agent | the host has the roster but not the detail route → `unsupported` |
 * | omits this agent | the host serves both and the teammate is gone → `missing` |
 * | fails too | nothing is reachable; do not guess → `error` |
 *
 * Anything that is not a `404` — a transport failure, a `500` — is `error`. It
 * used to fall into `unsupported`, which told an operator their host was too old
 * when their network had simply dropped.
 */
async function classifyFailure(
  error: unknown,
  roster: () => Promise<{ id: string }[]>,
  agentId: string,
): Promise<Exclude<Load, "loading" | "ready">> {
  if (!(error instanceof ApiError) || error.status !== 404) return "error";
  const members = await roster().catch(() => null);
  if (members === null) return "error";
  return members.some((member) => member.id === agentId) ? "unsupported" : "missing";
}

/**
 * One agent, opened (issue #264).
 *
 * Before this the Team card was a dead end: a name, a role, and a destructive
 * Remove. None of what an agent *is* was reachable once it existed, so an
 * operator could not read the instructions it was defined with, could not see
 * which tools it may use or which desks it belongs to, and could not change any
 * of it. This is the screen that answers those questions, and edits the ones
 * the host says are editable.
 *
 * ## Read-only is a fact about the agent, not a state of this screen
 *
 * A **manifest** teammate is declared in the company's version-controlled
 * `company.toml` and is edited here too: the host stores the change as an
 * override on the company record rather than rewriting the blueprint, so a
 * deployed company's own roster — including the global baseline every company
 * gets — is the operator's to change without a redeploy they may not be able to
 * make. An **overlay** teammate was added here and is edited here.
 *
 * Which is which comes from the host's own `editable` list rather than from a
 * rule this file re-implements. A console that decided for itself would
 * eventually offer a field the host refuses, and the operator would meet the
 * disagreement as a failed save instead of as a field that will not take an
 * edit.
 */
export function AgentDetailView({
  client,
  company,
  agentId,
  onBack,
}: {
  client: OpenCompanyClient;
  company: string | null;
  agentId: string;
  onBack: () => void;
}) {
  const [load, setLoad] = useState<Load>("loading");
  const [agent, setAgent] = useState<AgentDetailDto | null>(null);
  /**
   * The id of the agent currently on screen, kept current even across a
   * navigation that happens while an async write is in flight. Guards the
   * reset/save response handling: a slow response for a previous agent must not
   * clobber the active detail's draft or flip its editor.
   */
  const displayedAgentIdRef = useRef<string | null>(null);
  useEffect(() => {
    displayedAgentIdRef.current = agent?.id ?? null;
  }, [agent]);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<AgentDraft>(emptyDraft());
  const [saving, setSaving] = useState(false);
  /**
   * What this teammate is on and carrying (issue #1141), or `null` when the
   * board could not be read — in which case the header states neither rather
   * than an invented "idle · 0 open".
   */
  const [workload, setWorkload] = useState<Workload | null>(null);
  /** An inbox write is in flight; the switch is held until the host answers. */
  const [inboxSaving, setInboxSaving] = useState(false);
  /**
   * Whether this viewer may edit the daily budget (issue #1206, ported from
   * `TeamView.tsx`). Courtesy, not enforcement — the host refuses the write
   * with a 403 regardless; hiding the control from a non-admin only spares
   * them a control they cannot use. Every agent this page can show is
   * host-backed by construction (`boot` only reaches `ready` once `getAgent`
   * answers), so there is no `fromHost` half to this check the way the roster
   * card needed.
   */
  const [isAdmin, setIsAdmin] = useState(false);
  // Who set the cap override, for the attribution line. Only an admin may read
  // the user directory, so this stays empty for a member and the attribution
  // degrades to "an admin" rather than disappearing.
  const [people, setPeople] = useState<Person[]>([]);
  /** Whether the daily-budget dialog is open. */
  const [budgetOpen, setBudgetOpen] = useState(false);

  useEffect(() => {
    let live = true;
    void (async () => {
      let admin = false;
      try {
        admin = (await fetchMe(client, company)).role === "admin";
      } catch {
        // No user plane on this host, or not signed in — treat as non-admin.
      }
      if (!live) return;
      setIsAdmin(admin);
      if (!admin) {
        setPeople([]);
        return;
      }
      try {
        const dir = await listPeople(client, company);
        if (live) setPeople(dir);
      } catch {
        // Attribution falls back to "an admin"; not worth a toast.
        if (live) setPeople([]);
      }
    })();
    return () => {
      live = false;
    };
  }, [client, company]);

  /** A human label for whoever set a cap — never a raw user id. */
  function whoSet(userId: string): string {
    const person = people.find((p) => p.id === userId);
    return person?.displayName?.trim() || person?.email || "an admin";
  }

  const boot = useCallback(async () => {
    setLoad("loading");
    try {
      const detail = await client.getAgent(agentId, company);
      setAgent(detail);
      setDraft(draftFrom(detail));
      setLoad("ready");
    } catch (error) {
      setAgent(null);
      setLoad(await classifyFailure(error, () => client.listTeam(company), agentId));
    }
  }, [client, company, agentId]);

  useEffect(() => {
    void boot();
  }, [boot]);

  /**
   * The board, read for this one teammate — the same derivation the Company
   * cards use, from the same two reads (`lib/team-workload.ts`).
   *
   * Best-effort and never blocking: a host with no `…/tasks` route still opens
   * a teammate, it just cannot say what they are on.
   */
  useEffect(() => {
    let live = true;
    if (!company) {
      setWorkload(null);
      return;
    }
    void (async () => {
      const [tasks, columns] = await Promise.all([
        listTasks(client, company).catch(() => null),
        fetchBoardColumns(client, company).catch(() => null),
      ]);
      if (!live) return;
      // Empty columns is a host whose ledger list carries no board — an absence,
      // not a vocabulary. Same rule as the roster's cards.
      setWorkload(
        tasks && columns?.length
          ? (workloadByAssignee(tasks, columns).get(agentId) ?? { open: 0, status: "idle" })
          : null,
      );
    })();
    return () => {
      live = false;
    };
  }, [client, company, agentId]);

  /**
   * Give this teammate an inbox, or take it away (issue #1190).
   *
   * Moved here from the roster card, where it was the only control that wrote
   * to the host and sat one mis-click away while scanning thirteen cards. This
   * page already *reported* inbox state as a badge and offered no way to change
   * it; the read and the write live together now.
   *
   * Optimistic, then reverted on failure — the switch must never be left
   * claiming a state the host refused. Keyed on the roster agent id, which is
   * the `InboxStore` key the Inbox page reads and the ingest webhook files mail
   * under; nothing is persisted client-side.
   */
  async function toggleInbox(next: boolean) {
    if (!agent || inboxSaving) return;
    // Scoped to the teammate this call is *about*. This screen does not remount
    // when the hash names a different agent — it re-reads into the same state —
    // so a slow write for A that fails after the operator has stepped to B would
    // otherwise roll back B's switch, for a request B never made.
    const apply = (enabled: boolean) =>
      setAgent((held) => (held?.id === agentId ? { ...held, inboxEnabled: enabled } : held));
    apply(next);
    // One write in flight at a time. Two quick taps otherwise race, and the
    // host's last-writer-wins can settle on the opposite of what the switch shows.
    setInboxSaving(true);
    try {
      await setInboxEnabled(client, company, agentId, next);
    } catch (error) {
      apply(!next);
      toast.error(
        error instanceof ApiError && error.status === 404
          ? "This host doesn't offer teammate inboxes yet."
          : error instanceof Error
            ? error.message
            : "Couldn't change the inbox.",
      );
    } finally {
      setInboxSaving(false);
    }
  }

  /**
   * Set, change, or remove this teammate's daily cap (issue #1206, moved here
   * from the roster card for the same reason Inbox moved in #1190: a card in
   * a grid of thirteen is for recognising a teammate, not configuring one).
   *
   * `cap` is `null` to remove the cap and a number to set one — `0` included,
   * which caps the teammate at nothing. The two are different states on the
   * host and must stay different here, which is why this takes `number | null`
   * and never an optional.
   *
   * Merges the host's answer into `agent` rather than refetching, the same way
   * `toggleInbox` does — and the same `held?.id === agentId` guard, so a slow
   * write does not clobber state after the operator has navigated elsewhere.
   */
  async function applyBudget(cap: number | null) {
    try {
      const row = await client.setTeamBudget(agentId, cap, company);
      setAgent((held) =>
        held?.id === agentId
          ? {
              ...held,
              budgetUsdDaily: row.budgetUsdDaily,
              spentTodayUsd: row.spentTodayUsd,
              budgetSetBy: row.budgetSetBy,
              budgetSetAtMillis: row.budgetSetAtMillis,
            }
          : held,
      );
      toast.success(cap === null ? "Daily cap removed." : `Daily cap set to $${cap.toFixed(2)}.`);
    } catch (error) {
      toast.error(budgetError(error, "Couldn't change the daily cap."));
    }
  }

  /** Drop the override so the company's own default applies again. */
  async function resetBudget() {
    try {
      const row = await client.clearTeamBudgetOverride(agentId, company);
      setAgent((held) =>
        held?.id === agentId
          ? {
              ...held,
              budgetUsdDaily: row.budgetUsdDaily,
              spentTodayUsd: row.spentTodayUsd,
              budgetSetBy: row.budgetSetBy,
              budgetSetAtMillis: row.budgetSetAtMillis,
            }
          : held,
      );
      toast.success("Reset to the company default.");
    } catch (error) {
      toast.error(budgetError(error, "Couldn't reset the daily cap."));
    }
  }

  async function save() {
    if (!agent) return;
    const edits = agentEdits(agent, draft);
    if (!edits) {
      setEditing(false);
      return;
    }
    setSaving(true);
    try {
      const updated = await client.updateAgent(agentId, edits, company);
      // A slow save must not clobber the active detail: only fold the response
      // in when the agent on screen is still the one we saved (the same guard
      // the reset and budget writes use).
      if (displayedAgentIdRef.current !== agentId) return;
      setAgent(updated);
      setDraft(draftFrom(updated));
      setEditing(false);
      toast.success("Teammate updated.");
    } catch (error) {
      toast.error(
        error instanceof ApiError && error.status === 409
          ? error.message
          : error instanceof Error
            ? error.message
            : "Couldn't save this teammate.",
      );
    } finally {
      setSaving(false);
    }
  }

  /**
   * Drop this teammate's instructions override so its blueprint persona applies
   * again (issue #1530). Sends `instructions: null` — the three-state reset,
   * distinct from saving an emptied field only in that it needs no edit form
   * open. Offered only when an override is actually in force.
   */
  async function resetInstructions() {
    if (!agent) return;
    setSaving(true);
    try {
      const updated = await client.updateAgent(agentId, { instructions: null }, company);
      // A slow reset must not clobber the active detail: only fold the response
      // in when the agent on screen is still the one we asked to reset (the same
      // identity guard the budget writes and inbox toggle use).
      if (displayedAgentIdRef.current !== agentId) return;
      setAgent(updated);
      setDraft(draftFrom(updated));
      setEditing(false);
      toast.success("Instructions reset to the blueprint.");
    } catch (error) {
      toast.error(
        error instanceof ApiError && error.status === 409
          ? error.message
          : error instanceof Error
            ? error.message
            : "Couldn't reset the instructions.",
      );
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6">
        {/*
          A breadcrumb rather than a Back button (issue #1141). Back said where
          the operator had been; this says where they *are* — one teammate,
          inside the company — which is the question a linked page has to answer,
          and this page is linked from the org chart, the chat member pane and
          every "Not on a desk" chip. Arriving from any of those, "Back to team"
          named a page they had never seen.
        */}
        <nav aria-label="Breadcrumb" data-testid="agent-breadcrumb">
          <ol className="flex flex-wrap items-center gap-1 text-sm">
            <li>
              <Button
                variant="ghost"
                size="sm"
                className="-ml-2 h-7 px-2 text-muted-foreground"
                onClick={onBack}
                data-testid="agent-breadcrumb-company"
              >
                Company
              </Button>
            </li>
            <li aria-hidden className="text-muted-foreground">
              <ChevronRight className="size-3.5" />
            </li>
            <li aria-current="page" className="min-w-0 truncate font-medium">
              {/* Named as soon as there is a name, and "Teammate" until then.
                  A crumb that appeared only once the read landed would move
                  the page's controls across the row as it settled. */}
              {agent ? (agent.name?.trim() || agent.role) : "Teammate"}
            </li>
          </ol>
        </nav>

        {load === "loading" && <Skeleton className="h-64 rounded-xl" />}

        {load === "missing" && (
          <EmptyState
            title="This teammate is no longer on the roster."
            body="It may have been removed. Go back to the team to see who is here now."
          />
        )}

        {load === "unsupported" && (
          <EmptyState
            title="This host can't open a teammate yet."
            body="Opening a teammate needs a newer host. The roster still works."
          />
        )}

        {load === "error" && (
          <EmptyState
            title="Couldn't load this teammate."
            body="The company host didn't answer. Try again in a moment."
          />
        )}

        {load === "ready" && agent && (
          <>
            <Identity
              agent={agent}
              action={
                !editing ? (
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setEditing(true)}
                    // Disabled with the reason, never absent — an operator
                    // looking for the edit needs to find out *why* there isn't
                    // one, not to conclude the console forgot to build it. What
                    // makes a teammate uneditable is the host's own `editable`
                    // list and nothing this file decides: a current host offers
                    // at least name, role and instructions on every teammate,
                    // manifest ones included, so an empty list now means a host
                    // that does not support the edit rather than a blueprint row
                    // this console must refuse.
                    disabled={agent.editable.length === 0}
                    title={
                      agent.editable.length === 0
                        ? "This teammate can't be edited from here."
                        : undefined
                    }
                    data-testid="agent-edit"
                  >
                    <Pencil className="size-4" /> Edit
                  </Button>
                ) : undefined
              }
            />
            <FactLine agent={agent} workload={workload} />

            {/* The Edit action sits on the teammate's name row (issue #1434) —
                one editing action, in the place a page's actions live, rather
                than halfway down inside one of its cards. */}
            <Section
              title="Instructions"
              subtitle="What this teammate was defined to do. It frames every turn they take."
              action={
                // Reset is offered only when an override is actually masking the
                // blueprint, and only to a viewer the host will let write
                // instructions — otherwise it is a control that can only 409.
                isEditable(agent, "instructions") && agent.instructionsOverridden ? (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void resetInstructions()}
                    disabled={saving}
                    data-testid="agent-instructions-reset"
                  >
                    Reset to blueprint
                  </Button>
                ) : undefined
              }
            >
              {editing ? (
                <div className="grid gap-4">
                  <AgentFields
                    idPrefix="agent-edit"
                    draft={draft}
                    onChange={(key: AgentFieldKey, value) =>
                      setDraft((d) => ({ ...d, [key]: value }))
                    }
                    readOnly={(key) => !isEditable(agent, key)}
                  />
                  {agent.instructionsOverridden && agent.blueprintInstructions?.trim() && (
                    <p
                      className="whitespace-pre-wrap text-xs text-muted-foreground"
                      data-testid="agent-blueprint-hint"
                    >
                      Overriding the blueprint. Clearing this field, or “Reset to blueprint”,
                      restores: {agent.blueprintInstructions.trim()}
                    </p>
                  )}
                  <div className="flex justify-end gap-2">
                    <Button
                      variant="ghost"
                      onClick={() => {
                        setDraft(draftFrom(agent));
                        setEditing(false);
                      }}
                    >
                      Cancel
                    </Button>
                    <Button
                      onClick={() => void save()}
                      disabled={saving || !draftIsValid(agent, draft)}
                      data-testid="agent-save"
                    >
                      Save
                    </Button>
                  </div>
                </div>
              ) : (
                <>
                  <p
                    className="whitespace-pre-wrap text-sm text-muted-foreground"
                    data-testid="agent-description"
                  >
                    {agent.description?.trim() ||
                      "No description was written for this teammate."}
                  </p>
                  {agent.instructions?.trim() && (
                    <div className="space-y-1">
                      <p className="text-xs font-medium">
                        Persona instructions
                        {agent.instructionsOverridden ? " · overriding the blueprint" : ""}
                      </p>
                      <p
                        className="whitespace-pre-wrap text-sm text-muted-foreground"
                        data-testid="agent-instructions"
                      >
                        {agent.instructions.trim()}
                      </p>
                    </div>
                  )}
                  {agent.editable.length === 0 && (
                    <p className="text-xs text-muted-foreground" data-testid="agent-readonly-note">
                      This teammate can't be edited from here. Its daily budget can still be changed
                      below.
                    </p>
                  )}
                </>
              )}
            </Section>

            <Tools agent={agent} />
            <Inbox
              agent={agent}
              busy={inboxSaving}
              onToggle={(next) => void toggleInbox(next)}
            />
            <Budget
              agent={agent}
              canEdit={isAdmin}
              setByLabel={agent.budgetSetBy ? whoSet(agent.budgetSetBy) : undefined}
              onEdit={() => setBudgetOpen(true)}
              onRemoveCap={() => void applyBudget(null)}
              onResetBudget={() => void resetBudget()}
            />
            <Desks agent={agent} />
          </>
        )}
      </div>
      <BudgetDialog
        agent={budgetOpen ? agent : null}
        onOpenChange={setBudgetOpen}
        onSave={(cap) => {
          setBudgetOpen(false);
          void applyBudget(cap);
        }}
      />
    </div>
  );
}

/** Name, role, id, and the two facts that classify an agent. */
function Identity({ agent, action }: { agent: AgentDetailDto; action?: ReactNode }) {
  const display = agent.name?.trim() || agent.role;
  const seed = agent.id || display;
  const tone = toneFor(seed);
  // Same seed as `tone` — the id where there is one — so a rename doesn't
  // change this teammate's face on the one screen that should never be
  // showing letters (issue #1181, and issue #1185 for the seed itself).
  const avatar = avatarFor(seed);
  // #1208, on the page a teammate *is*. `display` already falls back to the
  // role, and a manifest-declared agent has no `name` at all, so the line under
  // the title was the title again on every teammate in every shipped company.
  const subtitle = roleSubtitle(display, agent.role);
  return (
    <div className="flex items-start justify-between gap-4">
      <div className="flex items-start gap-4 min-w-0">
        {/* The header of the page a teammate *is* — the one screen that should
            never be the one showing letters (issue #1181). 56px. */}
        <TeammateAvatar
          name={display}
          tone={tone}
          avatar={avatar}
          className="size-14 rounded-xl text-base"
          data-testid="agent-avatar"
        />
        <div className="min-w-0 flex-1 space-y-2">
          <div>
            <h1 className="truncate text-2xl font-semibold tracking-tight" data-testid="agent-name">
              {display}
            </h1>
            {subtitle && (
              <p className="truncate text-sm text-muted-foreground" data-testid="agent-role">
                {subtitle}
              </p>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="secondary" className="gap-1" data-testid="agent-tier">
              <Sparkles className="size-3" /> {tierLabel(agent)}
            </Badge>
            <Badge variant="outline" data-testid="agent-source">
              {agent.source === "manifest" ? "Company blueprint" : "Added here"}
            </Badge>
            {agent.inboxEnabled && (
              <Badge variant="outline" className="gap-1">
                <Mail className="size-3" /> Inbox
              </Badge>
            )}
            <span className="font-mono text-xs text-muted-foreground" data-testid="agent-id">
              {agent.id}
            </span>
          </div>
        </div>
      </div>
      {action && (
        <div className="flex shrink-0 flex-wrap justify-end gap-2">{action}</div>
      )}
    </div>
  );
}

/**
 * The three running facts about a teammate, on one line (issue #1141): what
 * they are on, how much is on them, and what today has cost.
 *
 * Every part is omitted independently when its source is silent, and none is
 * defaulted. A host that cannot answer the board draws no status and no count —
 * not "idle · 0 open", which is a claim — and an uncapped teammate draws no
 * spend line, because absence *is* the uncapped signal on the wire and `$0.00`
 * would read as a teammate capped at nothing.
 */
function FactLine({
  agent,
  workload,
}: {
  agent: AgentDetailDto;
  workload: Workload | null;
}) {
  const capped = agent.budgetUsdDaily !== undefined;
  if (!workload && !capped) return null;
  const working = workload?.status === "working";
  return (
    <div
      className="flex flex-wrap items-center gap-x-2 gap-y-2 text-sm text-muted-foreground"
      data-testid="agent-facts"
    >
      {workload && (
        <>
          <span className="flex items-center gap-1.5">
            <span
              className={cn(
                "size-2 shrink-0 rounded-full",
                working ? "bg-status-running" : "bg-status-idle",
              )}
              aria-hidden
            />
            <span
              className={cn(
                "font-medium",
                working ? "text-status-running-text" : "text-status-idle-text",
              )}
              data-testid="agent-status"
            >
              {working ? "Working" : "Idle"}
            </span>
          </span>
          <span aria-hidden>·</span>
          <span data-testid="agent-tasks">
            {workload.open === 1 ? "1 open task" : `${workload.open} open tasks`}
          </span>
        </>
      )}
      {capped && (
        <span data-testid="agent-spend">
          Today ${(agent.spentTodayUsd ?? 0).toFixed(2)} of $
          {(agent.budgetUsdDaily ?? 0).toFixed(2)}
        </span>
      )}
    </div>
  );
}

/**
 * The tool grants, resolved.
 *
 * Three facts, because the difference between them is the whole reason this
 * section exists. What the agent holds. Whether it holds it because it asked or
 * because it asked for nothing and inherited the company's grant. And what it
 * asked for and did not get, which is the line an operator checking a tool
 * change is actually looking for and which no surface showed before.
 */
function Tools({ agent }: { agent: AgentDetailDto }) {
  const summary = summarizeGrants(agent.tools);
  return (
    <Section
      title="Tools"
      subtitle={
        summary.standardGrant
          ? "This teammate lists no tools of its own, so it holds everything the company allows."
          : "What this teammate asked for, narrowed by what the company allows."
      }
    >
      {summary.effective.length === 0 ? (
        <p className="text-sm text-muted-foreground" data-testid="agent-tools-empty">
          {/* Both ways of holding nothing land here, and they are not the same
              fact. An agent that asked for nothing under a company that allows
              nothing has been refused nothing. */}
          {summary.standardGrant
            ? "This teammate has no tools, because the company allows none."
            : "This teammate has no tools. Nothing it asked for is covered by the company tool list."}
        </p>
      ) : (
        <div className="flex flex-wrap gap-2" data-testid="agent-tools">
          {summary.effective.map((glob) => (
            <Badge key={glob} variant="secondary" className="gap-1 font-mono text-xs">
              <Wrench className="size-3" /> {glob}
            </Badge>
          ))}
        </div>
      )}
      {summary.dropped.length > 0 && (
        <div className="space-y-1" data-testid="agent-tools-dropped">
          <p className="text-xs text-muted-foreground">
            Asked for but not granted, because the company tool list does not cover it:
          </p>
          <div className="flex flex-wrap gap-2">
            {summary.dropped.map((glob) => (
              <Badge key={glob} variant="outline" className="font-mono text-xs line-through">
                {glob}
              </Badge>
            ))}
          </div>
        </div>
      )}
      {!summary.standardGrant && (
        <p className="text-xs text-muted-foreground">
          Company tool list: {agent.tools.companyAllow.join(", ") || "nothing allowed"}
        </p>
      )}
    </Section>
  );
}

/**
 * Whether mail addressed to this teammate lands anywhere (issue #1190).
 *
 * A per-teammate setting, on the teammate's own page — not a switch in a grid
 * of cards, which is what it was. The subtitle says what turning it on actually
 * does, because "Inbox" alone does not: an inbox is an address the outside
 * world can reach, which is a different kind of decision from the rest of this
 * screen and worth one sentence.
 */
function Inbox({
  agent,
  busy,
  onToggle,
}: {
  agent: AgentDetailDto;
  /** A write is in flight — the switch is held rather than allowed to race. */
  busy: boolean;
  onToggle: (next: boolean) => void;
}) {
  return (
    <Section
      title="Inbox"
      subtitle="Give this teammate an address of its own, so mail routed to it arrives here rather than nowhere."
    >
      <label className="flex cursor-pointer items-center justify-between gap-3">
        <span className="flex items-center gap-2 text-sm">
          <Mail className="size-4 text-muted-foreground" />
          {agent.inboxEnabled ? "This teammate has an inbox." : "This teammate has no inbox."}
        </span>
        <Switch
          checked={agent.inboxEnabled}
          disabled={busy}
          onCheckedChange={onToggle}
          aria-label="Give this teammate an inbox"
          data-testid="agent-inbox-toggle"
        />
      </label>
    </Section>
  );
}

/**
 * Turns a failed budget write into something worth reading.
 *
 * The 403 is the one an operator will actually hit, and it needs to say *why* —
 * "only an admin can change a spend limit" is the answer, not "request failed".
 */
function budgetError(error: unknown, fallback: string): string {
  if (error instanceof ApiError) {
    if (error.status === 403) return "Only an admin can change a teammate's daily cap.";
    if (error.status === 404) return "This host doesn't support console budgets yet.";
    return error.message;
  }
  return error instanceof Error ? error.message : fallback;
}

/**
 * The teammate's daily spend cap, editable (issue #1206).
 *
 * Moved here from the roster card's `⋯` menu, for the same reason Inbox moved
 * in #1190: a card in a grid of thirteen is for recognising a teammate, not
 * configuring one. The card still shows the cap and today's spend — this is
 * where an operator now sets, changes, removes or resets it, beside Inbox.
 *
 * Renders the cap and attribution to everyone (the roster card does too), but
 * only offers the writing controls to an admin — same courtesy-not-enforcement
 * gate `TeamView.tsx` used, so a member sees the same facts without a control
 * that would only 403.
 */
function Budget({
  agent,
  canEdit,
  setByLabel,
  onEdit,
  onRemoveCap,
  onResetBudget,
}: {
  agent: AgentDetailDto;
  /** Whether to offer the writing controls at all (admins only). */
  canEdit: boolean;
  /** Who set the current override, already resolved to something readable. */
  setByLabel?: string;
  onEdit: () => void;
  onRemoveCap: () => void;
  onResetBudget: () => void;
}) {
  const cap = agent.budgetUsdDaily;
  const capped = cap !== undefined;
  // An override exists (someone set this deliberately), as opposed to the cap
  // simply coming from the company's own definition.
  const overridden = agent.budgetSetBy !== undefined;
  const usd = (n: number) => `$${n.toFixed(2)}`;
  return (
    <Section
      title="Budget"
      subtitle="The most this teammate may spend per day. It takes effect on their next task — no restart needed."
      action={
        canEdit ? (
          <Button variant="outline" size="sm" onClick={onEdit} data-testid="team-budget-edit">
            <Wallet className="size-4" />
            {capped ? "Change…" : "Set…"}
          </Button>
        ) : undefined
      }
    >
      <div className="space-y-1 text-sm" data-testid="agent-budget">
        {capped ? (
          <p className="text-muted-foreground">
            {usd(cap)}/day · {usd(agent.spentTodayUsd ?? 0)} spent today
          </p>
        ) : (
          <p className="text-muted-foreground">No daily cap — this teammate spends freely.</p>
        )}
        {setByLabel && agent.budgetSetAtMillis !== undefined && (
          <p className="text-xs text-muted-foreground" data-testid="agent-budget-attribution">
            {capped ? "Set by" : "Uncapped by"} {setByLabel} ·{" "}
            {new Date(agent.budgetSetAtMillis).toLocaleDateString()}
          </p>
        )}
      </div>
      {canEdit && (capped || overridden) && (
        <div className="flex flex-wrap gap-2">
          {capped && (
            <Button
              variant="ghost"
              size="sm"
              onClick={onRemoveCap}
              data-testid="team-budget-remove"
            >
              Remove cap
            </Button>
          )}
          {overridden && (
            <Button
              variant="ghost"
              size="sm"
              onClick={onResetBudget}
              data-testid="team-budget-reset"
            >
              Reset to company default
            </Button>
          )}
        </div>
      )}
    </Section>
  );
}

/**
 * Enter a daily cap for one teammate.
 *
 * Empty input is **not** submittable: "no cap" is the explicit "Remove cap"
 * action, not a blank field, so an operator clearing the box and saving can
 * never silently uncap a teammate. `0` is allowed and means exactly what it
 * says — this teammate may not spend.
 */
function BudgetDialog({
  agent,
  onOpenChange,
  onSave,
}: {
  agent: AgentDetailDto | null;
  onOpenChange: (open: boolean) => void;
  onSave: (cap: number) => void;
}) {
  const [value, setValue] = useState("");

  useEffect(() => {
    setValue(agent?.budgetUsdDaily !== undefined ? String(agent.budgetUsdDaily) : "");
  }, [agent]);

  const parsed = Number(value);
  const valid = value.trim() !== "" && Number.isFinite(parsed) && parsed >= 0;
  const name = agent?.name?.trim() || agent?.role || "this teammate";

  return (
    <Dialog open={agent !== null} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Daily budget</DialogTitle>
          <DialogDescription>
            The most {name} may spend per day. It takes effect on their next task — no restart
            needed.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-2">
          <Label htmlFor="agent-budget">US dollars per day</Label>
          <Input
            id="agent-budget"
            type="number"
            min={0}
            step="0.01"
            inputMode="decimal"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            placeholder="e.g. 5.00"
            data-testid="team-budget-input"
          />
          <p className="text-xs text-muted-foreground">
            $0 stops them spending entirely. To let them spend freely, use “Remove cap”.
          </p>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => valid && onSave(parsed)}
            disabled={!valid}
            data-testid="team-budget-save"
          >
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Desk membership, with the lead named. */
function Desks({ agent }: { agent: AgentDetailDto }) {
  return (
    <Section
      title="Desks"
      subtitle="The desks this teammate works on. A desk hands its work to its lead first."
    >
      {agent.desks.length === 0 ? (
        <p className="text-sm text-muted-foreground" data-testid="agent-desks-empty">
          This teammate is not on any desk.
        </p>
      ) : (
        <div className="flex flex-wrap gap-2" data-testid="agent-desks">
          {agent.desks.map((desk) => (
            <Badge key={desk.id} variant="secondary" className="gap-1">
              <Users className="size-3" /> {desk.name}
              {desk.lead && <span className="text-xs opacity-70">(lead)</span>}
            </Badge>
          ))}
        </div>
      )}
    </Section>
  );
}

function Section({
  title,
  subtitle,
  action,
  children,
}: {
  title: string;
  subtitle: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card>
      <CardContent className="space-y-3 py-4">
        <div className="flex items-start justify-between gap-3">
          <div className="space-y-1">
            <h3 className="font-medium">{title}</h3>
            <p className="text-xs text-muted-foreground">{subtitle}</p>
          </div>
          {action}
        </div>
        {children}
      </CardContent>
    </Card>
  );
}

function EmptyState({ title, body }: { title: string; body: string }) {
  return (
    <Card>
      <CardContent className="space-y-1 py-8 text-center">
        <p className="font-medium">{title}</p>
        <p className="text-sm text-muted-foreground">{body}</p>
      </CardContent>
    </Card>
  );
}
