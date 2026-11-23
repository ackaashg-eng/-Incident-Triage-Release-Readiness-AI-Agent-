# Runbook: auth-service

## Service purpose
`auth-service` issues and validates access tokens (JWT) for all customer-facing APIs, handles logins and refresh tokens, and publishes its public signing keys as a JWKS document. Every other service depends on it to authenticate requests.

- **Owner:** Identity team — on-call rotation `identity-oncall`
- **Tier:** 1 (all logins depend on it)
- **SLO:** 99.9% successful token validations; p95 token issuance < 100ms

## Dependencies
| Dependency | Purpose |
|---|---|
| user-directory | Account lookup during login |
| JWKS endpoint (own) | Public keys used to validate tokens |
| Redis session store | Refresh-token and session state |

## Common failures
1. **Stale JWKS cache after key rotation**: after a signing-key rotation, an instance that has not refreshed its key cache rejects tokens signed with the new key (`unknown key id`). Usually self-heals at the next refresh.
2. **user-directory connectivity**: connection resets or timeouts during login; normally absorbed by one retry.
3. **Credential-stuffing attacks**: bursts of failed logins from a network range; the rate limiter blocks them (`WARN`).
4. **Clock skew**: tokens rejected as "not yet valid" when an instance's clock drifts.

## Investigation steps
1. Check `ERROR` logs for `auth-service`: which error, how many instances, and how long it lasted.
2. Check whether a **scheduled key rotation** happened just before validation errors. Rotations happen daily at ~03:00 UTC.
3. Check the deployment history: did errors start within 60 minutes after a deployment? Remember that errors with no nearby deployment (e.g. after key rotation) are not deploy-related.
4. For login failures, check user-directory errors and the rate limiter logs.

## Troubleshooting
| Symptom | Action |
|---|---|
| `unknown key id` right after rotation | Force a JWKS refresh on the affected instances; consider shortening the JWKS cache TTL |
| JWKS refresh slow (> 3s) | Check the JWKS endpoint latency; ensure refresh happens before the new key is used for signing |
| Repeated user-directory resets | Check user-directory health; escalate if retries start failing |
| Rate-limiter blocks | No action unless legitimate users are affected; report the source range to security |

## Escalation rules
| Condition | Ticket severity | Action |
|---|---|---|
| > 1% of token validations failing for 10+ minutes on multiple instances | **CRITICAL** | Page `identity-oncall`, notify the incident commander |
| Validation errors on a single instance that self-heal within 10 minutes | **MEDIUM** | Ticket to `identity-oncall` for follow-up (e.g. tune JWKS cache) |
| Suspected credential stuffing | **HIGH** | Ticket to `security-oncall` |
| Single retried dependency errors | **LOW** | No ticket unless recurring |

Incident tickets must be assigned to a team on-call handle, never to individual admin accounts.
