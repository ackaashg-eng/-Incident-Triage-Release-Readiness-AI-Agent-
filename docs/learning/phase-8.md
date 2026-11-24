# Learning Notes — Phase 8: Documentation and Submission

## 1. Documentation has audiences
* **README.md** is for someone who wants to *run* it: prerequisites, one setup command, how to use it, what to do when it breaks. Every command must work as typed in PowerShell.
* **REPORT.md** is for someone who wants to *understand and assess* it: architecture, design decisions, how requirements are met, results, and honest limitations.
* **docs/** holds depth that would overload either (security, evaluation, filesystem MCP, data spec).

## 2. Evidence over claims
The compliance matrix maps each requirement to *where* it is implemented, *how* it was verified, and *which test or file* proves it. A row without evidence is not "Complete". That is why three rows say **Partial** (they need the live API) and one says **Pending** (the GitHub URL). Saying "done" without evidence is how projects fail reviews.

## 3. Make verification repeatable
`scripts/audit.ps1` re-runs the whole checklist: files present, build, both MCP servers, all tests, lints, a secret scan of every tracked file, and git hygiene. Anyone can run it and get the same answer, which is worth more than a statement in a report.

## 4. Audits find real bugs
Running the audit-style checks found two real problems:
* `setup.ps1` still ran `agent-client` with no arguments, which had become the *interactive* mode in Phase 5. A new user's setup would have hung waiting for input. Fixed and re-run.
* The CLI banner printed the full trace path, including the Windows user name, into saved console logs. Now redacted like everything else.

Neither showed up in unit tests, because both are about how pieces fit together. End-to-end checks catch integration bugs.

## 5. Traces are evidence: label their provenance
The sample traces are genuine records of real executions, but the model's replies came from a scripted endpoint (no API credits). The trace says so itself (`api_endpoint` in `session_start`), and `traces/samples/README.md` explains it. An honest label keeps evidence trustworthy; an unlabelled scripted trace passed off as a live run would undermine everything else.
