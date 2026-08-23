// The Settings section's sub-page table, and the helpers that read it.
//
// It lives in its own module rather than beside the section that renders it so
// that anything *pointing at* a sub-page can name one without importing the
// section — which imports every view under it, and would import itself back
// through them. `device-pairing.tsx` is the case that forced it: it tells a
// desktop user where to go, and for one release it told them to go somewhere
// that did not exist (issue #1476). Directions read from this table cannot say
// that again — a page id that is not here is a type error.

import {
  Blocks,
  ChartColumnBig,
  Globe,
  Laptop,
  Plug,
  Search,
  type LucideIcon,
  Settings2,
  Sparkles,
  UserCog,
} from "lucide-react";

/** The sub-pages that live under Settings. The id is the hash's second segment. */
export const SETTINGS_PAGES = [
  {
    id: "general",
    label: "General",
    icon: Settings2,
    hint: "Approvals, connection, lifecycle, domain, mail",
    group: "identity",
  },
  {
    id: "people",
    label: "People",
    icon: UserCog,
    hint: "Who can sign in, and as what",
    group: "identity",
  },
  // Beside People because it answers the same question one scope smaller: People
  // is who may sign in, this is which machines already have.
  { id: "devices", label: "Devices", icon: Laptop, hint: "Machines paired to your account", group: "identity" },
  { id: "connections", label: "Connections", icon: Plug, hint: "Third-party accounts", group: "integrations" },
  { id: "mcp", label: "MCP Servers", icon: Blocks, hint: "Tool servers and their tools", group: "integrations" },
  // A credential form belongs beside what it unlocks. An operator looking for
  // "where do I put my Vercel token" searches for hosting, so it sits here
  // rather than inside a third-party-accounts drawer.
  { id: "hosting", label: "Hosting", icon: Globe, hint: "Where this company's sites go live", group: "integrations" },
  // Beside Hosting for the same reason: a credential form belongs beside what
  // it unlocks, and an operator looking for "where do I put my Brave key"
  // searches for search.
  { id: "search", label: "Search", icon: Search, hint: "Where teammates look things up", group: "integrations" },
  // "What this company knows how to do" read as capability the company performs
  // — the implication issue #569 exists to remove, set here *before* the tab
  // gets a chance to correct it. The siblings describe their content; so does
  // this now.
  { id: "skills", label: "Skills", icon: Sparkles, hint: "Playbooks your teammates read", group: "capability" },
  { id: "usage", label: "Usage", icon: ChartColumnBig, hint: "What this company is spending", group: "spend" },
] as const satisfies readonly { id: string; label: string; icon: LucideIcon; hint: string; group: string }[];

export type SettingsPage = (typeof SETTINGS_PAGES)[number]["id"];

/** The settings rail groups related sub-pages without changing their routes. */
export const SETTINGS_PAGE_GROUPS = [
  { id: "identity", label: "Identity & lifecycle" },
  { id: "integrations", label: "Integrations" },
  { id: "capability", label: "Capability" },
  { id: "spend", label: "Spend" },
] as const satisfies readonly { id: (typeof SETTINGS_PAGES)[number]["group"]; label: string }[];

export const DEFAULT_SETTINGS_PAGE: SettingsPage = "general";

/** Whether a hash segment names a real sub-page. */
export function resolveSettingsPage(sub: string | null): SettingsPage {
  return SETTINGS_PAGES.some((p) => p.id === sub) ? (sub as SettingsPage) : DEFAULT_SETTINGS_PAGE;
}

/**
 * What the sub-nav calls a page, for prose that sends someone to it.
 *
 * Typed to `SettingsPage`, so directions written against this cannot outlive
 * the page they name: renaming a page rewrites the sentence, and removing one
 * stops the build.
 */
export function settingsPageLabel(page: SettingsPage): string {
  return SETTINGS_PAGES.find((p) => p.id === page)!.label;
}
