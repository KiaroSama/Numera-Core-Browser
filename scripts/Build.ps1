#requires -Version 7.0
[CmdletBinding()]
param([ValidateSet('Check', 'Build', 'Doctor')][string]$Mode = 'Check')
$ErrorActionPreference = 'Stop'
$ProjectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $ProjectRoot
try {
    if ($Mode -eq 'Check') {
        & cargo fmt --all -- --check
        if ($LASTEXITCODE) { throw 'Rust formatting failed' }
        & cargo clippy --workspace --all-targets --locked -- -D warnings
        if ($LASTEXITCODE) { throw 'Rust linting failed' }
        & cargo test --workspace --locked
        if ($LASTEXITCODE) { throw 'Rust tests failed' }
    } elseif ($Mode -eq 'Build') {
        & cargo build --release --locked -p core-cli
        if ($LASTEXITCODE) { throw 'CLI build failed' }
    } else {
        & cargo run --locked -q -p core-cli -- --json doctor
        if ($LASTEXITCODE) { throw 'Diagnostics failed' }
    }
} catch {
    [Console]::Error.WriteLine((@{timestamp=[DateTimeOffset]::UtcNow.ToString('o');severity='ERROR';component='build';message='Build command failed; inspect preceding tool diagnostics'} | ConvertTo-Json -Compress))
    exit 1
} finally { Pop-Location }
