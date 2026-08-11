// Skill presentation data for the console: per-category badge styling.
//
// Both the company's effective skills and the installable shared registry come
// from the host over the `…/skills` API (`@/api/skills`). Nothing about *which*
// skills exist lives on the client — a hardcoded registry array used to live
// here, and it had already drifted from what the backend could actually serve.

export type SkillCategory = "Marketing" | "Research" | "Ops" | "Content" | "Finance";

export const CATEGORY_STYLES: Record<SkillCategory, string> = {
  Marketing: "border-violet-500/30 bg-violet-500/10 text-violet-600 dark:text-violet-400",
  Research: "border-sky-500/30 bg-sky-500/10 text-sky-600 dark:text-sky-400",
  Ops: "border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400",
  Content: "border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
  Finance: "border-rose-500/30 bg-rose-500/10 text-rose-600 dark:text-rose-400",
};

// What a skill actually is to a teammate (issue #569).
//
// A desk agent can list, describe and read an installed skill, and can never run
// one: `dispatched_belt_excludes_every_deferred_family` pins `run_skill`,
// `skill_run`, `run_workflow` and `await_workflow` off every dispatched belt,
// and only the orchestrator is handed `RunWorkflowTool`. That is deliberate —
// the upstream runner reaches for a global config and bypasses the harness's
// metering — but the tab was built from the vocabulary of switching a capability
// on (install / enable / disable), so an operator reasonably read "enabled" as
// "a teammate will now do this", and nothing on the screen disagreed until they
// tried it and watched nothing happen.
//
// The copy lives here rather than inline in the view so the claim is one string
// with one test on it. What must not regress is the *claim*, not its layout.

/**
 * The Skills tab's standing statement of what installing and enabling a skill
 * does. Says the two things the screen otherwise implies the opposite of:
 * teammates **read** skills, and **running** one is the orchestrator's job.
 */
export const SKILLS_READ_ONLY_NOTE =
  "Skills are reference material your teammates read — playbooks they follow, not buttons they press. " +
  "Enabling one puts it in front of every teammate; executing a saved workflow stays the orchestrator's job.";

/**
 * What an installed skill's on/off state means for the company's teammates.
 *
 * Deliberately phrased as reach ("can read it") rather than capability ("can use
 * it"): the switch decides whether a skill is visible to a desk agent, and never
 * whether one can execute it.
 */
export function skillReachLabel(enabled: boolean): string {
  return enabled ? "Teammates can read this" : "Hidden from teammates";
}
