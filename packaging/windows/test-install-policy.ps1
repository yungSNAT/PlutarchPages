<#
.SYNOPSIS
  Build non-executable installer metadata fixtures; never install them.
#>
param(
  [Parameter(Mandatory = $true)] [string] $OutputDirectory,
  [ValidateSet('x64','x86','arm64')] [string[]] $Architectures = @('x64','x86','arm64')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'font-notices.ps1')

$cmdArgs = @(
  'test-install-policy',
  '--script-dir', $PSScriptRoot,
  '--output-dir', $OutputDirectory,
  '--shell', (Get-Process -Id $PID).Path
)
foreach ($arch in $Architectures) {
  $cmdArgs += @('--arch', $arch)
}
Invoke-CraftWinChecks $cmdArgs
