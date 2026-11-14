# Kilohertz Pinball — agent instructions

The plan is `docs/PLAN.md` (v5). Before delegating or starting any work, follow:

- **Model routing (§2.5, D34).** Every unit of work goes through the `work-classifier` agent
  first. Tiers: Haiku 5.5, Sonnet 5.5, Opus 5.5. **Fable 5.2 only** for very-high-blast-radius or
  extremely hard-to-troubleshoot work, with evidence, **and** only when the account's current
  usage limits leave enough Fable allowance. Unknown allowance means no Fable.
- **Agents (§2.6, D35).** `.claude/agents/`: `work-classifier`, `task-executor`, `work-verifier`,
  `issue-investigator`. Pass the classifier's brief verbatim and its model as the `model` override.
- **No self-review (§2.3 rule 9, D36).** Do not review code yourself or with a sibling agent.
  Open a PR against `main`, subscribe to its activity (auto-fix), resolve every CodeRabbit
  finding with a reply, and merge without waiting for a person once the §2.7 merge conditions
  hold.
- Rules in §2.3 apply to every change; record ambiguous choices in `docs/DECISIONS.md` and
  `docs/HUMAN_QUEUE.md` and continue.
