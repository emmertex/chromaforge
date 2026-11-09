# Install the Chromaforge plugin into Logi Options+ on this PC, the supported way.
# Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
#
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 [-Port 7981]
#
# 1. builds dist\Chromaforge.lplug4 (scripts\package.ps1),
# 2. writes the plugin's config.json (the port Chromaforge listens on for surfaces; Chromaforge
#    rewrites it, and the trust token, by itself when it starts),
# 3. hands the package to Logi's own installer (LogiPluginServiceTool install-plugin, what
#    double-clicking a .lplug4 does): Logi Options+ shows "Ready to install plugins"; click
#    Continue there. Logi installs the plugin and its default profiles and reloads its plugin
#    service by itself.
#
# Only Chromaforge's own folders change:
#   %LOCALAPPDATA%\Logi\LogiPluginService\Plugins\Chromaforge      the plugin (Logi's installer)
#   %LOCALAPPDATA%\Logi\LogiPluginService\PluginData\Chromaforge   config.json, the pairing token, its cache
# Other plugins (TactileBridge's Capture One plugin among them), their profiles and the rest of
# the Options+ settings are never touched. Chromaforge's Settings, Controls has the same Install
# button (steps 2 and 3, with the package that ships with Chromaforge).

param([int]$Port = 7981)
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$logi = Join-Path $env:LOCALAPPDATA 'Logi\LogiPluginService'
$tool = Join-Path ${env:ProgramFiles} 'Logi\LogiPluginService\LogiPluginServiceTool.exe'
if (-not (Test-Path $tool)) { throw "Logi Options+ is not installed ($tool is missing)." }

$pkg = Join-Path $repo 'dist\Chromaforge.lplug4'
& (Join-Path $PSScriptRoot 'package.ps1') -Out $pkg

$dataDir = Join-Path $logi 'PluginData\Chromaforge'
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
Set-Content -Path (Join-Path $dataDir 'config.json') -Value ("{`"port`": $Port}") -Encoding ascii
Write-Host "The plugin connects to Chromaforge on 127.0.0.1:$Port"

Write-Host 'Handing the package to Logi Options+ (click Continue in its "Ready to install plugins" window)...'
$p = Start-Process -FilePath $tool -ArgumentList 'install-plugin', "`"$pkg`"" -PassThru -Wait
if ($p.ExitCode -ne 0) { throw "Logi's installer failed (exit $($p.ExitCode))." }
Write-Host ''
Write-Host 'Nothing else to do: Chromaforge turns control surfaces on and trusts the plugin by itself the next time it starts.'
Write-Host '(It writes a token into the plugin data folder above; only a client it does not know asks to pair.)'
Write-Host 'To remove it again: scripts\uninstall.ps1'
