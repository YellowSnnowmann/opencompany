# Conversation — the single-agent chat

`#/conversation` is the WhatsApp-style two-pane surface: the company's chats on
the left, one transcript on the right. `views/Conversation.tsx` composes it;
everything in this folder is a piece of it.

The transcript half runs on **[assistant-ui](https://www.assistant-ui.com)**
(`@assistant-ui/react`). That is the only third-party library in the chat path,
and what follows is the shape of the seam — what it owns, what it does not, and
why each of those is where it is.

## What assistant-ui owns

Composer state and submission, Enter/Shift+Enter, textarea autosize and focus
handling; the scroll viewport, its auto-scroll and its scroll-to-bottom
affordance; and the run state that gates sending while a turn is open. None of
that is code this repo writes or tests any more.

## What it does not own

**The messages.** They live in `AppShell`, and they have to: a reply can arrive
over SSE while this view is unmounted, and a turn can outlive the request that
started it (issue #983). A runtime that owned the array would be a second copy
of the truth with no way to hear either event.

That is the whole reason this surface uses the **external-store** runtime —
`useExternalStoreRuntime`, in `runtime.ts`. It is the one runtime that takes the
message array as an *input*. The library renders; the shell stays the single
source of truth for what was said.

**The transport.** Unchanged, and deliberately so. Sending is still
`client.chat(...)` — `POST …/chat` on the Rust host — with the company event
stream carrying live frames. Nothing here speaks a data-stream protocol, and
`@assistant-ui/ai-sdk` is not a dependency.

**Anything around the transcript.** The left list is the company's desks and
teammates, built by `lib/threads.ts` from the host's roster and addressed by
desk/agent id — so it is a plain list (`ThreadList.tsx`), not a
`ThreadListPrimitive`, which models an assistant's *saved conversations* and
would want to create, rename and archive ids that are not its to mint. The strip
above the composer (`InflightStrip.tsx`) steers **named** runs — pause,
redirect, cancel a dispatched task (issue #111) — which is why the runtime is
handed no `onCancel`: abandoning the last reply is not the action an operator
wants here.

`onEdit`, `onReload` and `setMessages` are withheld for the plainer reason that
this host has no edit, regenerate or branch semantics. Withholding the callback
is how assistant-ui is told not to offer the affordance.

## The three send outcomes

`onNew` in `runtime.ts` reports one of three things to the shell, and the
distinction is load-bearing:

| outcome | what happened | callback |
| --- | --- | --- |
| resolved | the reply came back in the POST body and is on screen | `onSendEnd` |
| detached | the host answered `202`; the turn continues on the stream (#983) | `onSendDetached` |
| failed | the POST threw, and the turn probably outlived it (#1000) | `onSendFailed` |

Only `resolved` licenses the shell to drop the live frame it was holding. A
`failed` send rendered nothing, so that frame is the only copy of the answer
this console will be handed — reporting it as `onSendEnd` loses the reply.

## Grouping is decided before assistant-ui sees the list

`ThreadPrimitive.Messages` hands the render function one message at a time, so a
line's neighbours are not visible at render. Everything that depends on them —
who a line is from, whether it opens or closes a group, whether a day separator
goes above it — is decided up front by `decorate()` in `model.ts`, which returns
one `ConversationLine` per message, index-aligned with its input. `convertMessage`
then looks a line up by the index assistant-ui passes it.

The decorated line rides across the boundary in `metadata.custom` and comes back
out through `lineOf()`. Nothing is lossy, so the renderer can always reach the
original `ChatMessage` — which is what the board-card actions and the
markdown-vs-plain-text split need.

Two consequences worth knowing:

- **A message with no line is not ours.** The external-store runtime inserts an
  empty assistant placeholder while a run is open (`metadata.isOptimistic`).
  `lineOf` returns `null` for it and `Transcript.tsx` draws nothing, because
  this surface keeps its own working row — one that survives a trailing
  assistant message, which the placeholder does not, and which is what a
  detached turn needs.
- **The pure half is unit-tested**, in `test/unit/conversation-model.test.ts`.
  Grouping used to be observable only on screen; it is now assertable in
  milliseconds.

## The files

| file | what it is |
| --- | --- |
| `model.ts` | pure: sender resolution, `decorate`, the assistant-ui conversion, formatting, tones |
| `runtime.ts` | the external-store runtime, the send path, and the board-card writes (#246, #984) |
| `Transcript.tsx` | the viewport, the per-message render function, and the composer |
| `parts.tsx` | avatars, day separator, step timeline, bubbles, card chip, "Add to board" |
| `ThreadList.tsx` | the left pane |
| `InflightStrip.tsx` | the steer strip between transcript and composer (#111) |

## What the e2e specs drive

`chat-to-card`, `wiring`, `mcp-agent` and `composio-account-choice` reach this
surface through selectors the rewrite deliberately preserved: the `<aside>` chat
list, the `Message <name>…` placeholder, the button labelled exactly `Send`, the
`div.group/msg` row around each bubble, and the `Couldn't send — …` system line.
Changing any of those is changing those specs.
