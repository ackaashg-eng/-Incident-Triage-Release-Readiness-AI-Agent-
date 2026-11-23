<#
.SYNOPSIS
    Builds the workspace (if anything changed) and starts the agent from the
    project folder, so .env and relative paths resolve correctly.

.EXAMPLE
    .\scripts\run.ps1
#>
[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$AgentArgs
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot

Push-Location -LiteralPath $Root
try {
    & cargo build --workspace --quiet
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    & (Join-Path $Root 'target\debug\agent-client.exe') @AgentArgs
    exit $LASTEXITCODE
}
finally {
    Pop-Location
}
