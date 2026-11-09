# Remove the LightCraft plugin from Logi Options+ (Logi Plugin Service) on this PC.
# Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
#
#   powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1 [-NoRestart]
#
# Removes only LightCraft's own folders (the plugin, its data and pairing token, its Options+
# profiles). Other plugins, their profiles and Options+ settings are not touched. In LightCraft,
# Settings, Controls, Forget the paired plugin as well.

param([switch]$NoRestart)
$ErrorActionPreference = 'Stop'
$logi = Join-Path $env:LOCALAPPDATA 'Logi\LogiPluginService'
$serviceExe = Join-Path ${env:ProgramFiles} 'Logi\LogiPluginService\LogiPluginService.exe'

if (-not $NoRestart) {
    foreach ($name in 'LogiPluginServiceExt', 'LogiPluginService') {
        Get-Process -Name $name -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force; $_.WaitForExit(5000) | Out-Null }
    }
}
$targets = @(
    (Join-Path $logi 'Plugins\LightCraft'),
    (Join-Path $logi 'PluginData\LightCraft')
) + (Get-ChildItem (Join-Path $logi 'Applications') -Directory -ErrorAction SilentlyContinue | ForEach-Object { Join-Path $_.FullName '@_lightcraft' })
foreach ($t in $targets) {
    if (Test-Path $t) { Remove-Item $t -Recurse -Force; Write-Host "removed $t" }
}
if (-not $NoRestart) {
    for ($i = 0; $i -lt 30; $i++) {
        if (Get-Process -Name 'LogiPluginService' -ErrorAction SilentlyContinue) { break }
        Start-Sleep -Milliseconds 500
    }
    if (-not (Get-Process -Name 'LogiPluginService' -ErrorAction SilentlyContinue) -and (Test-Path $serviceExe)) {
        Start-Process -FilePath $serviceExe -WorkingDirectory (Split-Path $serviceExe)
    }
}
Write-Host 'The LightCraft plugin is removed.'
