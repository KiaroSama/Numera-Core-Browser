#requires -Version 7.0
<# Native source recipe candidate. Unvalidated until hosted cold build passes.
   No external engine objects, browser downloads, remote execution or caches. #>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SourceRoot,
    [ValidateSet('Acquire', 'Build')][string]$Phase = 'Acquire'
)
$ErrorActionPreference = 'Stop'
function Log([string]$Level, [string]$Message) {
    [Console]::Error.WriteLine((@{timestamp=[DateTimeOffset]::UtcNow.ToString('o');severity=$Level;component='chromium-source';message=$Message} | ConvertTo-Json -Compress))
}
function Native([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'Native source-build tool failed' }
}
try {
    if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:GITHUB_REPOSITORY -ne 'KiaroSama/Numera-Core-Browser') {
        throw 'Engine compilation requires the authorized public GitHub-hosted repository runner'
    }
    if ($SourceRoot -match '\s' -or -not [IO.Path]::IsPathFullyQualified($SourceRoot)) { throw 'Provide an absolute scratch source path without spaces' }
    $RepositoryRoot = Split-Path $PSScriptRoot -Parent
    $Lock = Get-Content (Join-Path $RepositoryRoot 'upstream/chromium/lock.json') -Raw | ConvertFrom-Json
    $HostPlatform = if ($IsWindows) { 'windows' } elseif ($IsMacOS) { 'macos' } else { 'linux' }
    $SourcePin = $Lock.platform_pins.$HostPlatform
    if (-not $SourcePin) { throw 'No reviewed source pin for host platform' }
    $Depot = Join-Path $SourceRoot 'depot_tools'
    $Checkout = Join-Path $SourceRoot 'checkout'
    New-Item -ItemType Directory -Path $SourceRoot -Force | Out-Null
    Set-Content (Join-Path $SourceRoot 'phase.txt') 'source-acquisition' -NoNewline
    if (Test-Path $Depot) { throw 'Cold spike requires a fresh task-owned source directory' }
    Log 'INFO' 'Acquiring pinned upstream build tools and full Chromium source dependencies'
    Native -Command 'git' -Arguments @('init', $Depot)
    Native -Command 'git' -Arguments @('-C', $Depot, 'remote', 'add', 'origin', $Lock.depot_tools_url)
    Native -Command 'git' -Arguments @('-C', $Depot, 'fetch', '--depth=1', 'origin', $Lock.depot_tools_revision)
    Native -Command 'git' -Arguments @('-C', $Depot, 'checkout', '--detach', 'FETCH_HEAD')
    # Process-local PATH only; never mutate global PATH or execution policy.
    $env:PATH = $Depot + [IO.Path]::PathSeparator + $env:PATH
    $env:DEPOT_TOOLS_UPDATE = '0'
    $env:DEPOT_TOOLS_WIN_TOOLCHAIN = '0'
    New-Item -ItemType Directory -Path $Checkout -Force | Out-Null
    Push-Location $Checkout
    try {
        Native -Command 'gclient' -Arguments @('config', '--unmanaged', $Lock.upstream_url)
        Native -Command 'gclient' -Arguments @('sync', '--no-history', '--jobs=2', '--revision', ('src@' + $SourcePin.revision))
        Push-Location 'src'
        try {
            $Revision = (& git rev-parse HEAD).Trim()
            if ($Revision -ne $SourcePin.revision) { throw 'Upstream source revision mismatch' }
            Set-Content (Join-Path $SourceRoot 'phase.txt') 'patch-application' -NoNewline
            foreach ($Patch in $Lock.patches) {
                $PatchPath = Join-Path $RepositoryRoot $Patch.path
                if ((Get-FileHash $PatchPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Patch.sha256) { throw 'Patch hash mismatch' }
                Native -Command 'git' -Arguments @('apply', '--check', $PatchPath)
                Native -Command 'git' -Arguments @('apply', $PatchPath)
            }
            if ($Phase -eq 'Build') {
                Set-Content (Join-Path $SourceRoot 'phase.txt') 'gn-generation' -NoNewline
                $Cpu = if ($env:RUNNER_ARCH -eq 'ARM64') { 'arm64' } else { 'x64' }
                $GnArgs = 'is_debug=false is_component_build=false symbol_level=0 blink_symbol_level=0 v8_symbol_level=0 use_remoteexec=false is_official_build=false is_chrome_branded=false proprietary_codecs=false ffmpeg_branding="Chromium" target_cpu="' + $Cpu + '"'
                Native -Command 'gn' -Arguments @('gen', 'out/Numera', ('--args=' + $GnArgs))
                Set-Content (Join-Path $SourceRoot 'phase.txt') 'compile-link' -NoNewline
                Native -Command 'autoninja' -Arguments @('-C', 'out/Numera', '-j', '2', 'chrome')
                Set-Content (Join-Path $SourceRoot 'phase.txt') 'compile-complete' -NoNewline
                Log 'INFO' 'Compilation completed; native regression, source packaging and release approval are still required'
            }
        } finally { Pop-Location }
    } finally { Pop-Location }
} catch {
    Log 'ERROR' 'Chromium source recipe stopped; retain native tool diagnostics and feasibility evidence'
    exit 1
}
