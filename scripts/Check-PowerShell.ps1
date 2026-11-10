#requires -Version 7.0
$ErrorActionPreference = 'Stop'
$Files = Get-ChildItem $PSScriptRoot -Filter '*.ps1'
foreach ($File in $Files) {
    $ParseTokens = $null
    $ParseErrors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($File.FullName, [ref]$ParseTokens, [ref]$ParseErrors) | Out-Null
    if ($ParseErrors.Count -gt 0) { throw 'PowerShell source syntax validation failed' }
}
[Console]::Error.WriteLine((@{timestamp=[DateTimeOffset]::UtcNow.ToString('o');severity='INFO';component='powershell-check';message='PowerShell AST parsing passed; native engine execution is separate'} | ConvertTo-Json -Compress))
