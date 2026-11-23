# Runbook: payments-api

## Service purpose
`payments-api` authorizes and captures card payments for checkout. It calls the external card processor **PayFlow** (`payflow-gateway`) for every authorization and stores payment state in the `payments` database. Downstream, `notification-worker` sends payment receipts and failure emails.

- **Owner:** Payments team — on-call rotation `payments-oncall`
- **Tier:** 1 (revenue-critical)
- **SLO:** 99.5% authorization success over 30 minutes; p95 checkout latency < 4s

## Dependencies
| Dependency | Purpose | Normal behaviour |
|---|---|---|
| payflow-gateway (external) | Card authorization | p50 1.4–2.1s, p95 up to ~3s at the EU morning peak |
| payments database | Payment state | p95 query < 20ms |
| notification-worker | Receipts / failure emails | asynchronous |

## Common failures
1. **Gateway timeouts (`GATEWAY_TIMEOUT`)**: requests to PayFlow exceed the client timeout. Usually PayFlow slowness, or a client timeout set below PayFlow's normal latency.
2. **Connection-pool exhaustion**: all HTTP connections to PayFlow are busy; requests queue, then time out. Often amplified by retries.
3. **Retry storms**: automatic retries multiply load during a slowdown and keep the pool saturated.
4. **Circuit breaker open**: after repeated failures the breaker rejects all payments to protect the system (`FATAL` log). Checkout returns HTTP 503.
5. **Settlement mismatches**: the nightly batch reports mismatches between our records and PayFlow's.

## Investigation steps
1. Check `ERROR` and `FATAL` logs for `payments-api`: when did failures start, and what is the top `error_code`?
2. Check the deployment history for the last 24h. **If failures began within 60 minutes after a deployment, treat that deployment as the prime suspect**, but confirm with evidence: correlation in time is not proof.
3. Compare the failing behaviour with the release notes and `service-config.yaml` (timeouts, retries, pool sizes) for the deployed version.
4. Check whether PayFlow itself reports an outage (status logs). If PayFlow reports normal operation, the problem is likely on our side.
5. Check `notification-worker` for a surge in `payment_failed` emails (customer impact indicator).

## Troubleshooting
| Symptom | Action |
|---|---|
| Timeouts started right after a deploy | **Roll back** to the previous version first (no schema changes in 2.13.x→2.14.0), then investigate |
| Client timeout below PayFlow's p95 latency | Restore a timeout ≥ 2× PayFlow p95 (previous value: 10,000ms) |
| Pool saturated + retries | Disable retry-on-timeout or add exponential backoff with jitter; restore pool size |
| Circuit breaker open | Do not force-close it; fix the cause, then let half-open probes recover |
| PayFlow-wide outage | Enable the "payments degraded" banner; contact PayFlow support |

## Escalation rules
| Condition | Ticket severity | Action |
|---|---|---|
| Card payments failing for > 5% of requests for 10+ minutes, or circuit breaker open | **CRITICAL** | Page `payments-oncall` immediately, assign the incident to `payments-oncall`, notify the incident commander |
| Success rate 95–99.5% for 30+ minutes | **HIGH** | Ticket to `payments-oncall` |
| Isolated slow requests / single errors | **MEDIUM** | Ticket to `payments-oncall`, next business day |
| Settlement mismatch | **HIGH** | Ticket to `payments-oncall` + finance-ops |

Incident tickets must be assigned to a team on-call handle (e.g. `payments-oncall`), never to individual admin accounts.
