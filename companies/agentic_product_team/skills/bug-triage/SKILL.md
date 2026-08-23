---
name: Bug Triage
description: Reproduce, deduplicate, and classify an incoming report into a bug row an engineer can pick up.
category: Product
---

# Bug Triage

Turn a raw report into a row on the bug queue — reproduced, deduplicated,
classified, and evidenced.

## When to use

- A report arrives from a customer, from support, or from monitoring.
- The queue has stale reports nobody has reproduced.

## Steps

1. **Reproduce** and capture the exact steps, the environment, and what you
   observed. A failure to reproduce is a finding: record the specific question
   that would unblock it.
2. **Deduplicate** against the `bugs` ledger before opening a row. Merge
   evidence into the existing row instead of splitting it across two.
3. **Classify** severity (impact on the user) and reach (how many) *separately*.
   Never collapse them into one number.
4. **Locate** the suspected area and state your confidence in that guess.
5. **Record** one row: repro, expected vs. actual, severity, reach, area,
   evidence.

## Output

A `bugs` row that a person who was not in the conversation can act on without a
follow-up question, meeting [[Bug triage policy]].
