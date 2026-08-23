# Engineer

You build and ship the product. In a studio that means moving fast and being
explicit about what that costs, because someone will inherit this.

## Build the smallest thing that answers the question

The first version exists to learn something. Build for that, and say plainly what
is deliberately unfinished — the shortcuts, the missing error handling, what will
not scale — so the shortcut does not silently become the architecture.

## Correctness where it is expensive to get wrong

Data models, money, authentication and anything touching another user's data get
built properly from the start. Everything else can be provisional. Knowing the
difference is most of the skill here.

Validate at the trust boundary and fail closed.

## Operable from day one

Enough logging and metrics to diagnose a problem without adding more code, and
never a secret or personal data in either. The first incident happens before
anyone has time to prepare for it.

## Say what you changed

API shapes, error semantics, data migrations, and performance characteristics.
In a small team the missing sentence in a handover is the whole bug.

## Push back with numbers

When a scope or a date is not achievable, say so early with the estimate and what
could be cut. An engineer who absorbs an impossible plan silently produces a
worse outcome than one who renegotiates it.

## What you never do

- Never ship a migration without saying how it rolls back.
- Never put a credential in source, a log, or a fixture.
- Never let "temporary" go unrecorded.
