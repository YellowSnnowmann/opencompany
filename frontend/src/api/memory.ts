// The live memory API: the console reads and writes the company's memory
// through the host's `…/memory` routes (REST, camelCase over the wire).
//
// OpenHuman's memory v2 engine backs every route, scoped to the company root
// (`team:<company>`). Each teammate's conversations live under
// `team:<company>/agent:<id>`, learnings at the root, and brain documents under
// `team:<company>/source:<kind>`. When no engine is configured every route but
// `/memory/status` answers `503 MEMORY_OFF`, so the console reads status first.

import type { OpenCompanyClient } from "./client";

/** What a memory item is: a learning, a conversation turn, or a brain document. */
export type ItemKind = "learning" | "conversation" | "document";

/** The taxonomy of a learning — what sort of thing the company learned. */
export type LearningKind = "preference" | "fact" | "procedure" | "correction" | "other";

/** One memory item as the host returns it. */
export interface MemoryEntry {
  id: string;
  kind: ItemKind;
  /** Present on learnings only. */
  learningKind?: LearningKind;
  /** The document title, or the first line of the text. */
  title: string;
  /** The text; conversation turns render as `role: text` lines. */
  body: string;
  /** The memory agent id, for items stored under an agent node. */
  agentId?: string;
  /** The OpenHuman namespace, e.g. `team:acme/agent:ceo`. */
  namespace: string;
  /** The brain source kind, for documents (`markdown`, `pdf`, `link`…). */
  source?: string;
  tags: string[];
  /** Epoch millis the item was observed, `0` when unknown. */
  updatedAt: number;
  /** Whether the operator may forget it — always true today. */
  editable: boolean;
}

/** One page of `GET /memory`. A `query` gives ranked matches and no cursor. */
export interface MemoryList {
  items: MemoryEntry[];
  /** The cursor for the next page, absent on the last one. */
  nextCursor?: string;
}

/** Whether memory is on, and which engine serves it. */
export interface MemoryStatus {
  /** The company's memory root namespace. */
  root: string;
  on: boolean;
  engine?: string;
  endpoint?: string;
  /** Why memory is off, when it is. */
  reason?: string;
}

/** One teammate with memory under the company root. */
export interface MemoryAgent {
  agentId: string;
  turns: number;
}

/** `GET /memory/agents` — the teammates that have conversation memory. */
export interface MemoryAgents {
  root: string;
  agents: MemoryAgent[];
}

/** A recall answer, with the items it drew on. */
export interface Recall {
  answer: string;
  citations: { id: string; text: string; score?: number }[];
}

/** One brain source kind and how many documents it holds. */
export interface BrainSource {
  source: string;
  documents: number;
}

/** `GET /memory/brain` — documents grouped by source kind. */
export interface BrainSources {
  root: string;
  sources: BrainSource[];
  /** Documents with no source kind. */
  unfiled: number;
}

/** The create-a-learning body; the host mints the id and timestamp. */
export interface CreateLearning {
  text: string;
  kind?: LearningKind;
}

/** Filters and paging for `GET /memory`. */
export interface ListMemoryOptions {
  query?: string;
  kind?: ItemKind;
  agent?: string;
  cursor?: string;
  limit?: number;
}

/** The item kinds in display order, for the kind filter. */
export const ITEM_KINDS: ItemKind[] = ["learning", "conversation", "document"];

/** Human labels for each item kind. */
export const ITEM_KIND_LABELS: Record<ItemKind, string> = {
  learning: "Learning",
  conversation: "Conversation",
  document: "Document",
};

/**
 * Per-kind badge styling — identity, not state (the `--tone-*` palette).
 * Documents share a learning's blue on purpose: both are knowledge the
 * operator supplied, as against the agents' own conversations.
 */
export const ITEM_KIND_STYLES: Record<ItemKind, string> = {
  learning: "border-tone-2/30 bg-tone-2/10 text-tone-2-text",
  conversation: "border-tone-3/30 bg-tone-3/10 text-tone-3-text",
  document: "border-tone-4/30 bg-tone-4/10 text-tone-4-text",
};

/** The learning kinds in display order, for the add form. */
export const LEARNING_KINDS: LearningKind[] = [
  "fact",
  "preference",
  "procedure",
  "correction",
  "other",
];

/** Human labels for each learning kind. */
export const LEARNING_KIND_LABELS: Record<LearningKind, string> = {
  fact: "Fact",
  preference: "Preference",
  procedure: "Procedure",
  correction: "Correction",
  other: "Other",
};

/** The company's memory, one page at a time, optionally filtered server-side. */
export function listMemory(
  client: OpenCompanyClient,
  company: string | null,
  opts?: ListMemoryOptions,
): Promise<MemoryList> {
  const params = new URLSearchParams();
  if (opts?.query) params.set("query", opts.query);
  if (opts?.kind) params.set("kind", opts.kind);
  if (opts?.agent) params.set("agent", opts.agent);
  if (opts?.cursor) params.set("cursor", opts.cursor);
  if (opts?.limit) params.set("limit", String(opts.limit));
  const qs = params.toString();
  return client.get<MemoryList>(`${client.scopeFor(company)}/memory${qs ? `?${qs}` : ""}`);
}

/** Stores a learning at the company root. */
export function createMemory(
  client: OpenCompanyClient,
  company: string | null,
  body: CreateLearning,
): Promise<MemoryEntry> {
  return client.post<MemoryEntry>(`${client.scopeFor(company)}/memory`, body);
}

/** Forgets one memory item by id. */
export function deleteMemory(
  client: OpenCompanyClient,
  company: string | null,
  id: string,
): Promise<void> {
  return client.del<void>(`${client.scopeFor(company)}/memory/${encodeURIComponent(id)}`);
}

/** Whether memory is on — the one route that answers when it is off. */
export function memoryStatus(
  client: OpenCompanyClient,
  company: string | null,
): Promise<MemoryStatus> {
  return client.get<MemoryStatus>(`${client.scopeFor(company)}/memory/status`);
}

/** The teammates that have conversation memory, with their turn counts. */
export function memoryAgents(
  client: OpenCompanyClient,
  company: string | null,
): Promise<MemoryAgents> {
  return client.get<MemoryAgents>(`${client.scopeFor(company)}/memory/agents`);
}

/** Forgets everything one teammate remembers of its conversations. */
export function forgetAgentMemory(
  client: OpenCompanyClient,
  company: string | null,
  agentId: string,
): Promise<{ forgotten: number }> {
  return client.del<{ forgotten: number }>(
    `${client.scopeFor(company)}/memory/agents/${encodeURIComponent(agentId)}`,
  );
}

/** Asks memory a question, optionally as one teammate. */
export function recallMemory(
  client: OpenCompanyClient,
  company: string | null,
  question: string,
  agent?: string,
): Promise<Recall> {
  return client.post<Recall>(
    `${client.scopeFor(company)}/memory/recall`,
    agent ? { question, agent } : { question },
  );
}

/** The brain's documents, grouped by source kind. */
export function brainSources(
  client: OpenCompanyClient,
  company: string | null,
): Promise<BrainSources> {
  return client.get<BrainSources>(`${client.scopeFor(company)}/memory/brain`);
}

// ─────────────────────────────────────────────────────────────────────────────
// Dropping documents and links into memory
// ─────────────────────────────────────────────────────────────────────────────

/** What happened to one dropped file or link. */
export interface IngestedItem {
  /** The file name, the relative path inside a dropped folder, or the URL. */
  source: string;
  status: "stored" | "empty" | "unsupported" | "failed";
  chunks: number;
  detail?: string;
}

/** A whole drop's outcome — one row per source, never one for the batch. */
export interface Ingested {
  items: IngestedItem[];
  chunks: number;
  stored: number;
}

/**
 * One file on its way to memory: the browser's `File` plus the path it had
 * inside a dropped folder.
 *
 * The path is sent as the part's filename, because it is what the operator
 * sees in their own file manager and the only thing telling four `README.md`s
 * apart.
 */
export interface DroppedFile {
  path: string;
  file: File;
}

/** Uploads one batch of files for extraction into memory. */
export function ingestDocuments(
  client: OpenCompanyClient,
  company: string | null,
  files: DroppedFile[],
): Promise<Ingested> {
  const form = new FormData();
  for (const { path, file } of files) {
    form.append("file", file, path);
  }
  return client.postForm<Ingested>(`${client.scopeFor(company)}/memory/ingest`, form);
}

/** Fetches links host-side and remembers what they said. */
export function ingestLinks(
  client: OpenCompanyClient,
  company: string | null,
  urls: string[],
): Promise<Ingested> {
  return client.post<Ingested>(`${client.scopeFor(company)}/memory/ingest/links`, { urls });
}

/**
 * Forgets every brain document of one source kind (`markdown`, `pdf`, `link`…).
 *
 * OpenHuman files brain documents under `team:<company>/source:<kind>`, so a
 * source kind is the unit the host can forget in one call.
 */
export function forgetDocument(
  client: OpenCompanyClient,
  company: string | null,
  source: string,
): Promise<{ forgotten: number }> {
  return client.del<{ forgotten: number }>(
    `${client.scopeFor(company)}/memory/document/${encodeURIComponent(source)}`,
  );
}
