param([string] $CraftFontsRoot)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'font-notices.ps1')

$cmdArgs = @('test-font-notices')
if ($CraftFontsRoot) { $cmdArgs += $CraftFontsRoot }
Invoke-CraftWinChecks $cmdArgs

# Smoke-test the PowerShell shim layer (FileInfo return shape + rejection propagation).
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('pdfcraft-ps-font-notices-' + [guid]::NewGuid())
try {
  $coreDir = Join-Path $fixture 'fonts\core'
  New-Item -ItemType Directory -Path $coreDir -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $coreDir 'OFL.txt') -Value 'Synthetic notice fixture'
  Set-Content -LiteralPath (Join-Path $fixture 'fonts\manifest.txt') 'Core | Regular | fonts/core/face.ttf | Latn | OFL-1.1 | fonts/core/OFL.txt'

  $notices = @(Get-CraftFontNotices $fixture $false)
  if ($notices.Count -ne 1 -or $notices[0] -isnot [IO.FileInfo] -or $notices[0].Directory.Name -ne 'core') {
    throw 'Expected Get-CraftFontNotices to return FileInfo objects in family directories'
  }
  Assert-CraftFontNoticeNames @('OFL-core.txt') @('SHORT.TXT|OFL-core.txt')
  $rejected = $false
  try { Assert-CraftFontNoticeNames @('OFL-missing.txt') @('OFL-core.txt') | Out-Null } catch { $rejected = $true }
  if (-not $rejected) { throw 'Expected Assert-CraftFontNoticeNames to reject missing notice' }
} finally {
  Remove-Item -LiteralPath $fixture -Recurse -Force -ErrorAction SilentlyContinue
}
