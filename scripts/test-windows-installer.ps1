param(
    [string]$Setup,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
if (-not $env:CI -and -not $Force) {
    throw 'This test installs and uninstalls Octodock for the current user; pass -Force to run it outside CI'
}
if (-not $Setup) {
    $Setup = Get-ChildItem (Join-Path $PSScriptRoot '../target/package') -Filter 'octodock-*-windows-*-setup.exe' |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Setup) { throw 'No octodock-*-setup.exe under target/package' }
$Setup = (Resolve-Path -LiteralPath $Setup).Path
$match = [regex]::Match((Split-Path $Setup -Leaf), '\Aoctodock-(\d+\.\d+\.\d+)-windows-[a-z0-9_]+-setup\.exe\z')
if (-not $match.Success) { throw "Unexpected installer name: $Setup" }
$version = $match.Groups[1].Value
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\io.github.octopuslowtech.octodock_is1'
$protocolKey = 'HKCU:\Software\Classes\octodock'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Octodock.lnk'
$root = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$dir = Join-Path $root "octodock installer test $([guid]::NewGuid().ToString('N'))"

function Invoke-Checked([string]$File, [string[]]$Arguments, [string]$What) {
    $process = Start-Process -FilePath $File -ArgumentList $Arguments -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$What exited with $($process.ExitCode)" }
}

function Wait-Until([scriptblock]$Condition, [string]$What) {
    $deadline = (Get-Date).AddSeconds(90)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "Timed out waiting for $What" }
        Start-Sleep -Milliseconds 250
    }
}

$log = Join-Path $root 'octodock-setup.log'
Invoke-Checked $Setup @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", "/LOG=`"$log`"") 'Setup'
$exe = Join-Path $dir 'octodock.exe'
foreach ($file in @('octodock.exe', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'licenses/fonts', 'unins000.exe')) {
    if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { throw "Installed file missing: $file" }
}
Write-Output 'PASS: installed files'


$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = $exe
$info.Arguments = '--version'
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($info)
try {
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $null = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit(10000)) { $process.Kill(); throw 'Installed version probe timed out' }
    if ($process.ExitCode -ne 0 -or $stdout.Result.Trim() -ne "octodock $version") {
        throw "Installed executable reports '$($stdout.Result.Trim())', expected 'octodock $version'"
    }
} finally { $process.Dispose() }
Write-Output "PASS: installed executable is $version"

$entry = Get-ItemProperty -LiteralPath $uninstallKey
if ($entry.DisplayVersion -ne $version) { throw "DisplayVersion is '$($entry.DisplayVersion)', expected '$version'" }
if ($entry.DisplayName -ne 'Octodock') { throw "DisplayName is '$($entry.DisplayName)'" }
$installed = [IO.Path]::GetFullPath($entry.InstallLocation).TrimEnd('\')
if ($installed -ne [IO.Path]::GetFullPath($dir).TrimEnd('\')) { throw "InstallLocation is '$installed'" }
$command = (Get-ItemProperty -LiteralPath "$protocolKey\shell\open\command").'(default)'
if ($command -ne "`"$exe`" `"%1`"") { throw "octodock:// handler is '$command'" }
if (-not (Test-Path -LiteralPath $shortcut)) { throw "Start menu shortcut missing: $shortcut" }
Write-Output 'PASS: uninstall entry, octodock:// handler, Start menu shortcut'

Invoke-Checked (Join-Path $dir 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') 'Uninstall'
Wait-Until { -not (Test-Path -LiteralPath $uninstallKey) } 'the uninstall entry to disappear'
Wait-Until { -not (Test-Path -LiteralPath $exe) } 'octodock.exe to be removed'
foreach ($leftover in @('licenses')) {
    Wait-Until { -not (Test-Path -LiteralPath (Join-Path $dir $leftover)) } "$leftover to be removed"
}
if (Test-Path -LiteralPath $protocolKey) { throw 'octodock:// handler survived uninstall' }
if (Test-Path -LiteralPath $shortcut) { throw 'Start menu shortcut survived uninstall' }
Write-Output 'PASS: uninstall removes the install and registrations'
