# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: orchestration-simulation.spec.ts >> a goal becomes delegated cards, the team works them, and review closes them
- Location: test/e2e/orchestration-simulation.spec.ts:97:1

# Error details

```
Error: card "sim gather the sources 1789766774798" was never accepted

card "sim gather the sources 1789766774798" was never accepted

expect(received).toBe(expected) // Object.is equality

Expected: "done"
Received: "working"

Call Log:
- Timeout 180000ms exceeded while waiting on the predicate
```

# Page snapshot

```yaml
- generic [ref=e2]:
  - generic [ref=e4]:
    - link "Skip to content" [ref=e5] [cursor=pointer]:
      - /url: "#main-content"
    - generic [ref=e6]:
      - button "E2E Harness Co" [ref=e8]:
        - generic [ref=e10]: E2E Harness Co
        - img [ref=e11]
      - button "Collapse sidebar" [ref=e15]:
        - img [ref=e16]
      - button "Search" [ref=e20]:
        - img [ref=e21]
        - generic [ref=e24]: Search
        - generic [ref=e25]: Ctrl K
      - generic [ref=e26]:
        - button "Overview" [ref=e28]:
          - img [ref=e29]
        - button "Notifications" [ref=e34]:
          - img [ref=e35]
        - button "Settings" [ref=e38]:
          - img [ref=e39]
        - link "Join our Discord" [ref=e42] [cursor=pointer]:
          - /url: https://discord.tinyhumans.ai
          - img [ref=e43]
      - button "Harness E2e" [ref=e46]:
        - generic [ref=e48]: HE
    - generic [ref=e49]:
      - generic [ref=e52]:
        - navigation "Main navigation" [ref=e53]:
          - generic [ref=e54]:
            - list [ref=e56]:
              - listitem [ref=e57]:
                - button "Room" [ref=e58]:
                  - img [ref=e59]
                  - generic [ref=e62]: Room
              - listitem [ref=e63]:
                - button "Company" [ref=e64]:
                  - img [ref=e65]
                  - generic [ref=e70]: Company
              - listitem [ref=e71]:
                - button "Connections" [ref=e72]:
                  - img [ref=e73]
                  - generic [ref=e75]: Connections
              - listitem [ref=e76]:
                - button "Automations" [ref=e77]:
                  - img [ref=e78]
                  - generic [ref=e82]: Automations
            - complementary [ref=e85]:
              - generic [ref=e86]:
                - generic [ref=e87]:
                  - button "Channels" [expanded] [ref=e88]:
                    - img [ref=e89]
                    - generic [ref=e91]: Channels
                  - button "New channel" [ref=e92]:
                    - img [ref=e93]
                - list [ref=e94]:
                  - listitem [ref=e95]:
                    - button "engineering-desk" [ref=e96]:
                      - img [ref=e97]
                      - generic [ref=e100]: engineering-desk
                  - listitem [ref=e101]:
                    - button "content-desk" [ref=e102]:
                      - img [ref=e103]
                      - generic [ref=e106]: content-desk
                  - listitem [ref=e107]:
                    - button "legal" [ref=e108]:
                      - img [ref=e109]
                      - generic [ref=e112]: legal
              - generic [ref=e113]:
                - generic [ref=e114]:
                  - button "Direct messages" [expanded] [ref=e115]:
                    - img [ref=e116]
                    - generic [ref=e118]: Direct messages
                  - button "New message" [ref=e119]:
                    - img [ref=e120]
                - list [ref=e123]:
                  - listitem [ref=e124]:
                    - button "Chief Executive" [ref=e125]:
                      - generic [ref=e127]: CE
                      - generic [ref=e128]: Chief Executive
                  - listitem [ref=e129]:
                    - button "Engineer" [ref=e130]:
                      - generic [ref=e132]: E
                      - generic [ref=e133]: Engineer
                  - listitem [ref=e134]:
                    - button "Operations" [ref=e135]:
                      - generic [ref=e137]: O
                      - generic [ref=e138]: Operations
                  - listitem [ref=e139]:
                    - button "Page Builder" [ref=e140]:
                      - generic [ref=e142]: PB
                      - generic [ref=e143]: Page Builder
                  - listitem [ref=e144]:
                    - button "Researcher" [ref=e145]:
                      - generic [ref=e147]: R
                      - generic [ref=e148]: Researcher
                  - listitem [ref=e149]:
                    - button "Writer" [ref=e150]:
                      - generic [ref=e152]: W
                      - generic [ref=e153]: Writer
              - button "Operator" [ref=e155]:
                - img [ref=e156]
                - generic [ref=e162]: Operator
        - button "Toggle Sidebar" [ref=e163]
      - main [ref=e164]:
        - generic [ref=e170]:
          - generic [ref=e171]:
            - generic [ref=e172]:
              - img [ref=e173]
              - heading "engineering-desk" [level=1] [ref=e176]
              - 'button "Copy channel name: #engineering-desk" [ref=e177]':
                - img
              - generic [ref=e178]: How things are built and technical plans.
            - button "1 Show agents" [ref=e179]:
              - img
              - generic [ref=e180]: "1"
              - generic [ref=e181]: Show agents
          - generic [ref=e183]:
            - generic [ref=e185]:
              - generic [ref=e186]:
                - img [ref=e188]
                - heading "#engineering-desk" [level=2] [ref=e191]
                - paragraph [ref=e192]: "This is the very beginning of #engineering-desk. How things are built and technical plans."
              - generic [ref=e193]:
                - generic "Today":
                  - paragraph: Today
                - article [ref=e194]:
                  - generic [ref=e197]: "Y"
                  - generic [ref=e198]:
                    - generic [ref=e199]:
                      - generic [ref=e200]: You
                      - generic [ref=e201]: 9:19 PM
                    - paragraph [ref=e203]: note-attach-mu7go3jj
                    - button "q3-notes-mu7go3jj.md 54 B" [ref=e206]:
                      - img [ref=e207]
                      - generic [ref=e210]: q3-notes-mu7go3jj.md
                      - generic [ref=e211]: 54 B
                      - img [ref=e212]
              - article [ref=e216]:
                - button "Open Engineer's profile" [ref=e218]:
                  - generic [ref=e220]: E
                - generic [ref=e221]:
                  - generic [ref=e222]:
                    - button "Engineer" [ref=e223]
                    - generic [ref=e224]: 9:19 PM
                  - paragraph [ref=e226]:
                    - strong [ref=e227]: MOCK_LLM
                    - text: mock inference backend reply.
              - article [ref=e229]:
                - generic [ref=e232]: "Y"
                - generic [ref=e233]:
                  - generic [ref=e234]:
                    - generic [ref=e235]: You
                    - generic [ref=e236]: 9:19 PM
                  - paragraph [ref=e238]: seeded-attach-mu7go85m
                  - button "roadmap-mu7go85m.md 35 B" [ref=e241]:
                    - img [ref=e242]
                    - generic [ref=e245]: roadmap-mu7go85m.md
                    - generic [ref=e246]: 35 B
                    - img [ref=e247]
              - article [ref=e251]:
                - button "Open Engineer's profile" [ref=e253]:
                  - generic [ref=e255]: E
                - generic [ref=e256]:
                  - generic [ref=e257]:
                    - button "Engineer" [ref=e258]
                    - generic [ref=e259]: 9:19 PM
                  - paragraph [ref=e261]:
                    - strong [ref=e262]: MOCK_LLM
                    - text: mock inference backend reply.
              - article [ref=e264]:
                - generic [ref=e267]: "Y"
                - generic [ref=e268]:
                  - generic [ref=e269]:
                    - generic [ref=e270]: You
                    - generic [ref=e271]: 9:19 PM
                  - paragraph [ref=e273]: paperclip-attach-mu7go8v3
                  - button "hero-mu7go8v3.png 70 B" [ref=e276]:
                    - img [ref=e277]
                    - generic [ref=e280]: hero-mu7go8v3.png
                    - generic [ref=e281]: 70 B
                    - img [ref=e282]
              - article [ref=e286]:
                - button "Open Engineer's profile" [ref=e288]:
                  - generic [ref=e290]: E
                - generic [ref=e291]:
                  - generic [ref=e292]:
                    - button "Engineer" [ref=e293]
                    - generic [ref=e294]: 9:19 PM
                  - paragraph [ref=e296]:
                    - strong [ref=e297]: MOCK_LLM
                    - text: mock inference backend reply.
              - article [ref=e299]:
                - generic [ref=e302]: "Y"
                - generic [ref=e303]:
                  - generic [ref=e304]:
                    - generic [ref=e305]: You
                    - generic [ref=e306]: 9:19 PM
                  - paragraph [ref=e308]: huge-attach-mu7goad7
                  - button "huge-mu7goad7.md 5.0 MB" [ref=e311]:
                    - img [ref=e312]
                    - generic [ref=e315]: huge-mu7goad7.md
                    - generic [ref=e316]: 5.0 MB
                    - img [ref=e317]
              - article [ref=e321]:
                - button "Open Engineer's profile" [ref=e323]:
                  - generic [ref=e325]: E
                - generic [ref=e326]:
                  - generic [ref=e327]:
                    - button "Engineer" [ref=e328]
                    - generic [ref=e329]: 9:19 PM
                  - paragraph [ref=e331]:
                    - strong [ref=e332]: MOCK_LLM
                    - text: mock inference backend reply.
              - generic [ref=e335]:
                - link "finished → In review" [ref=e336] [cursor=pointer]:
                  - /url: "#/company/tasks/01a0b663c074-000000000127"
                - button "Approve" [ref=e337]
              - article [ref=e339]:
                - button "Open Ceo's profile" [ref=e341]:
                  - generic [ref=e343]: C
                - generic [ref=e344]:
                  - generic [ref=e345]:
                    - button "Ceo" [ref=e346]
                    - generic [ref=e347]: 9:19 PM
                  - generic [ref=e348]:
                    - paragraph [ref=e349]: "\"dispatch-marker 1789766385771\" is ready for review."
                    - paragraph [ref=e350]:
                      - strong [ref=e351]: MOCK_LLM
                      - text: mock inference backend reply.
              - article [ref=e353]:
                - generic [ref=e356]: "Y"
                - generic [ref=e357]:
                  - generic [ref=e358]:
                    - generic [ref=e359]: You
                    - generic [ref=e360]: 9:20 PM
                  - paragraph [ref=e362]: build the launch checklist SPAWNONE 1789766451575
              - article [ref=e364]:
                - button "Open Engineer's profile" [ref=e366]:
                  - generic [ref=e368]: E
                - generic [ref=e369]:
                  - generic [ref=e370]:
                    - button "Engineer" [ref=e371]
                    - generic [ref=e372]: 9:20 PM
                  - paragraph [ref=e374]:
                    - strong [ref=e375]: MOCK_LLM
                    - text: "Queued a task card: \"build the launch checklist 1789766451575\". It will be opened on the board this turn."
                  - generic [ref=e377]:
                    - link "Card opened" [ref=e378] [cursor=pointer]:
                      - /url: "#/company/tasks/01a0b664c1e6-000000000190"
                      - img [ref=e379]
                      - text: Card opened
                    - button "Dismiss this card" [ref=e381]:
                      - img [ref=e382]
              - article [ref=e386]:
                - generic [ref=e389]: "Y"
                - generic [ref=e390]:
                  - generic [ref=e391]:
                    - generic [ref=e392]: You
                    - generic [ref=e393]: 9:21 PM
                  - paragraph [ref=e395]: please track this SPAWNONE 1789766461768
              - article [ref=e397]:
                - button "Open Engineer's profile" [ref=e399]:
                  - generic [ref=e401]: E
                - generic [ref=e402]:
                  - generic [ref=e403]:
                    - button "Engineer" [ref=e404]
                    - generic [ref=e405]: 9:21 PM
                  - paragraph [ref=e407]:
                    - strong [ref=e408]: MOCK_LLM
                    - text: "Queued a task card: \"please track this 1789766461768\". It will be opened on the board this turn."
              - article [ref=e410]:
                - generic [ref=e413]: "Y"
                - generic [ref=e414]:
                  - generic [ref=e415]:
                    - generic [ref=e416]: You
                    - generic [ref=e417]: 9:21 PM
                  - paragraph [ref=e419]: dismiss this one SPAWNONE 1789766470613
              - article [ref=e421]:
                - button "Open Engineer's profile" [ref=e423]:
                  - generic [ref=e425]: E
                - generic [ref=e426]:
                  - generic [ref=e427]:
                    - button "Engineer" [ref=e428]
                    - generic [ref=e429]: 9:21 PM
                  - paragraph [ref=e431]:
                    - strong [ref=e432]: MOCK_LLM
                    - text: "Queued a task card: \"dismiss this one 1789766470613\". It will be opened on the board this turn."
                  - generic [ref=e434]:
                    - link "Card opened" [ref=e435] [cursor=pointer]:
                      - /url: "#/company/tasks/01a0b6650cef-0000000001c8"
                      - img [ref=e436]
                      - text: Card opened
                    - button "Dismiss this card" [ref=e438]:
                      - img [ref=e439]
              - article [ref=e443]:
                - generic [ref=e446]: "Y"
                - generic [ref=e447]:
                  - generic [ref=e448]:
                    - generic [ref=e449]: You
                    - generic [ref=e450]: 9:21 PM
                  - paragraph [ref=e452]:
                    - strong [ref=e453]: MOCK_TOOL_CALL
                    - text: "{\"name\":\"composio_execute\",\"arguments\":{\"tool\":\"GMAIL_SEND_EMAIL\",\"arguments\":{\"to\":\""
                    - link "0c738ec2-4bb5-4f26-ba16-1d1a9ed21a0c@acme.test" [ref=e454] [cursor=pointer]:
                      - /url: mailto:0c738ec2-4bb5-4f26-ba16-1d1a9ed21a0c@acme.test
                    - text: "\",\"subject\":\"e2e\",\"body\":\"e2e\"}}}"
              - article [ref=e456]:
                - button "Open Engineer's profile" [ref=e458]:
                  - generic [ref=e460]: E
                - generic [ref=e461]:
                  - generic [ref=e462]:
                    - button "Engineer" [ref=e463]
                    - generic [ref=e464]: 9:21 PM
                  - paragraph [ref=e466]:
                    - strong [ref=e467]: MOCK_LLM
                    - text: "{ \"data\": { \"actedAs\": null, \"tool\": \"GMAIL_SEND_EMAIL\" }, \"successful\": true, \"error\": null, \"costUsd\": 0.0, \"markdownFormatted\": null }"
              - article [ref=e469]:
                - generic [ref=e472]: "Y"
                - generic [ref=e473]:
                  - generic [ref=e474]:
                    - generic [ref=e475]: You
                    - generic [ref=e476]: 9:22 PM
                  - paragraph [ref=e478]:
                    - strong [ref=e479]: MOCK_TOOL_CALL
                    - text: "{\"name\":\"composio_execute\",\"arguments\":{\"tool\":\"GMAIL_SEND_EMAIL\",\"arguments\":{\"to\":\""
                    - link "595d3ffc-e082-4a4d-8f29-5586b8bf8520@acme.test" [ref=e480] [cursor=pointer]:
                      - /url: mailto:595d3ffc-e082-4a4d-8f29-5586b8bf8520@acme.test
                    - text: "\",\"subject\":\"e2e\",\"body\":\"e2e\"}}}"
              - article [ref=e482]:
                - button "Open Engineer's profile" [ref=e484]:
                  - generic [ref=e486]: E
                - generic [ref=e487]:
                  - generic [ref=e488]:
                    - button "Engineer" [ref=e489]
                    - generic [ref=e490]: 9:22 PM
                  - paragraph [ref=e492]:
                    - strong [ref=e493]: MOCK_LLM
                    - text: "{ \"data\": { \"actedAs\": \"ca_billing\", \"tool\": \"GMAIL_SEND_EMAIL\" }, \"successful\": true, \"error\": null, \"costUsd\": 0.0, \"markdownFormatted\": null }"
              - article [ref=e495]:
                - generic [ref=e498]: "Y"
                - generic [ref=e499]:
                  - generic [ref=e500]:
                    - generic [ref=e501]: You
                    - generic [ref=e502]: 9:25 PM
                  - paragraph [ref=e504]:
                    - strong [ref=e505]: MOCK_TOOL_CALL
                    - text: "{\"name\":\"mcp_call_tool\",\"arguments\":{\"server\":\"pw-agent-mcp-ff03429b-fb0c-49f2-912a-52b9c652aefe\",\"tool\":\"echo\",\"arguments\":{\"text\":\"agent-mcp-60bc6de1-2e5e-47b2-987d-0b612d24235b\"}}}"
              - article [ref=e507]:
                - button "Open Engineer's profile" [ref=e509]:
                  - generic [ref=e511]: E
                - generic [ref=e512]:
                  - generic [ref=e513]:
                    - button "Engineer" [ref=e514]
                    - generic [ref=e515]: 9:25 PM
                  - paragraph [ref=e517]:
                    - strong [ref=e518]: MOCK_LLM
                    - text: "echo: agent-mcp-60bc6de1-2e5e-47b2-987d-0b612d24235b"
              - article [ref=e520]:
                - generic [ref=e523]: "Y"
                - generic [ref=e524]:
                  - generic [ref=e525]:
                    - generic [ref=e526]: You
                    - generic [ref=e527]: 9:26 PM
                  - paragraph [ref=e529]:
                    - text: "Ship a short market digest this week: find what is being said and write it up."
                    - strong [ref=e530]: MOCK_PLAN
                    - text: "[[{\"name\":\"spawn_task\",\"arguments\":{\"title\":\"sim gather the sources 1789766774798\",\"note\":\"Search and collect what is current, newest first.\",\"assignee\":\"engineer\"}},{\"name\":\"spawn_task\",\"arguments\":{\"title\":\"sim write the digest 1789766774798\",\"note\":\"Turn the collected sources into a short digest with links.\",\"assignee\":\"writer\"}}],[]] goal-1789766774798"
              - article [ref=e532]:
                - button "Open Engineer's profile" [ref=e534]:
                  - generic [ref=e536]: E
                - generic [ref=e537]:
                  - generic [ref=e538]:
                    - button "Engineer" [ref=e539]
                    - generic [ref=e540]: 9:26 PM
                  - paragraph [ref=e542]:
                    - strong [ref=e543]: MOCK_LLM
                    - text: "Queued a task card: \"sim write the digest 1789766774798\". It will be opened on the board this turn."
                  - generic [ref=e545]:
                    - link "Card opened" [ref=e546] [cursor=pointer]:
                      - /url: "#/company/tasks/01a0b669b465-000000000283"
                      - img [ref=e547]
                      - text: Card opened
                    - button "Dismiss this card" [ref=e549]:
                      - img [ref=e550]
              - generic [ref=e555]:
                - link "finished → In review" [ref=e556] [cursor=pointer]:
                  - /url: "#/company/tasks/01a0b669b465-000000000283"
                - button "Approve" [ref=e557]
              - article [ref=e559]:
                - button "Open Ceo's profile" [ref=e561]:
                  - generic [ref=e563]: C
                - generic [ref=e564]:
                  - generic [ref=e565]:
                    - button "Ceo" [ref=e566]
                    - generic [ref=e567]: 9:26 PM
                  - generic [ref=e568]:
                    - paragraph [ref=e569]: "\"sim gather the sources 1789766774798\" is ready for review (engineer ran it)."
                    - paragraph [ref=e570]: Search and collect what is current, newest first.
                    - paragraph [ref=e571]:
                      - strong [ref=e572]: MOCK_LLM
                      - text: mock inference backend reply.
              - generic [ref=e575]:
                - link "finished → In review" [ref=e576] [cursor=pointer]:
                  - /url: "#/company/tasks/01a0b669b472-000000000285"
                - button "Approve" [ref=e577]
              - article [ref=e579]:
                - button "Open Ceo's profile" [ref=e581]:
                  - generic [ref=e583]: C
                - generic [ref=e584]:
                  - generic [ref=e585]:
                    - button "Ceo" [ref=e586]
                    - generic [ref=e587]: 9:26 PM
                  - generic [ref=e588]:
                    - paragraph [ref=e589]: "\"sim write the digest 1789766774798\" is ready for review (writer ran it)."
                    - paragraph [ref=e590]: Turn the collected sources into a short digest with links.
                    - paragraph [ref=e591]:
                      - strong [ref=e592]: MOCK_LLM
                      - text: mock inference backend reply.
              - article [ref=e594]:
                - generic [ref=e597]: "Y"
                - generic [ref=e598]:
                  - generic [ref=e599]:
                    - generic [ref=e600]: You
                    - generic [ref=e601]: 9:26 PM
                  - paragraph [ref=e603]:
                    - text: Both pieces are back — take a look and close them out.
                    - strong [ref=e604]: MOCK_PLAN
                    - text: "[[{\"name\":\"review_task\",\"arguments\":{\"task_id\":\"01a0b669b465-000000000283\",\"decision\":\"approve\",\"note\":\"Sources look current.\"}},{\"name\":\"review_task\",\"arguments\":{\"task_id\":\"01a0b669b472-000000000285\",\"decision\":\"approve\",\"note\":\"Reads well; shipping it.\"}}],[]] close-1789766774798"
              - article [ref=e606]:
                - button "Open Engineer's profile" [ref=e608]:
                  - generic [ref=e610]: E
                - generic [ref=e611]:
                  - generic [ref=e612]:
                    - button "Engineer" [ref=e613]
                    - generic [ref=e614]: 9:26 PM
                  - paragraph [ref=e616]:
                    - strong [ref=e617]: MOCK_LLM
                    - text: mock inference backend reply.
            - generic [ref=e619]:
              - generic [ref=e620]:
                - 'combobox "Message #engineering-desk" [ref=e621]'
                - generic [ref=e622]:
                  - button "Full" [ref=e624]:
                    - img [ref=e625]
                    - generic [ref=e627]: Full
                    - img [ref=e628]
                  - button "Mention someone" [ref=e630]:
                    - img
                  - button "Attach a file" [ref=e631]:
                    - img
                  - button "Formatting" [ref=e632]:
                    - img
                  - button "Send" [disabled]:
                    - img
              - paragraph [ref=e633]: Enter to send · Shift+Enter for a new line
  - region "Notifications alt+T"
```

# Test source

```ts
  179 |   const writeId = await openCard(page, write);
  180 |   await expect(page.getByText("writer").first()).toBeVisible();
  181 | 
  182 |   // The spend gate, before anything is spent. An orchestrator cannot start paid
  183 |   // work by asking for it (issue #1512) — these cards are open and idle, and a
  184 |   // build that dispatched them here would pass every assertion after this one
  185 |   // while having taken the operator's decision away.
  186 |   expect(await stageOf(request, gatherId), "spawn_task must not dispatch").toBe("pending");
  187 |   expect(await stageOf(request, writeId), "spawn_task must not dispatch").toBe("pending");
  188 | 
  189 |   // ── 3. The operator starts the work ─────────────────────────────────────
  190 |   // The real gesture on the real board: entering Working is dispatch.
  191 |   await dispatch(page, gather);
  192 |   await dispatch(page, write);
  193 | 
  194 |   // ── 4. The team works them, and stops for a decision ────────────────────
  195 |   // A finished run lands in `in_review`, never `done` — accepting work is the
  196 |   // operator's call and a run does not make it for them.
  197 |   for (const [id, title] of [
  198 |     [gatherId, gather],
  199 |     [writeId, write],
  200 |   ] as const) {
  201 |     await expect
  202 |       .poll(() => stageOf(request, id), {
  203 |         message: `card "${title}" never settled`,
  204 |         timeout: 180_000,
  205 |         intervals: [2_000],
  206 |       })
  207 |       .toBe("in_review");
  208 |   }
  209 | 
  210 |   // ── 5. …and the conversation the goal was stated in is told ─────────────
  211 |   // The structural line a reader needs and the relay prose cannot give them:
  212 |   // the run *stopped*, and here is where it landed (issue #377). Counted rather
  213 |   // than addressed by card, because this surface renders a marker as a plain
  214 |   // system pill — see `markers` in `./orchestration`.
  215 |   await openMainLine(page);
  216 |   await expect
  217 |     .poll(() => markers(page).count(), {
  218 |       message: "the two settled cards did not mark the thread they were raised in",
  219 |       timeout: 120_000,
  220 |       intervals: [1_000],
  221 |     })
  222 |     .toBeGreaterThanOrEqual(markersBefore + 2);
  223 |   // `at least`, not `exactly`. Both halves of that are deliberate. Two is the
  224 |   // floor because two cards settled and each must say so. It is not a ceiling
  225 |   // because this surface renders a marker as a plain system pill with no card
  226 |   // id on it (see `markers` in `./orchestration`), so a third marker cannot be
  227 |   // told apart from ours — and in a full-suite run there is one: a card an
  228 |   // earlier spec raised in this same thread, settling on its own schedule while
  229 |   // this test runs. An exact count made this spec pass alone and fail in the
  230 |   // suite, which is the worst of both.
  231 |   // …and they say where the work landed, which is the half the relay prose
  232 |   // cannot carry: "finished" alone would leave a reader with the same wrong
  233 |   // impression issue #377 is about. Counted rather than read off the last pill,
  234 |   // because a straggler from another spec can arrive after ours.
  235 |   await expect
  236 |     .poll(
  237 |       () => page.getByRole("main").getByText("finished → In review", { exact: true }).count(),
  238 |       { message: "neither settled card said where it landed", timeout: 30_000 },
  239 |     )
  240 |     .toBeGreaterThanOrEqual(2);
  241 | 
  242 |   // ── 6. The orchestrator closes the goal out ─────────────────────────────
  243 |   // The second half of the loop, and the half nothing else covers: work that
  244 |   // was delegated coming back and being *accepted*. `review_task` with
  245 |   // `approve` is the `in_review → done` transition (issue #171, PR #179).
  246 |   await say(
  247 |     page,
  248 |     `Both pieces are back — take a look and close them out. ` +
  249 |       plan(
  250 |         [
  251 |           [
  252 |             call("review_task", {
  253 |               task_id: gatherId,
  254 |               decision: "approve",
  255 |               note: "Sources look current.",
  256 |             }),
  257 |             call("review_task", {
  258 |               task_id: writeId,
  259 |               decision: "approve",
  260 |               note: "Reads well; shipping it.",
  261 |             }),
  262 |           ],
  263 |           [],
  264 |         ],
  265 |         `close-${run}`,
  266 |       ),
  267 |   );
  268 | 
  269 |   for (const [id, title] of [
  270 |     [gatherId, gather],
  271 |     [writeId, write],
  272 |   ] as const) {
  273 |     await expect
  274 |       .poll(() => record(request, id).then((task) => task.column), {
  275 |         message: `card "${title}" was never accepted`,
  276 |         timeout: 180_000,
  277 |         intervals: [2_000],
  278 |       })
> 279 |       .toBe("done");
      |        ^ Error: card "sim gather the sources 1789766774798" was never accepted
  280 |   }
  281 | 
  282 |   // ── 7. The board an operator would come back to ─────────────────────────
  283 |   // Read from a reload rather than from the session that made the changes: the
  284 |   // columns above were written through the API, and a console that only moved
  285 |   // them in its own React state would still show them here.
  286 |   await page.reload();
  287 |   await openBoard(page);
  288 |   await expect(column(page, DONE)).toContainText(gather, { timeout: 60_000 });
  289 |   await expect(column(page, DONE)).toContainText(write);
  290 |   await expect(column(page, PENDING)).not.toContainText(gather);
  291 |   await expect(column(page, WORKING)).not.toContainText(gather);
  292 | });
  293 | 
```