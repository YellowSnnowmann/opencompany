# Discussions

How this repository's Discussions are organized, and how a thread moves through
them. Written for maintainers doing triage; contributors only need
[SUPPORT.md](../SUPPORT.md).

## Why not the default six

GitHub ships every repository the same categories — Announcements, General,
Ideas, Polls, Q&A, Show and tell. They sort posts by *what kind of speech act*
a post is, which is a distinction nobody needs when they are stuck: the person
whose container will not boot and the person whose agent looped are both "Q&A",
and both land in the same undifferentiated pile.

Categories here sort by **what the poster is doing**, because that is what
decides who can answer and what evidence is required. General stays as the
catch-all; Ideas and Polls are retired into RFCs and Announcements.

## Categories

| Category | Slug | Answerable | For |
| --- | --- | --- | --- |
| Announcements | `announcements` | no | Releases, breaking changes, anything a running company must act on. Maintainers post; anyone comments. |
| Running a company | `running-a-company` | yes | `company.toml`, roles, approvals, workflows — using the product rather than operating the server. |
| Agents & runs | `agents-and-runs` | yes | A run that stalled, steering, planning, the OpenHuman launcher, TinyAgents. |
| Storage & tenancy | `storage-and-tenancy` | yes | Backends, data directories, db-per-tenant and shared-single-DB, exports and bundles. |
| Self-hosting | `self-hosting` | yes | Docker, the entrypoint, reverse proxies, TLS, `/healthz`, wake-on-request. |
| RFCs | `rfcs` | no | Changes big enough that finding out at PR review is too late. |
| Q&A | `q-a` | yes | Everything else someone is stuck on. |
| Show your company | `show-and-tell` | no | What people are running on this. |
| General | `general` | no | The catch-all. Triage moves posts out of it. |

Five of these have a form under
[`.github/DISCUSSION_TEMPLATE/`](../../.github/DISCUSSION_TEMPLATE). **The file
name must equal the category slug** — a template whose slug does not match a
real category is silently ignored, which is the failure mode to check first if a
form stops appearing.

## Labels

Discussion labels are the repository's issue labels; these are the ones triage
depends on.

| Label | Meaning | Who sets it |
| --- | --- | --- |
| `awaiting maintainer` | Has replies, none from a maintainer. The triage queue. | Triage |
| `needs repro` | Cannot be acted on until someone reproduces it. | Anyone |
| `needs logs` | The container log or run journal is missing. | Anyone |
| `breaking` | Describes or proposes a change that breaks existing companies. | Maintainer |
| `good first issue` | A newcomer could close this. Mirrors the issue label. | Maintainer |
| `promoted` | An issue was opened from this thread; the issue is linked. | Maintainer |

## Triage

Run over the queue rather than the feed. In order:

1. **Wrong category** — move it. A misfiled thread is answered by nobody.
2. **`awaiting maintainer`, oldest first.** A thread with four community replies
   and no maintainer is the failure this queue exists to catch; "unanswered"
   does not show it, because those replies count as answers.
3. **Answerable and answered** — mark the answer. An unmarked answer means the
   next person with the same problem posts again.
4. **A bug** — open the issue, link both ways, label the thread `promoted`, and
   leave the thread open until the fix ships. Do not ask the reporter to file it
   again.
5. **An RFC that is settled** — record the decision as a comment and mark it the
   answer, then close. A decision nobody can find gets relitigated.

Threads are not closed for being old. A stale answered thread is documentation;
a stale unanswered one is a queue item that was missed.

## Cross-posting with OpenHuman

OpenHuman is the runtime inside OpenCompany, so a real question often belongs to
both repositories: the launcher, the agent journal, the workspace root.

Post it where the person is running into it, and link the counterpart rather
than closing it as a duplicate — the two repositories have different maintainers
and a link keeps both able to answer. When the fix lands in the other
repository, say so in the thread and mark it answered here.

## Changing this setup

Categories cannot be created from the API — there is no GraphQL mutation for
them, so they are made in **Settings → Discussions** by someone with admin on
the repository. Anything added there needs a row in the table above, and a form
under `.github/DISCUSSION_TEMPLATE/` if the category expects evidence.
