// The finance read plane: live Chargebee and PayPal data (issues #788, #789;
// host module `src/server/ops/finance.rs`).
//
// Distinct from `api/billing.ts`, which configures the *credentials*. This file
// is what the credentials unlock.
//
// # Why these types are snake_case
//
// They mirror the host's `chargebee::types` and `paypal::api` projections
// verbatim, and those are the shapes an agent already sees from
// `chargebee_list_invoices` and `paypal_list_transactions`. One invoice, one
// vocabulary, whether it is read in chat or in the console — and
// `total_in_minor_units` keeps saying what unit it is in, which a camelCase
// rewrite to `total` would quietly drop.

import type { OpenCompanyClient } from "./client";

/** A Chargebee invoice, as the host projects it. Money is integer minor units. */
export interface Invoice {
  id: string;
  customer_id: string;
  /** `paid`, `payment_due`, `posted`, `voided`, … Decode with `invoiceStatus`. */
  status: string;
  currency_code: string;
  total_in_minor_units: number;
  amount_due_in_minor_units: number;
  amount_paid_in_minor_units: number;
  /** Unix **seconds**, when Chargebee reports one. Not milliseconds. */
  due_date: number | null;
  line_items: string[];
  /** A hosted page where the customer can pay, when one could be raised. */
  payment_url: string | null;
  /** Present and true only when Chargebee replayed an earlier invoice. */
  replayed_earlier_invoice?: boolean;
}

/** A Chargebee customer. */
export interface Customer {
  id: string;
  email: string | null;
  name: string | null;
  company: string | null;
}

/** One line on an invoice being raised. */
export interface ChargeLine {
  description: string;
  /** Integer minor units. Build it with `toMinorUnits`, never `x * 100`. */
  amount_in_minor_units: number;
}

/** The body of `POST …/finance/chargebee/invoices`. */
export interface SendInvoice {
  customer_email: string;
  customer_name?: string;
  currency_code: string;
  line_items: ChargeLine[];
  due_days?: number;
  invoice_note?: string;
  /**
   * A retried or double-clicked send that reuses this key gets the original
   * invoice back instead of billing twice. Mint one per dialog, not per click.
   */
  idempotency_key?: string;
}

/** One currency's PayPal balance. Amounts are decimal **strings**. */
export interface Balance {
  currency_code: string;
  /**
   * As PayPal reports it, e.g. `"4320.50"`. Kept as text end to end: this is
   * rendered, never computed on, and `4320.50` through a float is how a balance
   * acquires a trailing `0000001`.
   */
  available: string;
  withheld: string;
  primary: boolean;
}

/** One PayPal transaction. */
export interface Transaction {
  id: string;
  /** ISO 8601, as PayPal reports it. */
  date: string;
  /** Signed decimal string — negative for money leaving the account. */
  amount: string;
  currency_code: string;
  /** `S` | `P` | `V` | `D`. Decode with `transactionStatus`. */
  status: string;
  counterparty: string | null;
  note: string | null;
}

/** What a connection test reports when the credential worked. */
export interface TestResult {
  ok: boolean;
  /** What was verified, naming the site or environment that answered. */
  detail: string;
}

/** Filters on the invoice list. */
export interface InvoiceFilter {
  status?: string;
  customerEmail?: string;
  limit?: number;
}

/** Builds a query string, omitting empty values. */
function query(params: Record<string, string | number | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === "") continue;
    search.set(key, String(value));
  }
  const rendered = search.toString();
  return rendered ? `?${rendered}` : "";
}

/** Lists invoices, newest first as Chargebee orders them. */
export async function listInvoices(
  client: OpenCompanyClient,
  company: string | null,
  filter: InvoiceFilter = {},
): Promise<Invoice[]> {
  return client.get<Invoice[]>(
    `${client.scopeFor(company)}/finance/chargebee/invoices${query({
      status: filter.status,
      customerEmail: filter.customerEmail,
      limit: filter.limit,
    })}`,
  );
}

/** Reads one invoice. */
export async function getInvoice(
  client: OpenCompanyClient,
  company: string | null,
  invoiceId: string,
): Promise<Invoice> {
  return client.get<Invoice>(
    `${client.scopeFor(company)}/finance/chargebee/invoices/${encodeURIComponent(invoiceId)}`,
  );
}

/** Looks a customer up by email. `null` means nobody matched — not an error. */
export async function getCustomer(
  client: OpenCompanyClient,
  company: string | null,
  email: string,
): Promise<Customer | null> {
  return client.get<Customer | null>(
    `${client.scopeFor(company)}/finance/chargebee/customers${query({ email })}`,
  );
}

/** Raises an invoice. Admin-only on the host; bills a real customer. */
export async function sendInvoice(
  client: OpenCompanyClient,
  company: string | null,
  body: SendInvoice,
): Promise<Invoice> {
  return client.post<Invoice>(
    `${client.scopeFor(company)}/finance/chargebee/invoices`,
    body,
  );
}

/** Verifies the stored Chargebee credential against the real site. */
export async function testChargebee(
  client: OpenCompanyClient,
  company: string | null,
): Promise<TestResult> {
  return client.post<TestResult>(
    `${client.scopeFor(company)}/finance/chargebee/test`,
  );
}

/** Reads the wallet balance, one entry per currency. */
export async function getBalance(
  client: OpenCompanyClient,
  company: string | null,
): Promise<Balance[]> {
  return client.get<Balance[]>(
    `${client.scopeFor(company)}/finance/paypal/balance`,
  );
}

/**
 * Lists transactions in a window.
 *
 * Both instants are required — the host does not default them, because the
 * three-hour publication lag is something the console has to *explain*, not
 * silently work around. Build the window with `defaultWindow`.
 */
export async function listTransactions(
  client: OpenCompanyClient,
  company: string | null,
  since: string,
  until: string,
  limit?: number,
): Promise<Transaction[]> {
  return client.get<Transaction[]>(
    `${client.scopeFor(company)}/finance/paypal/transactions${query({ since, until, limit })}`,
  );
}

/** Verifies the stored PayPal credential against the real environment. */
export async function testPaypal(
  client: OpenCompanyClient,
  company: string | null,
): Promise<TestResult> {
  return client.post<TestResult>(
    `${client.scopeFor(company)}/finance/paypal/test`,
  );
}
