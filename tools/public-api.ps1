param(
    [ValidateSet('check', 'update')]
    [string] $Mode = 'check'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$snapshotRoot = Join-Path $PSScriptRoot 'public-api'
$manifest = Join-Path $repoRoot 'Cargo.toml'
# cargo-public-api 0.52 reads the rustdoc JSON of this pinned nightly; CI installs the same one.
# Library crates only; ai6ctl is a binary with no public API.
$packages = @(
    'ai6',
    'ai6-diptel',
    'universal-exports',
    'ai6-agent-claude',
    'ai6-agent-codex',
    'ai6-terminal-pty',
    'ai6-safehouse-git',
    'ai6-safehouse-copy',
    'ai6-source-github',
    'ai6-source-jira'
)

if ($Mode -eq 'update') {
    New-Item -ItemType Directory -Path $snapshotRoot -Force | Out-Null
}

foreach ($name in $packages) {
    $snapshotPath = Join-Path $snapshotRoot "$name.txt"
    $arguments = @(
        '+nightly-2026-09-21', 'public-api',
        '--manifest-path', $manifest,
        '--package', $name,
        '--all-features', '-sss', '--color', 'never'
    )
    $lines = & cargo @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Public API extraction failed for $name"
    }
    $expected = [string]::Join("`n", $lines) + "`n"

    if ($Mode -eq 'update') {
        [System.IO.File]::WriteAllText(
            $snapshotPath,
            $expected,
            [System.Text.UTF8Encoding]::new($false)
        )
        Write-Host "Updated $name"
        continue
    }

    if (-not (Test-Path -LiteralPath $snapshotPath)) {
        throw "Missing public API snapshot $snapshotPath; run tools/public-api.ps1 -Mode update"
    }
    $current = [System.IO.File]::ReadAllText($snapshotPath)
    if ($current -cne $expected) {
        throw "Public API snapshot differs for $name; run tools/public-api.ps1 -Mode update and review the diff"
    }
    Write-Host "Verified $name"
}
