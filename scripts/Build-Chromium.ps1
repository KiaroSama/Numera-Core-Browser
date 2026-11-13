#requires -Version 7.0
<# Native source recipe candidate. Unvalidated until hosted cold build passes.
   No external engine objects, browser downloads, remote execution or caches. #>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$SourceRoot,
    [ValidateSet('Prerequisites', 'Acquire', 'Build')][string]$Phase = 'Acquire',
    [ValidateRange(1,4)][int]$Parallelism = 2,
    [switch]$ComponentBuild,
    [switch]$DebugBuild,
    [switch]$BuildDriver
)
$ErrorActionPreference = 'Stop'
function Log([string]$Level, [string]$Message) {
    [Console]::Error.WriteLine((@{timestamp=[DateTimeOffset]::UtcNow.ToString('o');severity=$Level;component='chromium-source';message=$Message} | ConvertTo-Json -Compress))
}
function Native([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'Native source-build tool failed' }
}
function Stage([string]$Name) {
    Set-Content (Join-Path $SourceRoot 'phase.txt') $Name -NoNewline -Encoding utf8NoBOM
    $Event = @{timestamp=[DateTimeOffset]::UtcNow.ToString('o');phase=$Name} | ConvertTo-Json -Compress
    Add-Content (Join-Path $SourceRoot 'phases.ndjson') $Event -Encoding utf8NoBOM
    Log 'INFO' ('Build phase: ' + $Name)
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
    if ($IsWindows) {
        Stage 'windows-sdk-preparation'
        & (Join-Path $PSScriptRoot 'Install-WindowsSdk.ps1') -EvidenceRoot $SourceRoot
    }
    if ($Phase -eq 'Prerequisites' -and $IsWindows) {
        Stage 'prerequisites-complete'
        exit 0
    }
    if ($IsMacOS) {
        Stage 'mac-toolchain-preparation'
        # Use the reviewed newest preinstalled Intel-host Xcode in this image.
        # DEVELOPER_DIR affects this process only, not global xcode-select.
        $MacDeveloper = '/Applications/Xcode_26.3.app/Contents/Developer'
        if (-not (Test-Path $MacDeveloper -PathType Container)) { throw 'Reviewed Intel-host Xcode 26.3 is not installed' }
        $env:DEVELOPER_DIR = $MacDeveloper
        $XcodeVersion = (& xcodebuild -version) -join "`n"
        if ($LASTEXITCODE -ne 0 -or $XcodeVersion -notmatch '(?m)^Xcode 26\.3$') { throw 'Reviewed Xcode version check failed' }
        $MacSdk = (& xcrun --sdk macosx --show-sdk-version).Trim()
        if ($LASTEXITCODE -ne 0 -or $MacSdk -ne '26.2') { throw 'Reviewed macOS SDK 26.2 check failed' }
        $MacSdkPath = (& xcrun --sdk macosx --show-sdk-path).Trim()
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $MacSdkPath -PathType Container)) { throw 'Selected macOS SDK is unavailable' }
        @{developer_dir=$MacDeveloper;xcode_version=$XcodeVersion;sdk_version=$MacSdk;sdk_path=$MacSdkPath;official_sdk_equivalence=$false;release_ready=$false} |
            ConvertTo-Json | Set-Content (Join-Path $SourceRoot 'mac-toolchain.json') -Encoding utf8NoBOM
    }
    Stage 'build-tool-acquisition'
    if (Test-Path $Depot) { throw 'Cold spike requires a fresh task-owned source directory' }
    Log 'INFO' 'Acquiring pinned upstream build tools'
    Native -Command 'git' -Arguments @('init', $Depot)
    Native -Command 'git' -Arguments @('-C', $Depot, 'remote', 'add', 'origin', $Lock.depot_tools_url)
    Native -Command 'git' -Arguments @('-C', $Depot, 'fetch', '--depth=1', 'origin', $Lock.depot_tools_revision)
    Native -Command 'git' -Arguments @('-C', $Depot, 'checkout', '--detach', 'FETCH_HEAD')
    # Process-local PATH only; never mutate global PATH or execution policy.
    $env:PATH = $Depot + [IO.Path]::PathSeparator + $env:PATH
    $env:DEPOT_TOOLS_UPDATE = '0'
    $env:DEPOT_TOOLS_WIN_TOOLCHAIN = '0'
    if ($IsWindows) {
        # Keep Git settings local to this build process and bootstrap .bat tools
        # through cmd as required by the pinned upstream Windows instructions.
        $Config = Join-Path $SourceRoot 'build.gitconfig'
        Set-Content $Config "[core]`n autocrlf = false`n longpaths = true`n preloadindex = true`n fscache = true" -Encoding utf8NoBOM
        $env:GIT_CONFIG_GLOBAL = $Config
        $VsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (Test-Path $VsWhere) {
            $VsPath = (& $VsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
            if ($VsPath) { $env:vs2026_install = $VsPath }
        }
        # DEPOT_TOOLS_UPDATE=0 deliberately bypasses update_depot_tools.bat,
        # including its Python bootstrap. Run the pinned bootstrap explicitly;
        # never enable the self-updater, which would replace our reviewed pin.
        Native -Command 'cmd.exe' -Arguments @('/d', '/c', (Join-Path $Depot 'cipd_bin_setup.bat'))
        Native -Command 'cmd.exe' -Arguments @('/d', '/c', (Join-Path $Depot 'bootstrap/win_tools.bat'))
        if (-not (Test-Path (Join-Path $Depot 'python3_bin_reldir.txt') -PathType Leaf)) {
            throw 'Pinned Windows Python bootstrap did not create its launcher marker'
        }
        Native -Command 'cmd.exe' -Arguments @('/d', '/c', ((Join-Path $Depot 'python-bin/python3.bat') + ' --version'))
    } else {
        Stage 'pinned-python-bootstrap'
        # The pinned helper downloads tools without updating the repository.
        # DEPOT_TOOLS_UPDATE=0 otherwise skips this POSIX Python bootstrap.
        Native -Command 'bash' -Arguments @((Join-Path $Depot 'ensure_bootstrap'))
        if (-not (Test-Path (Join-Path $Depot 'python3_bin_reldir.txt') -PathType Leaf)) {
            throw 'Pinned POSIX Python bootstrap did not create its launcher marker'
        }
        Native -Command (Join-Path $Depot 'python-bin/python3') -Arguments @('--version')
    }
    $DepotRevision = (& git -C $Depot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $DepotRevision -ne $Lock.depot_tools_revision) { throw 'Build-tool revision changed during bootstrap' }
    if ($Phase -eq 'Prerequisites') {
        Stage 'prerequisites-complete'
        exit 0
    }
    New-Item -ItemType Directory -Path $Checkout -Force | Out-Null
    Push-Location $Checkout
    try {
        Stage 'source-acquisition'
        Native -Command 'gclient' -Arguments @('config', '--unmanaged', $Lock.upstream_url)
        Native -Command 'gclient' -Arguments @('sync', '--no-history', '--jobs=2', '--revision', ('src@' + $SourcePin.revision))
        Push-Location 'src'
        try {
            $Revision = (& git rev-parse HEAD).Trim()
            if ($Revision -ne $SourcePin.revision) { throw 'Upstream source revision mismatch' }
            Stage 'patch-application'
            foreach ($Patch in $Lock.patches) {
                $PatchPath = Join-Path $RepositoryRoot $Patch.path
                if ((Get-FileHash $PatchPath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Patch.sha256) { throw 'Patch hash mismatch' }
                Native -Command 'git' -Arguments @('apply', '--check', $PatchPath)
                Native -Command 'git' -Arguments @('apply', $PatchPath)
            }
            if ($IsLinux -and $Phase -eq 'Build') {
                Stage 'linux-build-dependencies'
                Native -Command 'sudo' -Arguments @('-n', '--', 'python3', 'build/install-build-deps.py', '--no-prompt', '--no-android', '--no-arm', '--no-chromeos-fonts', '--no-backwards-compatible', '--no-syms')
            }
            if ($Phase -eq 'Build') {
                Stage 'gn-generation'
                $Cpu = if ($env:RUNNER_ARCH -eq 'ARM64') { 'arm64' } else { 'x64' }
                $Component = if ($ComponentBuild) { 'true' } else { 'false' }
                $Debug = if ($DebugBuild) { 'true' } else { 'false' }
                $GnArgs = 'is_debug=' + $Debug + ' is_component_build=' + $Component + ' symbol_level=0 blink_symbol_level=0 v8_symbol_level=0 use_remoteexec=false is_official_build=false is_chrome_branded=false proprietary_codecs=false ffmpeg_branding="Chromium" target_cpu="' + $Cpu + '"'
                Set-Content (Join-Path $SourceRoot 'gn-args.txt') $GnArgs -Encoding utf8NoBOM
                # Windows .bat launchers consume quotes inside --args. A native
                # args.gn file preserves GN string literals on every platform.
                New-Item -ItemType Directory -Path 'out/Numera' -Force | Out-Null
                Set-Content 'out/Numera/args.gn' $GnArgs -Encoding utf8NoBOM
                Native -Command 'gn' -Arguments @('gen', 'out/Numera')
                Stage 'compile-link'
                $Targets = @('-C', 'out/Numera', '-j', $Parallelism.ToString(), 'chrome')
                if ($BuildDriver) { $Targets += 'chromedriver' }
                if ($IsLinux) { $Targets += 'chrome_sandbox' }
                Native -Command 'autoninja' -Arguments $Targets
                Stage 'compile-complete'
                Log 'INFO' 'Compilation completed; native regression, source packaging and release approval are still required'
            }
        } finally { Pop-Location }
    } finally { Pop-Location }
} catch {
    Log 'ERROR' ('Chromium source recipe stopped at line ' + $_.InvocationInfo.ScriptLineNumber + '; error type: ' + $_.Exception.GetType().Name + '; retain native diagnostics')
    exit 1
}
