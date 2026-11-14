**(a) Top 5 risks or gaps**

1. Cross-agent enforcement is assumed: headless adapters and transcript hooks do not establish equivalent blocking coverage across Claude Code, Codex, Cursor, opencode, and pi.
2. The human gate conflates generality with deployment authorization; “general” does not specify which repositories, agents, or future projects inherit an executable rule.
3. Failure signals are ambiguous: nonzero exits, repeated edits, and corrections can reflect normal exploration; two occurrences do not establish a shared root cause.
4. Flagging one bad example and accepting its fix proves discrimination on that pair, not generality; generated checks can overfit, reject legitimate work, or execute unsafe replay commands.
5. “Artifact lands” hides the operational lifecycle: installation, versioning, conflicts, bypasses, rollback, and verification that each intended host actually enforces the rule.

**(b) Missing or badly-framed round-1 questions**

1. Autonomy boundary: may the system prepare patches, run sandboxed validation, and install repo-local rules automatically, or does each executable change require approval?
2. Generality boundary: does human approval select repo, language/ecosystem, organization, or global scope, and must scope expansion receive fresh approval?
3. Enforcement contract: must every supported host block violations, or can unsupported hosts offer detection only; should unavailable enforcement block work or warn?
4. Evidence threshold: should promotion require independent occurrences plus legitimate counterexamples, or pairwise replay alone; how should contested classifications be corrected?
5. Priorities: why decide repository home, classifier vendor, and PR screenshots before trust boundaries, cross-repo data access, and blocking guarantees?

**(c) Suggested changes**

1. Add a host capability matrix covering event capture, pre-execution blocking, bypass paths, installation, and verification; distinguish observed capabilities from assumptions.
2. Separate autonomous proposal preparation from human scope approval and deployment policy; make the approved artifact version and scope explicit.
3. Define an evidence record containing provenance, root-cause hypothesis, independent occurrences, uncertainty, counterexamples, and rejection feedback; deduplicate correlated failures.
4. Require sandboxed bad/fixed regression checks plus representative valid cases before blocking; introduce uncertain rules in observation mode with measurable promotion criteria.
5. Specify a versioned rule registry and reconciliation process with ownership, precedence, scoped exceptions, expiry, rollback, and enforcement coverage; defer PR-media requirements unless they address an evidenced failure.