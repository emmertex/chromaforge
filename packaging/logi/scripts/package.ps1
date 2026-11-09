# Build the LightCraft plugin package for Logi Options+ (dist\LightCraft.lplug4).
# Copyright (c) 2026 RAD Films - Roman Alurkoff. MIT License.
#
#   powershell -ExecutionPolicy Bypass -File scripts\package.ps1
#
# A .lplug4 is a ZIP of the plugin folder (the DLL, metadata\ and profiles\). Logi's supported
# install path takes it: double-click it, or
#   "%ProgramFiles%\Logi\LogiPluginService\LogiPluginServiceTool.exe" install-plugin dist\LightCraft.lplug4
# which is also what LightCraft's Settings, Controls, "Install the Logi plugin" runs. LightCraft
# looks for the package next to its executable (LightCraft.lplug4) or at LIGHTCRAFT_LOGI_PLUGIN.

param([string]$Out = '')
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if ($Out -eq '') { $Out = Join-Path $repo 'dist\LightCraft.lplug4' }

Write-Host 'Generating the default profiles...'
dotnet run --project (Join-Path $repo 'tools\GenerateProfiles') -c Release -- (Join-Path $repo 'src\LightCraftPlugin\profiles') | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Profile generation failed.' }
Write-Host 'Building the plugin...'
dotnet build (Join-Path $repo 'src\LightCraftPlugin\LightCraftPlugin.csproj') -c Release -v q
if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
$bin = Join-Path $repo 'src\LightCraftPlugin\bin\Release\net8.0'

$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("lightcraft-lplug4-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    foreach ($item in 'LightCraftPlugin.dll', 'LightCraftPlugin.deps.json', 'metadata', 'profiles') {
        Copy-Item (Join-Path $bin $item) $stage -Recurse -Force
    }
    Copy-Item (Join-Path $repo 'LICENSE') $stage -Force
    New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
    if (Test-Path $Out) { Remove-Item $Out -Force }
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    # entry names with forward slashes (Windows PowerShell's CreateFromDirectory writes backslashes)
    $zip = [System.IO.Compression.ZipFile]::Open($Out, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        $root = (Resolve-Path $stage).Path.TrimEnd('\') + '\'
        foreach ($f in Get-ChildItem $stage -Recurse -File) {
            $name = $f.FullName.Substring($root.Length).Replace('\', '/')
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f.FullName, $name, [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
        }
    } finally {
        $zip.Dispose()
    }
} finally {
    Remove-Item $stage -Recurse -Force
}
$size = (Get-Item $Out).Length
Write-Host "Package: $Out ($([math]::Round($size / 1KB)) KB)"
