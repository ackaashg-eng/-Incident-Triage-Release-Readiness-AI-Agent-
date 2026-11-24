<#
.SYNOPSIS
    One-time setup: checks prerequisites, installs the pinned filesystem MCP
    server, builds the Rust workspace and stores your Anthropic API key in .env.

.DESCRIPTION
    Safe to run again at any time. An existing key in .env is kept unless you
    pass -ResetKey. The key is typed in hidden and is never printed.

.PARAMETER ResetKey
    Ask for a new API key even if .env already contains one.

.PARAMETER SkipKey
    Do not ask for a key (for automated checks).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\setup.ps1
#>
[CmdletBinding()]
param(
    [switch]$ResetKey,
    [switch]$SkipKey
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$MinRust = [version]'1.88'
$MinNodeMajor = 24
$Placeholder = 'paste-your-anthropic-api-key-here'
$problems = New-Object System.Collections.Generic.List[string]

function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Ok([string]$Message)   { Write-Host "    OK  $Message" -ForegroundColor Green }
function Write-Warn([string]$Message) { Write-Host "    !!  $Message" -ForegroundColor Yellow }

Push-Location -LiteralPath $Root
try {
    # ------------------------------------------------------------------
    Write-Step 'Checking prerequisites'
    # ------------------------------------------------------------------
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw 'Rust (cargo) was not found. Install it from https://rustup.rs and re-open PowerShell.'
    }
    $rustText = (& rustc --version)
    if ($rustText -match 'rustc (\d+\.\d+)') {
        $rustVersion = [version]$Matches[1]
        if ($rustVersion -lt $MinRust) { throw "Rust $rustVersion is too old; need $MinRust or newer. Run: rustup update" }
        Write-Ok $rustText
    } else {
        throw "Could not read the Rust version from: $rustText"
    }

    $nodeOk = $false
    if (Get-Command node -ErrorAction SilentlyContinue) {
        $nodeText = (& node --version)
        if ($nodeText -match '^v(\d+)\.' -and [int]$Matches[1] -ge $MinNodeMajor) {
            Write-Ok "Node.js $nodeText"
            $nodeOk = $true
        } else {
            Write-Warn "Node.js $nodeText found, but the filesystem MCP server needs Node $MinNodeMajor or newer."
            $problems.Add("Upgrade Node.js to $MinNodeMajor+ (https://nodejs.org, LTS installer), then run setup again.")
        }
    } else {
        Write-Warn 'Node.js was not found.'
        $problems.Add("Install Node.js $MinNodeMajor+ (https://nodejs.org, LTS installer), then run setup again.")
    }

    # ------------------------------------------------------------------
    Write-Step 'Installing the pinned filesystem MCP server (npm)'
    # ------------------------------------------------------------------
    if ($nodeOk) {
        if (Test-Path 'package-lock.json') { & npm ci } else { & npm install }
        if ($LASTEXITCODE -ne 0) { throw "npm failed with exit code $LASTEXITCODE" }
        Write-Ok '@modelcontextprotocol/server-filesystem installed'
    } else {
        Write-Warn 'Skipped (Node.js requirement not met).'
    }

    # ------------------------------------------------------------------
    Write-Step 'Building the Rust workspace (first build can take a few minutes)'
    # ------------------------------------------------------------------
    & cargo build --workspace
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
    Write-Ok 'cargo build --workspace'

    # ------------------------------------------------------------------
    Write-Step 'Configuring .env'
    # ------------------------------------------------------------------
    $envFile = Join-Path $Root '.env'
    if (-not (Test-Path -LiteralPath $envFile)) {
        Copy-Item -LiteralPath (Join-Path $Root '.env.example') -Destination $envFile
        Write-Ok '.env created from .env.example'
    }

    $lines = @([System.IO.File]::ReadAllLines($envFile))
    $keyLine = $lines | Where-Object { $_ -match '^\s*ANTHROPIC_API_KEY\s*=' } | Select-Object -First 1
    $hasRealKey = $false
    if ($keyLine -and $keyLine -match '=\s*(\S+)\s*$') { $hasRealKey = ($Matches[1] -ne $Placeholder) }

    if ($SkipKey) {
        Write-Warn 'API key prompt skipped (-SkipKey).'
    } elseif ($hasRealKey -and -not $ResetKey) {
        Write-Ok 'An API key is already stored in .env (use -ResetKey to replace it).'
    } else {
        $secure = Read-Host -AsSecureString 'Paste your Anthropic API key (input is hidden), then press Enter'
        $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
        try { $key = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr).Trim() }
        finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }

        if (-not $key) {
            $problems.Add('No API key was entered. Run setup again (or edit .env).')
        } else {
            if ($key -notmatch '^sk-ant-') { Write-Warn 'That does not look like an Anthropic key (expected sk-ant-...). Saved anyway.' }
            $found = $false
            $newLines = @(foreach ($line in $lines) {
                if ($line -match '^\s*ANTHROPIC_API_KEY\s*=') { $found = $true; "ANTHROPIC_API_KEY=$key" } else { $line }
            })
            if (-not $found) { $newLines = @("ANTHROPIC_API_KEY=$key") + $newLines }
            # UTF-8 WITHOUT a byte-order mark: a BOM would corrupt the first variable name.
            [System.IO.File]::WriteAllLines($envFile, [string[]]$newLines, (New-Object System.Text.UTF8Encoding($false)))
            Write-Ok 'API key saved to .env (git-ignored)'
        }
        Remove-Variable key, secure -ErrorAction SilentlyContinue
    }

    # ------------------------------------------------------------------
    Write-Step 'Checks'
    # ------------------------------------------------------------------
    $agent = Join-Path $Root 'target\debug\agent-client.exe'
    # Both MCP servers: start, discover, read the workspace files, refuse escapes (no key needed).
    & $agent check-mcp
    if ($LASTEXITCODE -ne 0) { $problems.Add('The MCP server check failed; see the [FAIL] lines above.') }
    if ($SkipKey) {
        Write-Warn 'Configuration check skipped (no key requested).'
    } else {
        # Loads .env and validates every setting; the key is shown as ***.
        & $agent config
        if ($LASTEXITCODE -ne 0) { $problems.Add('The configuration is not valid; see the error above.') }
    }
}
finally {
    Pop-Location
}

Write-Host ''
if ($problems.Count -gt 0) {
    Write-Host 'Setup finished with open items:' -ForegroundColor Yellow
    foreach ($p in $problems) { Write-Host "  - $p" -ForegroundColor Yellow }
    exit 1
}
Write-Host 'Setup complete. Start the agent with:  .\scripts\run.ps1' -ForegroundColor Green
exit 0
