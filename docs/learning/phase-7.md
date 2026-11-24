# Learning Notes — Phase 7: Evaluation and Testing

## 1. Two questions, two kinds of evidence
An AI agent can fail in two very different ways:

| Question | Kind of failure | How to test |
|---|---|---|
| Does the **system** work? (tools, routing, validation, approval, limits) | deterministic bugs | ordinary automated tests with a *scripted* model, same result every run |
| Does the **model** do the right thing? (choose tools, reason, ask, resist injection) | probabilistic behaviour | run real prompts against the real model and check the outcomes; ideally many runs |

Mixing them is the classic evaluation mistake: a green test suite with a scripted model says nothing about Claude's judgement, and a good-looking live answer says nothing about whether the approval gate really holds. The report keeps them in separate sections.

## 2. Turning expectations into machine checks
Each benchmark in `evals/benchmarks.json` says what *should* happen in a form code can check:
* `expected_tools`: patterns like `telemetry__search_logs` + `input_contains: ["payments-api"]` (alternatives with `a|b`, `optional` when skipping is also correct);
* `forbidden_calls`: e.g. any tool whose input contains `.env`;
* `checks`: answer facts (`answer_contains_any: ["not ready"]`), a clarifying question, a maximum number of tool calls, how many tickets exist.

These checks are deliberately **functional and conservative**. They can produce false failures (an answer phrased unexpectedly), so the report lists every failed check for a human to review. They never judge *quality*; that column is left for a person.

## 3. Verdicts: PASS, FAIL, ERROR, NOT_RUN
`ERROR` is different from `FAIL`. If the API call failed, the model never answered, so there is nothing to judge, and counting it as a failure (or worse, a pass) would distort the results. The runner also **stops calling the API after a persistent error** (400/401/403, "credit balance"), marking the rest `NOT_RUN` instead of repeating the same failure eight times.

## 4. Test the test harness
An evaluator that silently mis-scores is worse than none. `tests/eval_harness.rs` runs the real `agent-client eval` against a fake API scripted to produce one of each verdict, then checks the JSON, the Markdown table and the traces. Only then can a live report be trusted.

## 5. Honesty under constraints
This phase had no working API key, so the live benchmark could not run. The right response is to say so plainly, record the actual error (with its request ID), build and verify everything around it, and make the real run one command away. The wrong response is to fill the table with plausible-looking numbers. Never fabricate evaluation results; an empty cell with a reason is a valid result.

## 6. Latency, measured not guessed
The functional table's latencies come from `Instant::now()` around each agent turn, printed by the tests (`FUNCTIONAL|B2|…|49`). They show the tool layer is fast (milliseconds). In a live run almost all latency is the model, which `report.md` will show per benchmark.

## 7. What a better evaluation would add (with credits)
* several runs per prompt to see variance (LLM output varies run to run);
* an LLM-as-judge or rubric scoring for answer quality, calibrated against human ratings;
* cost per benchmark from the token counts already recorded in `results.json`.
