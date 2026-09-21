# Shared helpers for add-to-path.ps1 and remove-from-path.ps1. Dot-sourced, not run directly.
# ASCII only on purpose: Windows PowerShell 5.1 misreads non-ASCII in scripts without a BOM.

# The user PATH lives in the registry under HKCU\<Key> (normally "Environment").
# It is read WITHOUT expanding %VARIABLES% and written back as REG_EXPAND_SZ, so entries such
# as %USERPROFILE%\bin survive unchanged. (SetEnvironmentVariable would flatten them.)

function Get-PathValue {
    param([string]$Scope, [string]$Key)
    if ($Scope -eq 'Process') {
        return [string][Environment]::GetEnvironmentVariable('Path', 'Process')
    }
    $reg = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($Key)
    if (-not $reg) { return '' }
    try {
        return [string]$reg.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    }
    finally { $reg.Close() }
}

function Set-PathValue {
    param([string]$Scope, [string]$Key, [string]$Value)
    if ($Scope -eq 'Process') {
        [Environment]::SetEnvironmentVariable('Path', $Value, 'Process')
        return
    }
    $reg = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($Key)
    try { $reg.SetValue('Path', $Value, [Microsoft.Win32.RegistryValueKind]::ExpandString) }
    finally { $reg.Close() }
    # Only the real user environment needs announcing; a scratch key (used by tests) does not.
    if ($Key -eq 'Environment') { Send-EnvironmentChange }
}

# Tells running programs (Explorer, new terminals) that the environment changed.
function Send-EnvironmentChange {
    if (-not ('TidyUp.Native' -as [type])) {
        Add-Type -Namespace TidyUp -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
    }
    $result = [UIntPtr]::Zero
    # HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG
    [void][TidyUp.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
}

function Split-PathList {
    param([string]$Value)
    if (-not $Value) { return @() }
    return @($Value -split ';' | Where-Object { $_ -ne '' })
}

function Test-SameDirectory {
    param([string]$A, [string]$B)
    $x = [Environment]::ExpandEnvironmentVariables($A).TrimEnd('\')
    $y = [Environment]::ExpandEnvironmentVariables($B).TrimEnd('\')
    return ($x -ieq $y)
}
