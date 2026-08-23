/**
 * Pure helpers for the Finance section: money conversion, provider status
 * decoding, and the PayPal reporting window.
 *
 * A plain module with no React in it, so the unit lane (which runs without the
 * React plugin) can import the parts that are actually easy to get wrong.
 */

/**
 * How many decimal places this currency's minor unit has.
 *
 * Asked of `Intl` rather than answered from a hand-written table. Two decimals
 * is right for USD and wrong for JPY (zero) and for the three-decimal dinars,
 * and a table of exceptions in this file would be a second, staler copy of data
 * the platform already ships. An unknown code falls back to 2, which is what
 * `Intl` itself does.
 */
export function minorUnitDigits(currencyCode: string): number {
  try {
    return (
      new Intl.NumberFormat("en-US", {
        style: "currency",
        currency: currencyCode.trim().toUpperCase(),
      }).resolvedOptions().maximumFractionDigits ?? 2
    );
  } catch {
    return 2;
  }
}

/**
 * Parses an operator-typed amount into integer minor units.
 *
 * Returns `null` for anything that is not a well-formed amount in this
 * currency, so the caller can refuse rather than send a guess.
 *
 * **Deliberately string arithmetic.** `Math.round(parseFloat(x) * 100)` is the
 * obvious version and it is wrong often enough to matter: `1.005 * 100` is
 * `100.49999999999999` in IEEE 754, and `8.7 * 100` is `869.9999999999999`.
 * Splitting on the decimal point and padding the fraction cannot drift, because
 * no float is ever multiplied. This is the same concern that made the host's
 * fields `*_in_minor_units: i64` — the unit is carried so the mistake has to be
 * deliberate, and this is the last place a rounding error could reintroduce it.
 */
export function toMinorUnits(input: string, currencyCode: string): number | null {
  const digits = minorUnitDigits(currencyCode);
  // Grouping separators are allowed only in the positions a real thousand
  // separator would occupy: an operator pasting `1,250.00` off an invoice
  // means 1250 (and rejecting it teaches them nothing), but `1,25` is a typo,
  // not `125` — so the grouping is validated before it is stripped.
  const raw = input.trim();
  if (raw === "") return null;
  const match = /^(\d+|\d{1,3}(?:,\d{3})+)(?:\.(\d*))?$/.exec(raw);
  if (!match) return null;
  const [, wholeWithGrouping, fraction = ""] = match;
  const whole = wholeWithGrouping.replace(/,/g, "");
  // More precision than the currency has is a refusal, not a silent round:
  // "0.005 USD" is either a typo or a misunderstanding, and quietly making it
  // 0.01 or 0.00 hides which.
  if (fraction.length > digits) return null;
  const padded = fraction.padEnd(digits, "0");
  const minor = Number(`${whole}${padded}`);
  return Number.isSafeInteger(minor) ? minor : null;
}

/** Renders integer minor units as currency, for display only. */
export function fromMinorUnits(minor: number, currencyCode: string): string {
  const digits = minorUnitDigits(currencyCode);
  const value = minor / 10 ** digits;
  try {
    return value.toLocaleString(undefined, {
      style: "currency",
      currency: currencyCode.trim().toUpperCase(),
    });
  } catch {
    return `${value.toFixed(digits)} ${currencyCode}`;
  }
}

/**
 * Decodes PayPal's single-letter transaction status.
 *
 * A raw `V` on a row means nothing to an operator, and "Reversed" is the
 * difference between money they have and money they had.
 */
export function transactionStatus(code: string): {
  label: string;
  tone: "done" | "pending" | "failed";
} {
  switch (code.trim().toUpperCase()) {
    case "S":
      return { label: "Success", tone: "done" };
    case "P":
      return { label: "Pending", tone: "pending" };
    case "V":
      return { label: "Reversed", tone: "failed" };
    case "D":
      return { label: "Denied", tone: "failed" };
    default:
      // Shown verbatim rather than mapped to "Unknown": if PayPal adds a code,
      // an operator seeing the letter can look it up, and one seeing "Unknown"
      // cannot.
      return { label: code || "—", tone: "pending" };
  }
}

/**
 * PayPal publishes transaction data on a lag of up to **three hours** and
 * rejects any window whose end is inside that gap.
 */
export const PAYPAL_LAG_MS = 3 * 60 * 60 * 1000;

/** PayPal caps a single transaction query at 31 days. */
export const PAYPAL_MAX_SPAN_DAYS = 31;

/**
 * The default reporting window: the last 30 days, ending three hours ago.
 *
 * The clamp is why this function exists. Asking for a window that ends "now"
 * fails, and PayPal's own message for it says only that data "is not
 * available" — which reads as "you had no transactions" rather than "move the
 * end date back". The host rewrites that message
 * (`explain_unavailable_window`); this stops the operator from hitting it on
 * their first click at all.
 *
 * 30 days, not 31, so the span is inside PayPal's cap with a day to spare
 * rather than exactly on it.
 */
export function defaultWindow(now: Date): { since: string; until: string } {
  const until = new Date(now.getTime() - PAYPAL_LAG_MS);
  const since = new Date(until.getTime() - 30 * 24 * 60 * 60 * 1000);
  return { since: since.toISOString(), until: until.toISOString() };
}

/** The latest instant PayPal will accept as a window end. */
export function latestSelectableEnd(now: Date): Date {
  return new Date(now.getTime() - PAYPAL_LAG_MS);
}

/**
 * Why a window would be refused, or `null` when it is fine.
 *
 * Checked before the request so the operator is told by the control they just
 * moved, rather than by a round trip that comes back as a provider error.
 */
export function windowProblem(
  since: string,
  until: string,
  now: Date,
): string | null {
  const from = new Date(since).getTime();
  const to = new Date(until).getTime();
  if (Number.isNaN(from) || Number.isNaN(to)) return "Enter both dates.";
  if (to <= from) return "The end of the range must come after the start.";
  if (to > latestSelectableEnd(now).getTime())
    return "PayPal publishes transactions up to 3 hours late, so the range must end at least 3 hours ago.";
  if (to - from > PAYPAL_MAX_SPAN_DAYS * 24 * 60 * 60 * 1000)
    return `PayPal allows at most ${PAYPAL_MAX_SPAN_DAYS} days in one query.`;
  return null;
}

/**
 * A human label for a Chargebee invoice status.
 *
 * `payment_due` is the one that matters: it is the difference between an
 * invoice that is finished and one that is owed, and the raw token buries that
 * under an underscore.
 */
export function invoiceStatus(status: string): {
  label: string;
  tone: "done" | "pending" | "failed" | "muted";
} {
  switch (status.trim().toLowerCase()) {
    case "paid":
      return { label: "Paid", tone: "done" };
    case "payment_due":
      return { label: "Payment due", tone: "pending" };
    case "not_paid":
      return { label: "Not paid", tone: "failed" };
    case "posted":
      return { label: "Posted", tone: "pending" };
    case "pending":
      return { label: "Pending", tone: "pending" };
    case "voided":
      return { label: "Voided", tone: "muted" };
    default:
      return { label: status || "—", tone: "muted" };
  }
}
