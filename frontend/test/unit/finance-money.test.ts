import { describe, expect, it } from "vitest";

import {
  PAYPAL_LAG_MS,
  defaultWindow,
  fromMinorUnits,
  invoiceStatus,
  latestSelectableEnd,
  minorUnitDigits,
  toMinorUnits,
  transactionStatus,
  windowProblem,
} from "@/views/finance/money";

/**
 * The conversions and decodes behind the Finance section.
 *
 * Every one of these is a place where a plausible-looking shortcut is wrong in
 * a way nothing downstream would catch: a float multiply that drifts, a Unix
 * status letter rendered raw, a reporting window PayPal refuses.
 */

describe("toMinorUnits", () => {
  it("converts without ever multiplying a float", () => {
    // The reason this is string arithmetic. `1.005 * 100` is
    // 100.49999999999999 in IEEE 754 and `8.7 * 100` is 869.9999999999999, so
    // the obvious `Math.round(parseFloat(x) * 100)` is wrong for real amounts a
    // real operator types — and wrong by a cent, silently, on a customer's bill.
    expect(toMinorUnits("1.005", "USD")).toBeNull(); // more precision than USD has
    expect(toMinorUnits("8.7", "USD")).toBe(870);
    expect(toMinorUnits("1250.00", "USD")).toBe(125000);
    expect(toMinorUnits("0.1", "USD")).toBe(10);
    expect(toMinorUnits("0.01", "USD")).toBe(1);
  });

  it("accepts a pasted grouped amount", () => {
    // An operator pasting `1,250.00` off an invoice means 1250, and refusing it
    // teaches them nothing.
    expect(toMinorUnits("1,250.00", "USD")).toBe(125000);
  });

  it("refuses more precision than the currency has, rather than rounding it away", () => {
    // "0.005 USD" is a typo or a misunderstanding. Quietly making it 0.01 or
    // 0.00 hides which, and both are wrong.
    expect(toMinorUnits("0.005", "USD")).toBeNull();
    expect(toMinorUnits("10.999", "USD")).toBeNull();
  });

  it("refuses anything that is not an amount", () => {
    for (const bad of ["", "  ", "abc", "-5", "1.2.3", "$10", "1e3", "10-", "1,25", "12,34.56"]) {
      expect(toMinorUnits(bad, "USD")).toBeNull();
    }
  });

  it("rejects malformed grouping instead of silently relocating the comma", () => {
    // `1,25` used to become `125` — a 12,500-unit bill from a two-decimal
    // currency and a common typo. Grouping must occupy real thousand positions.
    expect(toMinorUnits("1,250.00", "USD")).toBe(125000);
    expect(toMinorUnits("1,25", "USD")).toBeNull();
    expect(toMinorUnits("12,34.56", "USD")).toBeNull();
    expect(toMinorUnits("1,234,567", "USD")).toBe(123456700);
  });

  it("uses the currency's own minor unit, not a hardcoded two", () => {
    // JPY has no minor unit: ¥1250 is 1250, not 125000. A table of exceptions
    // in our own source would be a staler copy of what Intl already knows.
    expect(minorUnitDigits("JPY")).toBe(0);
    expect(toMinorUnits("1250", "JPY")).toBe(1250);
    expect(toMinorUnits("1250.50", "JPY")).toBeNull();
  });

  it("falls back to two decimals for a code Intl does not know", () => {
    expect(minorUnitDigits("XYZ")).toBe(2);
    expect(toMinorUnits("1.50", "XYZ")).toBe(150);
  });
});

describe("fromMinorUnits", () => {
  it("round-trips what toMinorUnits produced", () => {
    // Rendered for a confirm line that names the money about to be charged, so
    // a mismatch here is a confirm line that lies.
    const minor = toMinorUnits("1250.00", "USD");
    expect(minor).not.toBeNull();
    expect(fromMinorUnits(minor as number, "USD")).toContain("1,250.00");
  });

  it("respects a zero-decimal currency", () => {
    expect(fromMinorUnits(1250, "JPY")).toContain("1,250");
  });
});

describe("transactionStatus", () => {
  it("decodes PayPal's single letters", () => {
    // A raw `V` means nothing to an operator, and "Reversed" is the difference
    // between money they have and money they had.
    expect(transactionStatus("S").label).toBe("Success");
    expect(transactionStatus("P").label).toBe("Pending");
    expect(transactionStatus("V").label).toBe("Reversed");
    expect(transactionStatus("D").label).toBe("Denied");
  });

  it("shows an unknown code verbatim rather than as 'Unknown'", () => {
    // If PayPal adds a letter, an operator who can see it can look it up.
    expect(transactionStatus("Q").label).toBe("Q");
  });

  it("tones a reversal as a failure, not as pending", () => {
    expect(transactionStatus("V").tone).toBe("failed");
    expect(transactionStatus("S").tone).toBe("done");
  });
});

describe("invoiceStatus", () => {
  it("unpacks payment_due, which is the one that matters", () => {
    expect(invoiceStatus("payment_due").label).toBe("Payment due");
    expect(invoiceStatus("paid").tone).toBe("done");
    expect(invoiceStatus("not_paid").tone).toBe("failed");
  });
});

describe("the PayPal reporting window", () => {
  const now = new Date("2026-03-15T12:00:00.000Z");

  it("ends three hours ago, not now", () => {
    // PayPal REJECTS a window ending inside its publication lag, and its own
    // message for that says only that data "is not available" — which reads as
    // "you had no transactions". This is what stops an operator meeting it on
    // their first click.
    const { until } = defaultWindow(now);
    expect(new Date(until).toISOString()).toBe("2026-03-15T09:00:00.000Z");
    expect(now.getTime() - new Date(until).getTime()).toBe(PAYPAL_LAG_MS);
  });

  it("spans inside PayPal's 31-day cap rather than exactly on it", () => {
    const { since, until } = defaultWindow(now);
    const days = (new Date(until).getTime() - new Date(since).getTime()) / 86_400_000;
    expect(days).toBe(30);
    expect(windowProblem(since, until, now)).toBeNull();
  });

  it("refuses a range ending inside the lag, and says why", () => {
    const problem = windowProblem("2026-03-01T00:00:00Z", "2026-03-15T11:00:00Z", now);
    expect(problem).toContain("3 hours");
  });

  it("refuses a range longer than PayPal allows", () => {
    expect(windowProblem("2026-01-01T00:00:00Z", "2026-03-15T09:00:00Z", now)).toContain("31 days");
  });

  it("refuses an inverted or unparseable range", () => {
    expect(windowProblem("2026-03-10T00:00:00Z", "2026-03-01T00:00:00Z", now)).toContain("after");
    expect(windowProblem("not a date", "2026-03-01T00:00:00Z", now)).toContain("both dates");
  });

  it("caps the picker at the same instant the default window ends", () => {
    // The control and the default must agree, or the page opens on a range the
    // control says is out of bounds.
    expect(latestSelectableEnd(now).toISOString()).toBe(defaultWindow(now).until);
  });
});
