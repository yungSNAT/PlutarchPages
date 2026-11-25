param(
  [ValidateSet('x64', 'x86', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild,
  [switch] $BuiltWithoutCraftFonts
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Dot-source Resolve-CraftPrivateCacheDir for isolated helper compilation.
. (Join-Path $PSScriptRoot 'font-notices.ps1')

function Resolve-CraftHelperBinary([string] $CacheDir, [string] $Entry, [string] $Filter, [string] $Stem) {
  $src = Join-Path $PSScriptRoot $Entry
  $sources = @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter $Filter | Sort-Object Name)
  $hashInput = ($sources | ForEach-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }) -join ''
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $hashBytes = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($hashInput))
  } finally {
    $sha.Dispose()
  }
  $srcHash = ([BitConverter]::ToString($hashBytes) -replace '-', '').Substring(0, 16)
  $ext = if ([IO.Path]::DirectorySeparatorChar -eq '\') { '.exe' } else { '' }
  $exe = Join-Path $CacheDir "$Stem-$srcHash$ext"
  if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    & rustc --edition=2024 -D warnings $src -o $exe
    $rc = $LASTEXITCODE
    if ($rc -ne 0) {
      $global:LASTEXITCODE = 0
      Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
      throw "Failed to compile $Entry (exit $rc)"
    }
  }
  $exeHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
  return @{ Path = $exe; Hash = $exeHash }
}

function Invoke-CraftPackage([string[]] $Arguments) {
  $cacheDir = Resolve-CraftPrivateCacheDir
  try {
    $pkg = Resolve-CraftHelperBinary -CacheDir $cacheDir -Entry 'package.rs' -Filter 'package*.rs' -Stem 'package'
    if ((Get-FileHash -LiteralPath $pkg.Path -Algorithm SHA256).Hash -ne $pkg.Hash) {
      throw 'Compiled package binary integrity check failed'
    }
    $hostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
    $fullArgs = @('--script-dir', $PSScriptRoot, '--shell', (Get-Process -Id $PID).Path, '--host-arch', $hostArch)
    if (-not [string]::IsNullOrWhiteSpace($env:CRAFT_FONTS_DIR)) {
      $wc = Resolve-CraftHelperBinary -CacheDir $cacheDir -Entry 'win_checks.rs' -Filter 'win_checks*.rs' -Stem 'win_checks'
      if ((Get-FileHash -LiteralPath $wc.Path -Algorithm SHA256).Hash -ne $wc.Hash) {
        throw 'Compiled win_checks binary integrity check failed'
      }
      $fullArgs += @('--win-checks', $wc.Path)
    }
    $fullArgs += $Arguments
    & $pkg.Path @fullArgs
    $code = $LASTEXITCODE
    if ($code -ne 0) {
      $global:LASTEXITCODE = 0
      throw "package.rs failed with exit code $code"
    }
  } finally {
    Remove-Item -LiteralPath $cacheDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$cmdArgs = @('--arch', $Arch)
if ($SkipBuild) { $cmdArgs += '--skip-build' }
if ($BuiltWithoutCraftFonts) { $cmdArgs += '--built-without-craft-fonts' }
Invoke-CraftPackage -Arguments $cmdArgs
