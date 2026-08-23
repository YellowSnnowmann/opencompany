// The search configuration API: which provider a company's agents search
// through, and the key behind it.
//
// The key is write-only: it is sent on save into the host's secret store and is
// never returned. The read shape carries the provider slug, booleans and the
// non-secret endpoint only, so there is no field on this type that could leak a
// key into a rendered page.
//
// Standalone functions over the shared client, mirroring `api/hosting.ts`, so
// `OpenCompanyClient` needs no new methods.

import type { OpenCompanyClient } from "./client";

/**
 * The non-secret view of a company's search configuration.
 *
 * `provider` is what the operator picked; `effectiveProvider` is what the
 * agents actually search through. They differ exactly when a provider is
 * selected with its credential missing, which is the one state a single
 * "connected" flag would render as a working connection.
 */
export interface SearchStatus {
  /** The provider the company selected. `managed` when it selected nothing. */
  provider: string;
  /** The provider the agents actually search through. */
  effectiveProvider: string;
  /** Whether an API key is stored. Never the key. */
  apiKeyConfigured: boolean;
  /** The instance URL, for SearXNG. Not secret. */
  endpoint: string | null;
  /** Whether the selected provider is still missing its key. */
  needsApiKey: boolean;
  /** Whether the selected provider is still missing its endpoint. */
  needsEndpoint: boolean;
  /** Whether the company's manifest explicitly grants `search`. */
  granted: boolean;
  /** Whether the running host has the search tools compiled in. */
  inBuild: boolean;
  /** The providers this build can search through. */
  supportedProviders: string[];
}

/** The write-only save body. Omitted fields keep their stored value. */
export interface SearchConfig {
  /** The provider slug. Omit to leave it unchanged. */
  provider?: string;
  /** Write-only. Omit to leave the stored key unchanged. */
  apiKey?: string;
  /** The instance URL, for SearXNG. Omit to leave it unchanged. */
  endpoint?: string;
}

/** Reads the company's search configuration status. */
export async function getSearch(
  client: OpenCompanyClient,
  company: string | null,
): Promise<SearchStatus> {
  return client.get<SearchStatus>(`${client.scopeFor(company)}/search`);
}

/**
 * Saves whatever is supplied, and returns the resulting status.
 *
 * A patch, not a replace: the host applies only the fields present and
 * non-empty, so switching provider never means re-typing an API key — which an
 * operator cannot do anyway, since it is never shown back to them.
 */
export async function saveSearch(
  client: OpenCompanyClient,
  company: string | null,
  config: SearchConfig,
): Promise<SearchStatus> {
  return client.put<SearchStatus>(`${client.scopeFor(company)}/search`, config);
}

/** Clears the whole connection, falling the company back to managed search. */
export async function clearSearch(
  client: OpenCompanyClient,
  company: string | null,
): Promise<SearchStatus> {
  return client.del<SearchStatus>(`${client.scopeFor(company)}/search/key`);
}
