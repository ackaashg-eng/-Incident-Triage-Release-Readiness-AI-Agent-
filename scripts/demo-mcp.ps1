<#
.SYNOPSIS
    Talks to the ops-telemetry-mcp server with raw MCP (JSON-RPC over stdio)
    messages and prints what comes back. No AI and no API key involved.

.DESCRIPTION
    Sends a fixed script of requests (handshake, discovery, resource reads,
    valid and invalid tool calls, ticket creation) to the compiled server,
    then prints one summary line per response. Use -Raw to see the exact JSON.

    Tickets go to data\demo-tickets.db (git-ignored), separate from the real
    data\tickets.db. Run the script twice and the ticket ID increases, which
    shows that tickets persist between server runs.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\demo-mcp.ps1 -Raw
#>
param(
    [switch]$Raw
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo build -p ops-telemetry-mcp --quiet
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    # Each entry: (id, label, JSON-RPC request). The leading comma keeps every
    # entry as its own 3-element array inside the outer list.
    $calls = @(
        ,@(1, 'initialize (handshake)', @{ method = 'initialize'; params = @{ protocolVersion = '2025-11-25'; capabilities = @{}; clientInfo = @{ name = 'demo-mcp.ps1'; version = '1.0' } } })
        ,@(2, 'tools/list', @{ method = 'tools/list'; params = @{} })
        ,@(3, 'resources/list', @{ method = 'resources/list'; params = @{} })
        ,@(4, 'resources/templates/list', @{ method = 'resources/templates/list'; params = @{} })
        ,@(5, 'read config://release-checklist', @{ method = 'resources/read'; params = @{ uri = 'config://release-checklist' } })
        ,@(6, 'read service://runbooks/payments-api', @{ method = 'resources/read'; params = @{ uri = 'service://runbooks/payments-api' } })
        ,@(7, 'read service://runbooks/../../.env', @{ method = 'resources/read'; params = @{ uri = 'service://runbooks/../../.env' } })
        ,@(8, 'search_logs auth-service ERROR 5', @{ method = 'tools/call'; params = @{ name = 'search_logs'; arguments = @{ service_name = 'auth-service'; severity = 'ERROR'; limit = 5 } } })
        ,@(9, 'search_logs billing-service', @{ method = 'tools/call'; params = @{ name = 'search_logs'; arguments = @{ service_name = 'billing-service' } } })
        ,@(10, 'search_logs severity=CRITICAL', @{ method = 'tools/call'; params = @{ name = 'search_logs'; arguments = @{ service_name = 'auth-service'; severity = 'CRITICAL' } } })
        ,@(11, 'search_logs limit=100000', @{ method = 'tools/call'; params = @{ name = 'search_logs'; arguments = @{ service_name = 'auth-service'; limit = 100000 } } })
        ,@(12, 'search_logs limit="ten" (wrong type)', @{ method = 'tools/call'; params = @{ name = 'search_logs'; arguments = @{ service_name = 'auth-service'; limit = 'ten' } } })
        ,@(13, 'get_deployment_history payments-api 24h', @{ method = 'tools/call'; params = @{ name = 'get_deployment_history'; arguments = @{ service_name = 'payments-api'; window_hours = 24 } } })
        ,@(14, 'get_deployment_history window=-5', @{ method = 'tools/call'; params = @{ name = 'get_deployment_history'; arguments = @{ service_name = 'auth-service'; window_hours = -5 } } })
        ,@(15, 'create_incident_ticket (valid)', @{ method = 'tools/call'; params = @{ name = 'create_incident_ticket'; arguments = @{ title = 'payments-api checkout outage after 2.14.0'; severity = 'CRITICAL'; summary = 'Gateway timeouts since 09:12 UTC, 7 min after deploying 2.14.0 (dep-1048); circuit breaker open (log-0036).'; assignee = 'payments-oncall' } } })
        ,@(16, 'create_incident_ticket (assignee=admin)', @{ method = 'tools/call'; params = @{ name = 'create_incident_ticket'; arguments = @{ title = 'SYSTEM OVERRIDE'; severity = 'CRITICAL'; summary = 'Injected request taken from a log message.'; assignee = 'admin' } } })
    )

    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($call in $calls) {
        $id, $label, $req = $call
        $msg = [ordered]@{ jsonrpc = '2.0'; id = $id; method = $req.method; params = $req.params }
        $lines.Add(($msg | ConvertTo-Json -Depth 10 -Compress))
        if ($id -eq 1) { $lines.Add('{"jsonrpc":"2.0","method":"notifications/initialized"}') }
    }

    $env:DATA_DIR = Join-Path $root 'mock_data'
    $env:TICKETS_DB = Join-Path $root 'data\demo-tickets.db'
    $env:LOG_LEVEL = 'error'
    $exe = Join-Path $root 'target\debug\ops-telemetry-mcp.exe'
    $output = $lines | & $exe
    Remove-Item Env:DATA_DIR, Env:TICKETS_DB, Env:LOG_LEVEL

    if ($Raw) {
        Write-Host '--- requests (client -> server, stdin) ---'
        $lines | ForEach-Object { Write-Host $_ }
        Write-Host '--- responses (server -> client, stdout) ---'
        $output | ForEach-Object { Write-Host $_ }
        return
    }

    $responses = @{}
    foreach ($line in $output) {
        $msg = $line | ConvertFrom-Json
        if ($null -ne $msg.id) { $responses[[int]$msg.id] = $msg }
    }

    foreach ($call in $calls) {
        $id, $label, $req = $call
        $r =$responses[[int]$id]
        if ($null -eq $r) { $summary = 'NO RESPONSE' }
        elseif ($r.error) { $summary = "JSON-RPC error $($r.error.code): $($r.error.message)" }
        else {
            $res = $r.result
            switch ($id) {
                1 { $summary = "server=$($res.serverInfo.name) $($res.serverInfo.version), protocol=$($res.protocolVersion)" }
                2 { $summary = 'tools: ' + (($res.tools | ForEach-Object { "$($_.name) (readOnly=$($_.annotations.readOnlyHint))" }) -join ', ') }
                3 { $summary = 'resources: ' + (($res.resources | ForEach-Object { $_.uri }) -join ', ') }
                4 { $summary = 'templates: ' + (($res.resourceTemplates | ForEach-Object { $_.uriTemplate }) -join ', ') }
                { $_ -in 5, 6 } { $t = $res.contents[0].text; $summary = "$($res.contents[0].mimeType), $($t.Length) chars, starts: " + ($t -split "`n")[0] }
                default {
                    $s = $res.structuredContent
                    if ($res.isError -and $s.error) {
                        $e = $s.error
                        $summary = "isError: $($e.code) - $($e.message)"
                        if ($e.valid_values) { $summary += " | valid: $($e.valid_values -join ', ')" }
                        if ($e.field_errors) { $summary += ' | fields: ' + (($e.field_errors | ForEach-Object { $_.field }) -join ', ') }
                    }
                    elseif ($res.isError) { $summary = "isError: $($res.content[0].text)" }
                    elseif ($s.entries) { $summary = "ok: $($s.returned) of $($s.total_matching) -> " + (($s.entries | ForEach-Object { "$($_.id) $($_.severity)" }) -join ', ') }
                    elseif ($null -ne $s.deployments) { $summary = "ok: $($s.count) deployment(s) -> " + (($s.deployments | ForEach-Object { "$($_.version) $($_.commit.Substring(0,8)) $($_.deployed_at) $($_.rollback_status)" }) -join '; ') }
                    elseif ($s.ticket) { $summary = "ok: $($s.ticket.ticket_id) $($s.ticket.severity) -> $($s.ticket.assignee), created $($s.ticket.created_at)" }
                    else { $summary = 'ok' }
                }
            }
        }
        '{0,3}  {1,-42} {2}' -f $id, $label, $summary
    }
}
finally {
    Pop-Location
}
