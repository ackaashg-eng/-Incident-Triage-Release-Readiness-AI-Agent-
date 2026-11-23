# Runbook: notification-worker

## Service purpose
`notification-worker` consumes notification jobs from queues (`notif-email`, `notif-sms`) and delivers them through external providers: an SMTP relay for email and an SMS gateway. Other services (e.g. `payments-api`, `auth-service`) enqueue receipts, payment-failure notices and password-reset messages.

- **Owner:** Messaging team — on-call rotation `messaging-oncall`
- **Tier:** 2 (delays are tolerated; losses and duplicates are not)
- **SLO:** 99% of notifications delivered within 5 minutes; zero duplicate SMS

## Dependencies
| Dependency | Purpose | Limits |
|---|---|---|
| smtp-primary (external) | Email delivery | Max 50 concurrent connections; answers `421` when exceeded |
| sms-primary (external) | SMS delivery | Daily quota; rejects messages above it |
| Message queues | Job intake | Lag warning at 60s |

## Common failures
1. **SMTP 421 (too many connections)**: the provider throttles us; batches are retried and re-queued.
2. **SMS quota exhaustion**: the provider rejects SMS once the daily quota is nearly used up.
3. **Queue lag**: consumers fall behind during traffic spikes (e.g. a surge of payment-failure emails caused by an upstream incident).
4. **Duplicate deliveries**: retries without idempotency keys re-send messages the provider already accepted (the cause of the 3.1.3 rollback).

## Investigation steps
1. Check `WARN`/`ERROR` logs for provider responses (421, quota) and queue lag.
2. If volume is unusual, check which template is spiking: a surge of `payment_failed` usually means an upstream `payments-api` incident, not a worker problem.
3. Check the deployment history for recent deployments and **rollbacks**.
4. For release decisions, compare the release notes and `service-config.yaml` against the release checklist.

## Troubleshooting
| Symptom | Action |
|---|---|
| SMTP 421 | Lower `smtp.max_connections`; backoff is automatic |
| SMS quota > 90% | Enable the secondary SMS provider, or defer non-urgent SMS until the quota resets (00:00 UTC) |
| Queue lag > 60s | Scale the consumer group; investigate upstream volume |
| Duplicate SMS | Roll back immediately; verify provider idempotency keys are sent |

## Escalation rules
| Condition | Ticket severity | Action |
|---|---|---|
| All deliveries failing for 30+ minutes (e.g. password-reset emails not sent) | **CRITICAL** | Page `messaging-oncall`, notify the incident commander |
| Duplicate or lost notifications | **HIGH** | Ticket to `messaging-oncall`, consider rollback |
| Queue lag > 15 minutes | **HIGH** | Page `messaging-oncall` |
| Provider throttling that self-recovers | **LOW** | No ticket unless recurring daily |
| SMS quota exhausted before 18:00 UTC | **MEDIUM** | Ticket to `messaging-oncall` |

Incident tickets must be assigned to a team on-call handle, never to individual admin accounts.
