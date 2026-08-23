// The Chargebee billing configuration API (issue #788, UI tracked in #527).
//
// Credentials are write-only: the API key and the webhook credential are sent
// on save and stored in the host's secret store; neither is ever returned. The
// read shape carries booleans and the (non-secret) site identifier only, so
// there is no field on this type that could leak a key into a rendered page.
//
// Standalone functions over the shared client, mirroring `api/mcp.ts` and
// `api/skills.ts`, so `OpenCompanyClient` needs no new methods.

import type { OpenCompanyClient } from "./client";

/**
 * The non-secret view of a company's Chargebee configuration.
 *
 * Four separate flags rather than one `connected`, because they fail
 * differently and a single boolean sends an operator to the wrong place for
 * three of them — see `views/finance/health.ts` for the precedence between
 * them and how each is worded.
 */
export interface BillingStatus {
  /** Whether an API key is stored. Never the key. */
  apiKeyConfigured: boolean;
  /** The Chargebee site slug, e.g. `acme-test`. Not secret. */
  site: string | null;
  /** Whether a webhook credential is stored. */
  webhookConfigured: boolean;
  /** The URL to paste into Chargebee, or null on a host with no public URL. */
  webhookUrl: string | null;
  /** Whether the company's manifest explicitly grants `chargebee`. */
  granted: boolean;
  /** Whether the `chargebee` feature is compiled into the running host. */
  inBuild: boolean;
}

/** The write-only save body. Omitted fields keep their stored value. */
export interface BillingConfig {
  /** Write-only. Omit to leave the stored key unchanged. */
  apiKey?: string;
  /** The site identifier; accepts a bare slug, a host, or a full URL. */
  site?: string;
  /** Write-only `username:password` pair. Omit to leave it unchanged. */
  webhookSecret?: string;
}

/** Reads the company's Chargebee configuration status. */
export async function getBilling(
  client: OpenCompanyClient,
  company: string | null,
): Promise<BillingStatus> {
  return client.get<BillingStatus>(`${client.scopeFor(company)}/billing/chargebee`);
}

/**
 * Saves whatever is supplied, and returns the resulting status.
 *
 * A patch, not a replace: the host applies only the fields present and
 * non-empty, so correcting the site never means re-typing the API key — which
 * an operator cannot do anyway, since it is never shown back to them.
 */
export async function saveBilling(
  client: OpenCompanyClient,
  company: string | null,
  config: BillingConfig,
): Promise<BillingStatus> {
  return client.put<BillingStatus>(`${client.scopeFor(company)}/billing/chargebee`, config);
}

/** Clears every stored Chargebee credential. */
export async function clearBilling(
  client: OpenCompanyClient,
  company: string | null,
): Promise<BillingStatus> {
  return client.del<BillingStatus>(`${client.scopeFor(company)}/billing/chargebee/key`);
}

/** The non-secret view of a company's PayPal connection (issue #789). */
export interface PaypalStatus {
  /** Whether a client id is stored. Never the id. */
  clientIdConfigured: boolean;
  /** Whether a client secret is stored. */
  clientSecretConfigured: boolean;
  /** `sandbox` or `live` — which PayPal world the credentials belong to. */
  environment: string;
  /** Whether the company's manifest explicitly grants `paypal`. */
  granted: boolean;
  /** Whether the `paypal` feature is compiled into the running host. */
  inBuild: boolean;
}

/** The write-only PayPal save body. Omitted fields keep their stored value. */
export interface PaypalConfig {
  clientId?: string;
  clientSecret?: string;
  /** `sandbox` or `live`; anything else is stored as `sandbox`. */
  environment?: string;
}

/** Reads the company's PayPal configuration status. */
export async function getPaypal(
  client: OpenCompanyClient,
  company: string | null,
): Promise<PaypalStatus> {
  return client.get<PaypalStatus>(`${client.scopeFor(company)}/billing/paypal`);
}

/** Saves whatever is supplied, and returns the resulting status. */
export async function savePaypal(
  client: OpenCompanyClient,
  company: string | null,
  config: PaypalConfig,
): Promise<PaypalStatus> {
  return client.put<PaypalStatus>(`${client.scopeFor(company)}/billing/paypal`, config);
}

/** Clears the stored PayPal credentials and resets the environment. */
export async function clearPaypal(
  client: OpenCompanyClient,
  company: string | null,
): Promise<PaypalStatus> {
  return client.del<PaypalStatus>(`${client.scopeFor(company)}/billing/paypal/key`);
}
