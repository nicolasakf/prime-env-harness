# Examples

Two complete environments, each with behavioral checks, a suggested execution
policy, and a notebook report template. Hold'em also ships an optional replay
viewer.

| Directory | Seats | What it is |
| --- | --- | --- |
| [`texas-holdem`](texas-holdem) | 2 to 9, human or agent | Fixed-hand no-limit Texas Hold'em with play chips, private cards, side pots, and no table talk. Setup: hands, starting chips, blinds. |
| [`sequential-card-market-stateless`](sequential-card-market-stateless) | 2, human or agent | Two players trade a contract that settles to the sum of all card ranks, with private hole cards, public community cards, and cumulative P&L. Each agent request carries its full current state instead of the transcript. |

## Install

From the repository root:

```bash
python3 scripts/install_examples.py            # both examples
python3 scripts/install_examples.py texas-holdem
```

Add `--data-dir <directory>` if you launch Prime Env with a custom data
directory. The script copies each example into a fresh builder draft, installs its
report template and any viewer, and prints the commands to finish. It does not run or save
anything. Then, in Prime Env:

```text
/env policy <draft-path> {"backend":"local"}
/env validate <draft-path>
/env save <draft-path> <id>
/env enter <id>
```

Or tell the agent: "validate and save the new drafts, then let me play
texas-holdem against an agent". Both examples use pure-Python rules with no
dependencies, so `local` execution is enough and Docker is not required.

## Reports

After a run finishes, ask for a report ("make a report for that run"). The
`game-report` template under each example turns the run's recorded actions into
charts and tables. The Hold'em template was verified on the version shipped here.
The market template was verified on an earlier version that differs only in the
acknowledgment returned by `say`, and is marked compatible with the shipped
version without a separate run.

## Viewer

`texas-holdem/viewer/` draws the table: seats, cards, bets, the board, each
hand's showdown, a hand log, and stacks by hand, with hand-by-hand chapter marks.
It replays an ended run with every card face up ("show me a replay of that run"),
and follows a running one live with `/env view`: at the table you see your own
cards and everyone else's face down, and `/env view public` hides them all. Only
runs recorded with snapshot history can be replayed.
Viewers are optional; other environments have none unless you ask the builder
for one.

## Design notes

[`design-notes/`](design-notes) holds the reasoning behind changes to an example.
