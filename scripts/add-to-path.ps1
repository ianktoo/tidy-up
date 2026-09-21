<#
.SYNOPSIS
    Installs tidy-up for the current user and adds it to your PATH.

.DESCRIPTION
    Copies tidy-up.exe into a folder under your profile and adds that folder to your USER
    PATH, so `tidy-up` works from any terminal. No administrator rights needed.
    Undo it with remove-from-path.ps1.

    Run it from the extracted release folder:
        powershell -ExecutionPolicy Bypass -File .\scripts\add-to-path.ps1

    Safe to run more than once. Use -WhatIf to see what would happen without changing anything.

.PARAMETER InstallDir
    Where to install. Default: %LOCALAPPDATA%\Programs\tidy-up

.PARAMETER Binary
    Path to tidy-up.exe. Default: the one next to this script's folder.

.PARAMETER Scope
    User (default, permanent) or Process (this PowerShell session only).

.PARAMETER NoCopy
    Do not copy anything; only add InstallDir to PATH.

.PARAMETER EnvironmentKey
    Testing hook: the registry key under HKCU that holds the PATH. Leave at the default.
#>
[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\tidy-up'),
    [string]$Binary,
    [ValidateSet('User', 'Process')][string]$Scope = 'User',
    [switch]$NoCopy,
    [string]$EnvironmentKey = 'Environment'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_path-helpers.ps1')

$InstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')

if (-not $NoCopy) {
    if (-not $Binary) {
        foreach ($candidate in @((Join-Path $PSScriptRoot '..\tidy-up.exe'), (Join-Path $PSScriptRoot 'tidy-up.exe'))) {
            if (Test-Path -LiteralPath $candidate) { $Binary = $candidate; break }
        }
    }
    if (-not $Binary -or -not (Test-Path -LiteralPath $Binary)) {
        throw 'Cannot find tidy-up.exe. Run this from the extracted release folder, or pass -Binary <path>.'
    }
    $source = (Resolve-Path -LiteralPath $Binary).Path
    $target = Join-Path $InstallDir 'tidy-up.exe'

    if ($PSCmdlet.ShouldProcess($target, 'Copy tidy-up.exe')) {
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        if ($source -ieq $target) {
            Write-Host "tidy-up.exe is already in $InstallDir"
        }
        else {
            try { Copy-Item -LiteralPath $source -Destination $target -Force }
            catch { throw "Could not copy to $target. Close any running tidy-up and try again. ($($_.Exception.Message))" }
            # removes the "downloaded from the internet" mark so SmartScreen does not prompt
            Unblock-File -LiteralPath $target -ErrorAction SilentlyContinue
            Write-Host "Copied tidy-up.exe to $InstallDir"
        }
    }
}

$entries = @(Split-PathList (Get-PathValue $Scope $EnvironmentKey))
if (@($entries | Where-Object { Test-SameDirectory $_ $InstallDir }).Count -gt 0) {
    Write-Host "$InstallDir is already on your $Scope PATH."
}
elseif ($PSCmdlet.ShouldProcess("$Scope PATH", "Add $InstallDir")) {
    Set-PathValue $Scope $EnvironmentKey (($entries + $InstallDir) -join ';')
    Write-Host "Added $InstallDir to your $Scope PATH."
}

if ($WhatIfPreference) {
    Write-Host 'Nothing was changed (-WhatIf).'
}
else {
    Write-Host ''
    Write-Host 'Done. Open a NEW terminal, then check it:  tidy-up --version'
    Write-Host 'To undo all of this:  powershell -ExecutionPolicy Bypass -File .\scripts\remove-from-path.ps1'
}
