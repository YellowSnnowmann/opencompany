import { describe, expect, it } from "vitest";

import type { ChatMessage } from "@/lib/chat";
import {
  HIVE_REFERRAL_AUTHOR,
  HIVE_REPORT_AUTHOR,
  blindRoundLength,
  foldEpisodes,
  foldStandings,
  parseEndingReport,
} from "@/lib/hive/episode";

/**
 * Folding a desk transcript back into the episodes it recorded.
 *
 * The two failures worth guarding are opposites and both silent: inventing a room
 * where a desk simply answered, and dropping the standings of a room that
 * actually deliberated. The first makes every DM sprout deliberation furniture;
 * the second is the status quo, where a room renders as bubbles beginning with
 * "!propose".
 */

let seq = 0;
function op(text: string): ChatMessage {
  return { id: `h${++seq}`, from: "you", text, at: seq * 1000, byPerson: true };
}
function turn(agentId: string, text: string): ChatMessage {
  return { id: `h${++seq}`, from: "company", channel: agentId, text, at: seq * 1000 };
}
function report(text: string): ChatMessage {
  return {
    id: `h${++seq}`,
    from: "company",
    channel: HIVE_REPORT_AUTHOR,
    text,
    at: seq * 1000,
  };
}

describe("a conversation that is not a room", () => {
  it("folds a plain exchange to no episodes at all", () => {
    // The load-bearing rule: room affordances are a question about the data,
    // never about the channel kind. A DM, #general and a single-responder desk
    // all land here, and none of them may sprout a topic rail.
    seq = 0;
    expect(foldEpisodes([op("how are the numbers?"), turn("analyst", "Up 4%.")])).toEqual(
      [],
    );
  });

  it("does not treat a reply merely mentioning a marker as a move", () => {
    seq = 0;
    expect(
      foldEpisodes([op("explain"), turn("analyst", "You would !propose #x to start.")]),
    ).toEqual([]);
  });

  it("does not sprout deliberation furniture around an ordinary exchange on a desk that has deliberated before", () => {
    // A channel earns `hasRoom` the moment it has hosted one real room, but
    // that must not turn every later plain Q&A on the same desk into a
    // "still deliberating" block — the operator's question here gets an
    // ordinary reply with no move, so it renders as a plain exchange even
    // though the desk deliberated earlier in the same transcript.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("decide the rollout"),
        turn("planner", "!propose #stage ship to staging first"),
        report("The desk settled on #stage after 1 turn (backed by planner)."),
        op("what time did that ship?"),
        turn("planner", "Around 9am."),
      ],
      { quorum: 1 },
    );
    // The real room is one closed episode with its one move.
    const real = episodes.find((e) => e.triggerId === "h1")!;
    expect(real.turns).toHaveLength(1);
    expect(real.ending).not.toBeNull();
    // Any dangling episode the trailing ordinary Q&A leaves behind claims no
    // turns, referrals, or report, so `groupEpisodes` never attaches a
    // message to it and it renders as nothing — the ordinary follow-up stays
    // a plain reply.
    for (const episode of episodes) {
      if (episode === real) continue;
      expect(episode.turns).toHaveLength(0);
      expect(episode.referrals).toHaveLength(0);
      expect(episode.reportId).toBeNull();
    }
  });

  it("still opens on the ordinary reply once a real move follows it in the same room", () => {
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("decide the rollout"),
        turn("planner", "let me think about this"),
        turn("planner", "!propose #stage ship to staging first"),
        report("The desk settled on #stage after 1 turn (backed by planner)."),
      ],
      { quorum: 1 },
    );
    expect(episodes).toHaveLength(1);
    expect(episodes[0].triggerId).toBe("h1");
    // The pre-move ordinary line deposited no trace of its own.
    expect(episodes[0].turns).toHaveLength(1);
    expect(episodes[0].turns[0].move).not.toBeNull();
  });
});

describe("segmentation", () => {
  it("keeps turns and reports threaded under their triggering message", () => {
    seq = 0;
    const trigger = op("decide the rollout");
    const threadedTurn = { ...turn("planner", "!propose #stage staging first"), parentId: trigger.id };
    const threadedReport = {
      ...report("The desk settled on #stage after 1 turn (backed by planner)."),
      parentId: trigger.id,
    };
    const episodes = foldEpisodes([trigger, threadedTurn, threadedReport], { quorum: 1 });
    expect(episodes).toHaveLength(1);
    expect(episodes[0].turns).toHaveLength(1);
    expect(episodes[0].reportId).toBe(threadedReport.id);
  });

  it("opens at the operator message and closes at the report", () => {
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("decide the rollout"),
        turn("planner", "!propose #stage ship to staging first"),
        turn("critic", "!evidence #stage ^1 the last full rollout took checkout down"),
        turn("critic", "!support #stage ^3 staging first"),
        report("The desk settled on #stage after 3 turns (backed by planner, critic)."),
      ],
      { quorum: 2 },
    );
    expect(episodes).toHaveLength(1);
    expect(episodes[0].triggerId).toBe("h1");
    expect(episodes[0].turns).toHaveLength(3);
    expect(episodes[0].ending).toEqual({
      kind: "converged",
      topic: "stage",
      supporters: ["planner", "critic"],
      turns: 3,
    });
  });

  it("separates two episodes on one desk", () => {
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("first question"),
        turn("planner", "!propose #a one"),
        report("The desk spent its 1-turn budget without reaching a decision."),
        op("second question"),
        turn("planner", "!propose #b two"),
        report("The desk spent its 1-turn budget without reaching a decision."),
      ],
      { quorum: 2 },
    );
    expect(episodes).toHaveLength(2);
    expect(episodes[0].topics.map((t) => t.id)).toEqual(["a"]);
    expect(episodes[1].topics.map((t) => t.id)).toEqual(["b"]);
  });

  it("flags a room it could not attach to an operator message", () => {
    // The transcript window scrolled past the trigger. Better to say so than to
    // attribute the room to whatever line happens to be at the top.
    seq = 0;
    const episodes = foldEpisodes([turn("planner", "!propose #stage x")], { quorum: 2 });
    expect(episodes[0].ambiguous).toBe(true);
  });

  it("renders a room that is still talking, with no ending yet", () => {
    seq = 0;
    const episodes = foldEpisodes([op("go"), turn("planner", "!propose #stage x")], {
      quorum: 2,
    });
    expect(episodes[0].ending).toBeNull();
    expect(episodes[0].turns).toHaveLength(1);
  });

  it("keeps a failure note out of the turns and does not close on it", () => {
    // A `hive-report` row saying a member's turn did not finish is not the
    // closing report; closing on it would end the room mid-argument.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("go"),
        report("@critic's turn did not finish: the model timed out"),
        turn("planner", "!propose #stage x"),
        report("The desk spent its 1-turn budget without reaching a decision."),
      ],
      { quorum: 2 },
    );
    expect(episodes).toHaveLength(1);
    expect(episodes[0].failed).toHaveLength(1);
    expect(episodes[0].turns).toHaveLength(1);
  });
});

describe("standings", () => {
  it("counts a proposal as its own author's support", () => {
    seq = 0;
    const episodes = foldEpisodes(
      [op("go"), turn("planner", "!propose #stage x"), turn("critic", "!support #stage ^2 y")],
      { quorum: 2 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.supporters).toEqual(["planner", "critic"]);
    expect(stage?.carried).toBe(true);
  });

  it("keeps an uncited support apart from a grounded one", () => {
    // On a require_evidential desk it does not count, and a transcript that reads
    // as though it did is precisely the failure worth surfacing.
    seq = 0;
    const episodes = foldEpisodes(
      [op("go"), turn("planner", "!propose #stage x"), turn("critic", "!support #stage y")],
      { quorum: 2 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.ungrounded).toEqual(["critic"]);
    expect(stage?.carried).toBe(false);
  });

  it("silences an advocate an objection targets", () => {
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("go"),
        turn("planner", "!propose #stage x"),
        turn("critic", "!object >2 ^1 that figure is stale"),
      ],
      { quorum: 1 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.silenced).toEqual(["planner"]);
    expect(stage?.supporters).toEqual([]);
  });

  it("silences an advocate only on the topic the objection's target argued", () => {
    // An objection is LOCAL where a refutation is global. Applying it to every
    // topic the advocate touched quietly turns each objection into a refutation,
    // and the room loses support nobody argued against. Caught by rendering the
    // fixture: one objection at a #ship proposal was silencing the same member's
    // separate, evidenced #stage support.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("decide the rollout"),
        turn("planner", "!propose #stage ship to staging first"),
        turn("critic", "!propose #ship go straight to production"),
        turn("archivist", "!evidence #stage ^1 the last rollout took checkout down"),
        turn("critic", "!support #stage ^4 that outage is enough for me"),
        turn("skeptic", "!object >3 ^4 production first ignores the outage"),
      ],
      { quorum: 2 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    const ship = episodes[0].topics.find((t) => t.id === "ship");
    expect(ship?.silenced).toEqual(["critic"]);
    expect(stage?.silenced).toEqual([]);
    expect(stage?.supporters).toEqual(["planner", "critic"]);
    expect(stage?.carried).toBe(true);
  });

  it("silences nobody when the objection targets a line that advocated nothing", () => {
    // There is no support to withdraw from a question or a piece of evidence.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("go"),
        turn("planner", "!propose #stage x"),
        turn("archivist", "!evidence #stage ^1 a fact"),
        turn("skeptic", "!object >3 ^1 that fact is stale"),
      ],
      { quorum: 1 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.silenced).toEqual([]);
    expect(stage?.supporters).toEqual(["planner"]);
  });

  it("records a refutation without removing the topic", () => {
    // Nothing is removed when a topic is refuted: it keeps its supporters and
    // stays in the standings so a reader can audit the refutation back to the
    // message it cites.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("go"),
        turn("planner", "!propose #stage x"),
        turn("critic", "!refute #stage ^2 the benchmark says otherwise"),
      ],
      { quorum: 1 },
    );
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.refuters).toEqual(["critic"]);
    expect(stage?.supporters).toEqual(["planner"]);
  });

  it("does not depend on the order objections arrived in", () => {
    const turns = [
      { messageId: "h2", seq: 2, agentId: "planner", at: 0, move: { kind: "propose" as const, topic: "stage", cites: [], body: "", text: "" }, blind: false, demoted: null, failed: false, referral: false },
      { messageId: "h3", seq: 3, agentId: "critic", at: 0, move: { kind: "object" as const, target: 2, cites: [1], body: "", text: "" }, blind: false, demoted: null, failed: false, referral: false },
    ];
    const forward = foldStandings(turns, 1);
    const backward = foldStandings([turns[1], turns[0]], 1);
    expect(forward).toEqual(backward);
  });
});

describe("referrals", () => {
  it("never counts an answer from another desk as a supporter", () => {
    // It folds as a system row precisely so it can inform the room without ever
    // authoring a position in it — one supporter must not count on two desks.
    seq = 0;
    const messages = [
      op("go"),
      turn("planner", "!propose #stage x"),
      {
        id: `h${++seq}`,
        from: "company" as const,
        channel: HIVE_REFERRAL_AUTHOR,
        text: "!support #stage ^1 the platform desk agrees",
        at: 9000,
      },
    ];
    const episodes = foldEpisodes(messages, { quorum: 2 });
    const stage = episodes[0].topics.find((t) => t.id === "stage");
    expect(stage?.supporters).toEqual(["planner"]);
    expect(episodes[0].referrals).toHaveLength(1);
  });
});

describe("the blind opening round", () => {
  it("ends at the first speaker who speaks twice", () => {
    expect(
      blindRoundLength(
        ["a", "b", "c", "a", "b"].map((agentId) => ({
          messageId: "",
          seq: null,
          agentId,
          at: 0,
          move: null,
          blind: false,
          demoted: null,
          failed: false,
          referral: false,
        })),
      ),
    ).toBe(3);
  });

  it("is not a round when only one member spoke", () => {
    expect(
      blindRoundLength([
        { messageId: "", seq: null, agentId: "a", at: 0, move: null, blind: false, demoted: null, failed: false, referral: false },
      ]),
    ).toBe(0);
  });
});

describe("the host's closing sentence", () => {
  it("reads all four endings", () => {
    expect(
      parseEndingReport("The desk settled on #stage after 9 turns (backed by a, b)."),
    ).toEqual({ kind: "converged", topic: "stage", supporters: ["a", "b"], turns: 9 });
    expect(
      parseEndingReport(
        "The desk deadlocked after 9 turns: #a and #b carried together and nobody broke the tie.",
      ),
    ).toEqual({ kind: "deadlocked", topics: ["a", "b"], turns: 9 });
    expect(
      parseEndingReport("The desk spent its 18-turn budget without reaching a decision."),
    ).toEqual({ kind: "exhausted", turns: 18 });
    expect(
      parseEndingReport("Nobody on the desk had anything to add, so the room did not open."),
    ).toEqual({ kind: "idle", turns: 0 });
  });

  it("reads an ending that carries its postscripts", () => {
    // Two may follow any ending: how many turns did not finish, and which lines
    // were demoted.
    expect(
      parseEndingReport(
        "The desk settled on #stage after 9 turns (backed by a, b). 1 turn did not finish and the room continued without it.",
      )?.kind,
    ).toBe("converged");
  });

  it("does not invent a member from the host's empty-supporter wording", () => {
    expect(
      parseEndingReport("The desk settled on #stage after 2 turns (backed by the room)."),
    ).toEqual({ kind: "converged", topic: "stage", supporters: [], turns: 2 });
  });

  it("handles the singular turn", () => {
    expect(
      parseEndingReport("The desk settled on #stage after 1 turn (backed by a).")?.turns,
    ).toBe(1);
  });

  it("is not fooled by a failure note", () => {
    expect(parseEndingReport("@critic's turn did not finish: timed out")).toBeNull();
  });
});

describe("agreement with the host", () => {
  it("reproduces the host's verdict on the canonical transcript", () => {
    // The shape `companies/hive_math_lab` produces, and the same fixture the
    // styleguide renders. If the console's fold and the desk's own report can
    // agree anywhere, it is here — and a disagreement on this one means the
    // fold has drifted from the room it is describing.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("decide the rollout"),
        turn("planner", "!propose #stage ship to staging first"),
        turn("critic", "!propose #ship go straight to production"),
        turn("archivist", "!evidence #stage ^1 the last rollout took checkout down"),
        turn("critic", "!support #stage ^4 that outage is enough for me"),
        turn("skeptic", "!object >3 ^4 production first ignores the outage"),
        turn("planner", "!commit #stage ^4 the room settled on staging"),
        report("The desk settled on #stage after 6 turns (backed by planner, critic)."),
      ],
      { quorum: 2 },
    );
    expect(episodes[0].ending?.kind).toBe("converged");
    expect(episodes[0].disagrees).toBe(false);
  });
});

describe("the budget a running room is spending", () => {
  it("derives three turns per member and says the number is derived", () => {
    // What makes a room that is still talking legible: a bare turn count cannot
    // say whether the desk is a third of the way through or one turn from
    // reporting itself exhausted.
    seq = 0;
    const episodes = foldEpisodes([op("go"), turn("a", "!propose #x y")], { members: 6 });
    expect(episodes[0].turnBudget).toBe(18);
    expect(episodes[0].turnBudgetDerived).toBe(true);
  });

  it("honours a budget it was told, and stops calling it derived", () => {
    seq = 0;
    const episodes = foldEpisodes([op("go"), turn("a", "!propose #x y")], {
      members: 6,
      turnBudget: 9,
    });
    expect(episodes[0].turnBudget).toBe(9);
    expect(episodes[0].turnBudgetDerived).toBe(false);
  });
});

describe("honesty about the fold", () => {
  it("marks every parsed episode derived", () => {
    seq = 0;
    const episodes = foldEpisodes([op("go"), turn("a", "!propose #x y")], { quorum: 2 });
    expect(episodes[0].derived).toBe(true);
  });

  it("records a disagreement with the host rather than hiding it", () => {
    // The report is the authority on screen. This flag is how the console says
    // it could not reproduce the verdict, instead of quietly rendering its own.
    seq = 0;
    const episodes = foldEpisodes(
      [
        op("go"),
        turn("planner", "!propose #stage x"),
        report("The desk spent its 1-turn budget without reaching a decision."),
      ],
      { quorum: 1 },
    );
    // Our fold carries #stage at quorum 1; the host says exhausted.
    expect(episodes[0].topics[0].carried).toBe(true);
    expect(episodes[0].ending?.kind).toBe("exhausted");
    expect(episodes[0].disagrees).toBe(true);
  });
});

describe("a transcript the host already tidied", () => {
  /**
   * The reload case, and the one that has never worked.
   *
   * `history_for_desk` rewrites the room's grammar into prose before it leaves
   * the host — `!support #kettle ^16 …` becomes `…`, per `readable_moves` — and
   * ships the authored body alongside it as `cueText`. The fold read `text`, so
   * on reload it saw no moves, decided the desk had simply answered, and
   * rendered no episode at all. Only a crossing to another desk ever saved it,
   * because `HIVE_REFERRAL_AUTHOR` is matched on the channel rather than the
   * body.
   *
   * Written against the tidied shape on purpose: a fixture carrying grammar in
   * `text` passes whether or not `cueText` is read, and would have passed
   * before this change.
   */
  function tidied(agentId: string, authored: string, prose: string): ChatMessage {
    return {
      id: `h${++seq}`,
      from: "company",
      channel: agentId,
      text: prose,
      cueText: authored,
      at: seq * 1000,
    };
  }

  it("still finds the room when only cueText carries the grammar", () => {
    seq = 0;
    const rows = [
      op("we need to decide the remedy"),
      tidied("exchanges", "!propose #kettle swap it for the steel one", "swap it for the steel one"),
      tidied(
        "refunds",
        "!support #kettle ^2 the customer asked for the swap first",
        "the customer asked for the swap first",
      ),
    ];

    const episodes = foldEpisodes(rows, { members: 2 });

    expect(episodes).toHaveLength(1);
    expect(episodes[0].turns.map((t) => t.move?.kind)).toEqual(["propose", "support"]);
    expect(episodes[0].topics.map((t) => t.id)).toEqual(["kettle"]);
    // The proposer backs its own option: the fold counts `Propose` and
    // `Support` alike, which is why reusing a topic id is a vote-corrupting
    // slip rather than a style one.
    expect(episodes[0].topics[0].supporters).toEqual(["exchanges", "refunds"]);
  });

  it("falls back to text for a host that sends no cueText", () => {
    // A console talking to an older host, and every row that carries no move —
    // the two bodies are equal there, so nothing has to choose between them.
    seq = 0;
    const rows = [
      op("we need to decide the remedy"),
      turn("exchanges", "!propose #kettle swap it for the steel one"),
      turn("refunds", "!support #kettle ^2 the customer asked for the swap first"),
    ];

    const episodes = foldEpisodes(rows, { members: 2 });

    expect(episodes).toHaveLength(1);
    expect(episodes[0].turns.map((t) => t.move?.kind)).toEqual(["propose", "support"]);
  });
});
