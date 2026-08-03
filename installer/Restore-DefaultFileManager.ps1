# FerroxPlorer default file manager recovery script.
# It only restores HKCU associations which are still marked as owned by
# FerroxPlorer and still point to FerroxPlorer.exe. Other file managers are
# never overwritten.

$ErrorActionPreference = 'Stop'

$backupPath = 'Software\FerroxPlorer\DefaultFileManager'
$ownerValue = 'FerroxPlorerOwner'
$targets = @(
    [PSCustomObject]@{ Id = 'Directory'; VerbKey = 'Software\Classes\Directory\shell\open'; WithTarget = $true },
    [PSCustomObject]@{ Id = 'Drive'; VerbKey = 'Software\Classes\Drive\shell\open'; WithTarget = $true },
    [PSCustomObject]@{ Id = 'ThisPcOpen'; VerbKey = 'Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open'; WithTarget = $false },
    [PSCustomObject]@{ Id = 'ThisPcOpenNewWindow'; VerbKey = 'Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow'; WithTarget = $false }
)

function Get-Flag([Microsoft.Win32.RegistryKey]$key, [string]$name) {
    return [int]($key.GetValue($name, 0)) -eq 1
}

function Restore-Value(
    [Microsoft.Win32.RegistryKey]$destination,
    [Microsoft.Win32.RegistryKey]$backup,
    [string]$existedName,
    [string]$backupName,
    [string]$targetName
) {
    if (Get-Flag $backup $existedName) {
        $value = $backup.GetValue($backupName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $destination.SetValue($targetName, $value, $backup.GetValueKind($backupName))
    } else {
        $destination.DeleteValue($targetName, $false)
    }
}

function Is-FerroxPlorerCommand([string]$command, [bool]$withTarget) {
    $pattern = if ($withTarget) { '^"([^"]+)" "%1"$' } else { '^"([^"]+)"$' }
    if ($command -notmatch $pattern) {
        return $false
    }
    return [System.IO.Path]::GetFileName($Matches[1]).Equals('ferroxplorer.exe', [System.StringComparison]::OrdinalIgnoreCase)
}

function Remove-EmptySubKey([Microsoft.Win32.RegistryKey]$parent, [string]$child) {
    try {
        $parent.DeleteSubKey($child, $false)
    } catch [System.IO.IOException] {
        # Keep keys that contain values or subkeys created by the user.
    }
}

$hkcu = [Microsoft.Win32.Registry]::CurrentUser
$backupRoot = $hkcu.OpenSubKey($backupPath, $false)
if ($null -eq $backupRoot -or -not (Get-Flag $backupRoot 'Active')) {
    Write-Host 'No FerroxPlorer default-file-manager backup was found. No registry changes were made.' -ForegroundColor Yellow
    exit 0
}

$restored = 0
$skipped = 0
$failed = 0

foreach ($target in $targets) {
    $commandPath = "$($target.VerbKey)\command"
    $command = $hkcu.OpenSubKey($commandPath, $true)
    if ($null -eq $command) {
        Write-Warning "$($target.Id): command is missing; skipped."
        $skipped++
        continue
    }

    $commandText = [string]$command.GetValue('', '')
    if (-not (Get-Flag $command $ownerValue) -or -not (Is-FerroxPlorerCommand $commandText $target.WithTarget)) {
        Write-Host "$($target.Id): not owned by FerroxPlorer; kept unchanged." -ForegroundColor Yellow
        $command.Close()
        $skipped++
        continue
    }

    $backup = $backupRoot.OpenSubKey($target.Id, $false)
    if ($null -eq $backup) {
        Write-Warning "$($target.Id): backup is missing; skipped."
        $command.Close()
        $failed++
        continue
    }

    try {
        $commandExisted = Get-Flag $backup 'CommandExisted'
        $verbExisted = Get-Flag $backup 'VerbExisted'
        Restore-Value $command $backup 'DefaultExisted' 'DefaultValue' ''
        Restore-Value $command $backup 'DelegateExisted' 'DelegateValue' 'DelegateExecute'
        $command.DeleteValue($ownerValue, $false)
        $backup.Close()
        $command.Close()

        if (-not $commandExisted) {
            $parts = $commandPath.LastIndexOf('\')
            $parent = $hkcu.OpenSubKey($commandPath.Substring(0, $parts), $true)
            if ($null -ne $parent) {
                Remove-EmptySubKey $parent $commandPath.Substring($parts + 1)
                $parent.Close()
            }
        }
        if (-not $verbExisted) {
            $parts = $target.VerbKey.LastIndexOf('\')
            $parent = $hkcu.OpenSubKey($target.VerbKey.Substring(0, $parts), $true)
            if ($null -ne $parent) {
                Remove-EmptySubKey $parent $target.VerbKey.Substring($parts + 1)
                $parent.Close()
            }
        }

        Write-Host "$($target.Id): restored." -ForegroundColor Green
        $restored++
    } catch {
        Write-Warning "$($target.Id): recovery failed: $($_.Exception.Message)"
        if ($null -ne $backup) { $backup.Close() }
        if ($null -ne $command) { $command.Close() }
        $failed++
    }
}

$backupRoot.Close()

if ($failed -eq 0 -and $skipped -eq 0) {
    try {
        $hkcu.DeleteSubKeyTree($backupPath, $false)
    } catch {
        Write-Warning "Recovery completed, but backup cleanup failed: $($_.Exception.Message)"
    }
} else {
    Write-Warning 'Some entries were skipped or failed. Backup data was kept for inspection.'
}

if ($restored -gt 0) {
    Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class FerroxPlorerShellNotify { [DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, uint flags, IntPtr item1, IntPtr item2); }'
    [FerroxPlorerShellNotify]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
    Write-Host 'Windows file associations were refreshed.' -ForegroundColor Green
}

if ($failed -gt 0) {
    exit 1
}
