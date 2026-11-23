# Production Release Checklist (mandatory)

Every production release must show **evidence** for each item below. An item without evidence counts as **not met**. A release is ready only when every item is met, or an exception is documented and approved by the service owner.

| ID | Requirement | Typical evidence |
|---|---|---|
| RC-1 | All unit **and** integration tests pass on the release commit | CI run reference in the release notes |
| RC-2 | A load/performance test was run for changes that touch external calls, timeouts, connection pools, retries or batching | Load-test result in the release notes |
| RC-3 | The release-notes entry is complete, including a **tested rollback plan** | Release notes |
| RC-4 | No open CRITICAL/HIGH incident for the service, and no `FATAL` logs for the service in the last 24 hours | Logs, incident tickets |
| RC-5 | `ERROR` logs for the service in the last 24 hours are understood (cause known, not caused by the change) | Logs |
| RC-6 | The previous release is stable: **no rollback of the service in the last 72 hours** | Deployment history |
| RC-7 | New feature flags ship **disabled by default** | `service-config.yaml` |
| RC-8 | Database migrations are backward compatible with the currently deployed version | Release notes |
| RC-9 | A release owner / on-call contact is assigned | Release notes |
