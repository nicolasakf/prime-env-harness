# Prime Env

**Build, run, and evaluate AI models in controlled environments, from the terminal.**

Prime Env is a terminal harness where you describe an environment in plain
language (a card game, a negotiation, a document workflow, a simulation), have an
AI builder write and validate it, and then play it yourself or let models play it
against each other. Every run is recorded, resumable, and reportable.

## Why controlled environments

A model's behavior is only meaningful relative to the situation it acts in. Chat
transcripts and one-off prompts are hard to repeat, hard to score, and easy to
over-read. A controlled environment fixes the world around the model: the rules,
the information each participant may see, the actions that are legal, and the
outcome that counts. That makes results comparable across models and across runs,
and it turns "seemed smart" into measurements such as profit and loss, win rate,
decision latency, and cost. The same property makes environments the raw material
for reinforcement learning and for evaluations that cannot be gamed by wording.
They also keep risk contained: code runs under an explicit execution policy, and
hidden information stays hidden.

Prime Env is built to make that whole workflow cheap:

- **Create** an environment by conversation. The builder asks about rules,
participants, turn flow, privacy, deadlines and failure handling before it
writes any code.
- **Test** it before anyone plays. Every environment ships behavioral checks that
must pass; saved definitions are immutable, content-addressed versions, so a
result always names the exact rules that produced it.
- **Run** it with humans and models at the same table: one human and one agent,
several agent seats, or agents only. Agent reasoning stays in private seat
sessions and never leaks to other participants.
- **Control** execution. Choose local or Docker per run and grant networking,
mounts, secrets and integrations explicitly, or not at all.
- **Evaluate** the outcome. Instances persist every action and receipt, survive
crashes, and can be turned into notebook reports from reusable templates.



## Built on Prime Agent

Prime Env is built on top of [Prime Agent](https://github.com/PrimeIntellect-ai/prime-agent)
by [Prime Intellect](https://www.primeintellect.ai), an open-source Rust agent
harness (MIT-licensed). From it we inherit the persistent Python REPL,
recursive subagents, skills, MCP support, session daemon, and the terminal UI.
Prime Env adds the environment contract and runtime, the participant and seat
model, the `/env` interface, and the builder skills on top. Thank you to the
Prime Intellect team for building the foundation.

## Quick start

Requirements: a Rust toolchain (`./install-rust.sh` can help), Python 3.10 or
newer with `venv`, and Node 22.20 or newer for TypeScript environments. Docker
is optional.

```bash
git clone https://github.com/nicolasakf/prime-env-harness
cd prime-env-harness
cargo run -p pa-cli --bin prime-env-harness
```

On first launch, run `/login` to choose a model provider, then `/tutorial`.
Prime Env keeps its settings, credentials, sessions, and environments in
`~/.prime-env-harness`, separate from any other tool. Use
`--data-dir <directory>` to place them elsewhere.

You never have to memorize commands. Everything under `/env` can also be done by
talking to the agent: "build me a blackjack game where a human is a dealer and
it plays against up to six AI models", "enter it with local execution", "show
me what I can do". For reference:

```text
/env create <description>                Build a new environment
/env policy <draft> <policy-json>        Choose where and how it runs
/env validate <draft>                    Run its behavioral checks
/env save <draft> <id>                   Save an immutable version
/env enter <id>                          Play it
/env act <action>                        Take an action (or click [ Actions ])
/env leave | /env resume <instance>      Pause and come back later
```

New to the idea? [Environments Hub: A Community Hub To Scale RL To Open AGI](https://www.primeintellect.ai/blog/environments)
is a good introduction to what an environment is and why they matter.

## Examples

`[examples/](examples/README.md)` ships two complete environments you can try
immediately:


| Example                            | What it is                                                                                    |
| ---------------------------------- | --------------------------------------------------------------------------------------------- |
| `texas-holdem`                     | No-limit Texas Hold'em for 2 to 9 seats, human or agent, with private cards and a game report |
| `sequential-card-market-stateless` | A two-player card-value trading game with cumulative P&L, stateless agents, and a game report |


```bash
python3 scripts/install_examples.py
```

The script creates builder drafts and prints the `/env` commands to validate and
save them.

## Documentation

- [Harness guide](docs/harness.md): architecture, sessions, participants, wakes, forms, policy, recovery.
- [Reference](docs/reference.md): running, execution policies, integrations, seats, and ownership.
- [Environment forms](docs/environment-forms.md): terminal forms for setup and actions.
- [Verification record](VERIFICATION.md): checks run and known platform gaps.
- [Future ideas](FUTURE_IDEAS.md).



## Development

```bash
cargo run -p pa-cli --bin prime-env-harness   # debug build, rebuilds on change
make check                                    # fmt, clippy, tests, release build
make deny                                     # after dependency changes
```

Repository rules, crate boundaries and review standards are in
[AGENTS.md](AGENTS.md). Run heavy Cargo commands one at a time; `-j 2` lowers CPU
pressure.

> [!WARNING]
> Prime Env executes model-generated code with your user permissions. Local
> execution is not a sandbox. Use Docker, and grant only what an environment needs,
> for anything you do not trust.



## License

MIT, see [LICENSE](LICENSE). The inherited Prime Agent code is
Copyright (c) 2025-2026 Prime Intellect Ltd.

## Citation

If you use the underlying harness in research, please cite Prime Agent:

```bibtex
@article{karten2026prime,
  title={Prime Agent: A Self-Improving RLM Harness},
  author={Karten, Seth and Zhang, Alex L. and Thomas, Kevin and Müller, Sebastian and Bakouch, Elie and Auras, Daniel and Senghaas, Mika and Obeid, Fares and Dunas, Konstantin and Hagemann, Johannes and Jaghouar, Sami},
  journal={arXiv preprint arXiv:2608.23552},
  year={2026}
}
```

