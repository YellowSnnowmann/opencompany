# Bug triage policy

The rules a bug row is held to, so severity means the same thing on Monday as it
did last month.

## Severity is impact, reach is count

They are recorded separately and never collapsed. A cosmetic bug affecting
everyone is not a P1; data loss for one customer is.

| Severity | Means |
| --- | --- |
| `critical` | Data loss, security exposure, or unusable with no workaround. |
| `high` | A core flow is broken and the workaround is one users will not find. |
| `medium` | Wrong, with a workaround people can actually use. |
| `low` | Cosmetic, or a rare edge nobody has hit. |

When the argument is between two levels, write the argument into the row and
take the lower one.

## Every claim cites something

A log line, a request id, a screenshot, a support thread. A row that says "users
report slowness" and cites nothing is a rumor with a ticket number.

## Cannot-reproduce is not a close

First pass: record what was tried and the one question that would unblock it,
status `needs-info`. Only close for good after that question has gone
unanswered — with the reason and the date.

## Duplicates merge, they do not stack

Fold the evidence into the existing row. Two rows for one problem make the
problem look half as severe as it is.

See also [[Definition of ready]].
