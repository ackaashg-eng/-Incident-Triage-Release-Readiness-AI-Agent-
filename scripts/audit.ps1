<#
.SYNOPSIS
    Final repository audit: verifies the submission checklist with real commands.

.DESCRIPTION
    Checks required folders/files, builds, starts both MCP servers (check-mcp),
    runs the whole test suite, lint and format checks, scans every tracked file
    for secrets, and checks git hygiene. Prints PASS/FAIL per item and exits 1
    if anything mandatory fails. No API key is needed: the live-Claude items are
    reported as MANUAL.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\audit.ps1
#>
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
$results = New-Object System.Collections.Generic.List[object]

function Add-Result([string]$Item, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Item = $Item; Status = $(if ($Ok) { 'PASS' } else { 'FAIL' }); Detail = $Detail })
    $color = if ($Ok) { 'Green' } else { 'Red' }
    Write-Host ("[{0}] {1} - {2}" -f $(if ($Ok) { 'PASS' } else { 'FAIL' }), $Item, $Detail) -ForegroundColor $color
}

function Invoke-Quiet([string]$Exe, [string[]]$Arguments) {
    # Native tools write progress to stderr; capture everything as text.
    $out = & $Exe @Arguments 2>&1 | ForEach-Object { "$_" }
    return [pscustomobject]@{ Code = $LASTEXITCODE; Output = $out }
}

try {
    Write-Host "Repository audit - $root`n"

    # 1. Structure ------------------------------------------------------------
    $required = @(
        'README.md', 'REPORT.md', 'PROJECT_CONTEXT.md', 'Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json',
        '.env.example', '.gitignore',
        'mcp-server\src\main.rs', 'mcp-server\src\server.rs', 'mcp-server\src\data.rs', 'mcp-server\src\tickets.rs', 'mcp-server\src\error.rs',
        'agent-client\src\main.rs', 'agent-client\src\cli.rs', 'agent-client\src\eval.rs', 'agent-client\src\trace.rs',
        'agent-client\src\llm\anthropic.rs', 'agent-client\src\agent\runner.rs', 'agent-client\src\agent\registry.rs',
        'agent-client\src\agent\approval.rs', 'agent-client\src\safety\injection.rs', 'agent-client\src\safety\redact.rs',
        'agent-client\src\mcp\manager.rs', 'agent-client\src\mcp\filesystem.rs',
        'mock_data\logs.json', 'mock_data\deployments.json', 'mock_data\release-checklist.md',
        'mock_data\runbooks\auth-service.md', 'mock_data\runbooks\payments-api.md', 'mock_data\runbooks\notification-worker.md',
        'mock_data\workspace\RELEASE_NOTES.md', 'mock_data\workspace\service-config.yaml',
        'evals\benchmarks.json', 'docs\EVALUATION.md', 'docs\SECURITY.md', 'docs\FILESYSTEM_MCP.md', 'docs\MOCK_DATA_SPEC.md',
        'traces\samples\investigation-with-hitl-approval.jsonl', 'traces\samples\handled-failures-and-edge-cases.jsonl',
        'scripts\setup.ps1', 'scripts\run.ps1'
    )
    $missing = $required | Where-Object { -not (Test-Path $_) }
    Add-Result 'Required files and folders' ($missing.Count -eq 0) $(if ($missing) { "missing: $($missing -join ', ')" } else { "$($required.Count) present" })

    # 2. Build ------------------------------------------------------------------
    $build = Invoke-Quiet 'cargo' @('build', '--workspace', '--quiet')
    Add-Result 'Workspace builds' ($build.Code -eq 0) "cargo build exit $($build.Code)"

    # 3. Both MCP servers start, connect, discover, read, refuse ------------------
    $check = Invoke-Quiet '.\target\debug\agent-client.exe' @('check-mcp')
    $okLines = @($check.Output | Where-Object { $_ -match '^\s+\[ok\s*\]' }).Count
    $failLines = @($check.Output | Where-Object { $_ -match '\[FAIL\]' }).Count
    Add-Result 'Both MCP servers start and pass check-mcp' ($check.Code -eq 0 -and $failLines -eq 0) "$okLines checks ok, $failLines failed"

    # 4. Tests -------------------------------------------------------------------
    $test = Invoke-Quiet 'cargo' @('test', '--workspace')
    $passed = ($test.Output | Select-String 'test result: ok\. (\d+) passed' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value } | Measure-Object -Sum).Sum
    $failedTests = @($test.Output | Select-String -CaseSensitive '^test .* FAILED$|panicked').Count
    Add-Result 'Full test suite' ($test.Code -eq 0 -and $failedTests -eq 0) "$passed passed, exit $($test.Code)"

    $clippy = Invoke-Quiet 'cargo' @('clippy', '--workspace', '--all-targets', '--quiet', '--', '-D', 'warnings')
    Add-Result 'Clippy (warnings = errors)' ($clippy.Code -eq 0) "exit $($clippy.Code)"
    $fmt = Invoke-Quiet 'cargo' @('fmt', '--all', '--', '--check')
    Add-Result 'Formatting' ($fmt.Code -eq 0) "exit $($fmt.Code)"

    # 5. Secrets -----------------------------------------------------------------
    $tracked = git ls-files
    Add-Result '.env is not tracked' (-not ($tracked -contains '.env')) 'git ls-files'
    git check-ignore -q .env
    Add-Result '.env is git-ignored' ($LASTEXITCODE -eq 0) 'git check-ignore'
    $patterns = @('sk-ant-[A-Za-z0-9_\-]{10,}', 'AKIA[0-9A-Z]{16}', '-----BEGIN [A-Z ]*PRIVATE KEY-----', 'ghp_[A-Za-z0-9]{20,}', 'xox[bp]-[A-Za-z0-9\-]{10,}')
    $hits = @()
    foreach ($file in $tracked) {
        if (-not (Test-Path $file -PathType Leaf)) { continue }
        if ((Get-Item $file).Length -gt 5MB) { continue }
        foreach ($p in $patterns) {
            $m = Select-String -Path $file -Pattern $p -CaseSensitive -ErrorAction SilentlyContinue | Where-Object {
                # Test fixtures and docs use obviously fake keys on purpose.
                $_.Line -notmatch 'sk-ant-(test|api03-(AbCdEf|abcdef|AAAA|ZZZZ|SHOULDNOT))' -and $_.Line -notmatch 'AKIAIOSFODNN7EXAMPLE' -and $_.Line -notmatch 'ghp_abcdefghijklmnop' -and $_.Line -notmatch "MIIBOgIBAAJBAK"
            }
            if ($m) { $hits += $m | ForEach-Object { "$($_.Path | Resolve-Path -Relative):$($_.LineNumber)" } }
        }
    }
    Add-Result 'No secrets in tracked files' ($hits.Count -eq 0) $(if ($hits) { $hits -join ', ' } else { "$($tracked.Count) tracked files scanned" })
    $traceText = (Get-Content traces\samples\* -Raw) -join ''
    $leak = $traceText -match 'sk-ant-' -or $traceText -match [regex]::Escape($env:USERNAME + '\')
    Add-Result 'Sample traces contain no keys or user paths' (-not $leak) 'traces/samples/*'

    # 6. Git hygiene -------------------------------------------------------------
    $status = git status --porcelain
    Add-Result 'Git working tree clean' (-not $status) $(if ($status) { "$(@($status).Count) uncommitted change(s)" } else { 'clean' })
    $remote = git remote -v
    Write-Host ("[INFO] Git remote: {0}" -f $(if ($remote) { ($remote | Select-Object -First 1) } else { 'none configured (GitHub URL pending)' })) -ForegroundColor Yellow

    # 7. Manual items -------------------------------------------------------------
    Write-Host "[MANUAL] Live Claude run (.\scripts\run.ps1 eval) - needs an API key with credits" -ForegroundColor Yellow

    $failedCount = @($results | Where-Object Status -eq 'FAIL').Count
    Write-Host "`nAudit: $($results.Count - $failedCount) PASS, $failedCount FAIL"
    if ($failedCount -gt 0) { exit 1 }
}
finally {
    Pop-Location
}
