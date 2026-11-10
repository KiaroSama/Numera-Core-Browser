#requires -Version 7.0
<# Hosted build prerequisite only. Never runs on a user's installed Core. #>
[CmdletBinding()]
param([Parameter(Mandatory)][string]$EvidenceRoot)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or $env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:GITHUB_REPOSITORY -ne 'KiaroSama/Numera-Core-Browser') {
    throw 'SDK preparation requires the authorized Windows GitHub-hosted runner'
}
$RepositoryRoot = Split-Path $PSScriptRoot -Parent
$Lock = Get-Content (Join-Path $RepositoryRoot 'upstream/windows-sdk/lock.json') -Raw | ConvertFrom-Json
$Kit = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10'
$Version = $Lock.sdk_directory_version
$Report = @{requested_release=$Lock.release;directory_version=$Version;installer_sha256=$Lock.sha256;installed_by_task=$false;ready=$false}
function SdkStage([string]$Name) {
    Set-Content (Join-Path $EvidenceRoot 'phase.txt') $Name -NoNewline -Encoding utf8NoBOM
    $Event = @{timestamp=[DateTimeOffset]::UtcNow.ToString('o');phase=$Name} | ConvertTo-Json -Compress
    Add-Content (Join-Path $EvidenceRoot 'phases.ndjson') $Event -Encoding utf8NoBOM
    [Console]::Error.WriteLine($Event)
}
function CompleteSdk {
    foreach ($Directory in @('um', 'shared', 'ucrt', 'winrt', 'cppwinrt')) {
        if (-not (Test-Path (Join-Path $Kit "Include/$Version/$Directory") -PathType Container)) { return $false }
    }
    foreach ($Directory in @('um/x86', 'um/x64', 'ucrt/x86', 'ucrt/x64')) {
        if (-not (Test-Path (Join-Path $Kit "Lib/$Version/$Directory") -PathType Container)) { return $false }
    }
    if (-not (Test-Path (Join-Path $Kit "bin/$Version/x64/rc.exe") -PathType Leaf)) { return $false }
    $DebugHelp = Join-Path $Kit 'Debuggers/x64/dbghelp.dll'
    if (-not (Test-Path $DebugHelp -PathType Leaf)) { return $false }
    $DebugVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($DebugHelp)
    return ($DebugVersion.FileBuildPart -gt 26100 -or ($DebugVersion.FileBuildPart -eq 26100 -and $DebugVersion.FilePrivatePart -ge 3323))
}
try {
    if (-not (CompleteSdk)) {
        $Installer = Join-Path $EvidenceRoot 'winsdksetup.exe'
        SdkStage 'windows-sdk-download'
        Invoke-WebRequest -Uri $Lock.resolved_url -OutFile $Installer
        if ((Get-FileHash $Installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Lock.sha256) { throw 'SDK installer hash mismatch' }
        SdkStage 'windows-sdk-publisher-verification'
        $Signature = Get-AuthenticodeSignature $Installer
        if ($Signature.Status -ne 'Valid' -or $Signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation(?:,|$)') {
            throw 'SDK installer publisher verification failed'
        }
        $Report.installer_signer = $Signature.SignerCertificate.Subject
        # Include the native x86/x64/ARM64 and UWP libraries and debugging tools
        # required by upstream. Install only on this disposable hosted machine.
        $LogPath = Join-Path $EvidenceRoot 'windows-sdk.log'
        SdkStage 'windows-sdk-installation'
        $Process = Start-Process -FilePath $Installer -ArgumentList @('/features', '+', '/quiet', '/norestart', '/log', $LogPath) -PassThru
        if (-not $Process.WaitForExit(900000)) {
            $Process.Kill($true)
            throw 'SDK installer exceeded its fifteen-minute bound'
        }
        $Report.installer_exit_code = $Process.ExitCode
        if ($Process.ExitCode -notin @(0, 3010)) { throw 'SDK prerequisite installation failed' }
        $Report.installed_by_task = $true
        $Report.applied_installer_release = $Lock.release
    }
    SdkStage 'windows-sdk-validation'
    if (-not (CompleteSdk)) { throw 'Required complete SDK and debugging tools are still unavailable' }
    $DebugHelp = Join-Path $Kit 'Debuggers/x64/dbghelp.dll'
    if (-not (Test-Path $DebugHelp -PathType Leaf)) { throw 'Required SDK debugging tools are unavailable' }
    $DebugVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($DebugHelp)
    $Report.debugging_tools_version = $DebugVersion.FileVersion
    if ($DebugVersion.FileBuildPart -lt 26100 -or ($DebugVersion.FileBuildPart -eq 26100 -and $DebugVersion.FilePrivatePart -lt 3323)) {
        throw 'SDK debugging tools are older than the Chromium requirement'
    }
    $Report.ready = $true
} finally {
    $Report | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $EvidenceRoot 'windows-sdk.json') -Encoding utf8NoBOM
}
