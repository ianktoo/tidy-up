<#
.SYNOPSIS
    Undoes add-to-path.ps1: removes tidy-up from your PATH and deletes the installed copy.

.DESCRIPTION
    Removes the install folder from your USER PATH and deletes tidy-up.exe from it (and the folder
    itself, if that leaves it empty). Nothing else on your PATH is changed, and %VARIABLES% in your
    PATH are preserved. Your files are never touched, and the undo journals (.tidy-up folders)
    stay, so you can still restore earlier runs if you install tidy-up again.

        powershell -ExecutionPolicy Bypass -File .\scripts\remove-from-path.ps1

    Safe to run more than once. Use -WhatIf to see what would happen without changing anything.

.PARAMETER InstallDir
    The folder tidy-up was installed to. Default: %LOCALAPPDATA%\Programs\tidy-up

.PARAMETER KeepFiles
    Only remove the PATH entry; leave tidy-up.exe where it is.

.PARAMETER Scope
    User (default) or Process (this PowerShell session only).

.PARAMETER EnvironmentKey
    Testing hook: the registry key under HKCU that holds the PATH. Leave at the default.
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\tidy-up'),
    [switch]$KeepFiles,
    [ValidateSet('User', 'Process')][string]$Scope = 'User',
    [string]$EnvironmentKey = 'Environment'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_path-helpers.ps1')

$InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')

$entries = @(Split-PathList (Get-PathValue $Scope $EnvironmentKey))
$kept = @($entries | Where-Object { -not (Test-SameDirectory $_ $InstallDir) })
if ($kept.Count -eq $entries.Count) {
    Write-Host "$InstallDir was not on your $Scope PATH."
}
elseif ($PSCmdlet.ShouldProcess("$Scope PATH", "Remove $InstallDir")) {
    Set-PathValue $Scope $EnvironmentKey ($kept -join ';')
    Write-Host "Removed $InstallDir from your $Scope PATH."
}

$exe = Join-Path $InstallDir 'tidy-up.exe'
if ($KeepFiles) {
    Write-Host "Kept $exe (-KeepFiles)."
}
elseif (Test-Path -LiteralPath $exe) {
    if ($PSCmdlet.ShouldProcess($exe, 'Delete tidy-up.exe')) {
        try { Remove-Item -LiteralPath $exe -Force }
        catch { throw "Could not delete $exe. Close any running tidy-up and try again. ($($_.Exception.Message))" }
        Write-Host "Deleted $exe"
        # remove the folder only if we left it empty; never delete anything else
        if ((Test-Path -LiteralPath $InstallDir) -and -not (Get-ChildItem -LiteralPath $InstallDir -Force)) {
            Remove-Item -LiteralPath $InstallDir -Force
            Write-Host "Deleted the empty folder $InstallDir"
        }
    }
}
else {
    Write-Host "No tidy-up.exe in $InstallDir."
}

if ($WhatIfPreference) {
    Write-Host 'Nothing was changed (-WhatIf).'
}
else {
    Write-Host ''
    Write-Host 'Done. Open a NEW terminal so the change takes effect.'
}
