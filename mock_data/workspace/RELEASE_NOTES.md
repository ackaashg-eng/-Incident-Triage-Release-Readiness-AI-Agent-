# Release Notes — Production Services

Newest releases first. Versions and commits match the production deployment history.

---

## payments-api

### 2.14.0 — deployed 2026-11-20 09:05 UTC (commit `561378f`)
**Release owner:** payments-oncall

**Changes**
- Migrated to the PayFlow client v3 (async HTTP client).
- **Changed:** PayFlow request timeout reduced from **10,000 ms to 2,000 ms** to "fail fast" (`payflow.timeout_ms`).
- **Added:** automatic retry on timeout, up to 3 attempts, fixed 0 ms backoff (`payflow.retry`).
- **Changed:** HTTP connection pool reduced from **50 to 20** connections per instance (`payflow.pool.max_connections`).

**Verification**
- Unit tests: passed (CI #8812).
- Integration tests: passed against the PayFlow mock (CI #8812).
- Load test: **skipped** — PayFlow sandbox unavailable during the release window.

**Rollback plan:** redeploy 2.13.2 (no database migrations in this release).

### 2.13.2 — deployed 2026-11-18 13:45 UTC (commit `f245c55`)
- Fixed rounding of zero-decimal currencies (JPY, KRW).
- Verification: unit + integration tests passed (CI #8790). Rollback plan: redeploy 2.13.1.

### 2.13.1 — deployed 2026-11-15 09:00 UTC (commit `19524a0`)
- Idempotency-key validation on `POST /v1/payments`; dependency updates.
- Verification: unit + integration tests passed (CI #8741). Rollback plan: redeploy 2.13.0.

---

## auth-service

### 4.8.1 — deployed 2026-11-19 16:30 UTC (commit `6d47046`)
**Release owner:** identity-oncall
- JWT library patch release (security fix, no API changes).
- Email claims are now redacted in debug logs.
- Verification: unit + integration tests passed (CI #8801). Rollback plan: redeploy 4.8.0.

### 4.8.0 — deployed 2026-11-14 10:20 UTC (commit `0344cc0`)
- Refresh-token reuse detection; new session revocation endpoint.
- Verification: unit + integration tests passed, load test passed (CI #8712). Rollback plan: redeploy 4.7.3.

---

## notification-worker

### 3.2.0 — RELEASE CANDIDATE (planned for 2026-11-21, not yet deployed)
**Release owner:** messaging-oncall

**Changes**
- New batched email pipeline: up to 500 recipients per SMTP provider call, behind the feature flag `batch_send_v2`.
- Automatic failover to the secondary SMS provider when the primary quota exceeds 90%.
- Database migration: adds nullable column `notifications.batch_id` (backward compatible with 3.1.x).

**Verification**
- Unit tests: passed (CI #8840).
- Integration tests: **2 failures** (`test_batch_partial_failure`, `test_sms_failover_order`), flagged as flaky, under investigation.
- Load test: not run yet.

**Rollback plan:** _TBD_

### 3.1.4 — deployed 2026-11-17 18:10 UTC (commit `508ac0d`)
- Re-landed SMS retries, now with provider idempotency keys (fixes the duplicate deliveries of 3.1.3).
- Verification: unit + integration tests passed (CI #8768). Rollback plan: redeploy 3.1.2.

### 3.1.3 — deployed 2026-11-17 13:30 UTC (commit `d4f4880`) — **ROLLED BACK** 14:20 UTC
- Retried SMS sends on provider 5xx responses.
- Rolled back to 3.1.2 after duplicate SMS deliveries.

### 3.1.2 — deployed 2026-11-16 11:00 UTC (commit `02a12a4`)
- Template rendering cache; SMTP connection reuse.
