$script:CraftWinChecksDir = $null
$script:CraftWinChecksExe = $null
$script:CraftWinChecksSrcHash = $null
$script:CraftWinChecksExeHash = $null

function New-CraftPrivateCacheDir {
  if ([IO.Path]::DirectorySeparatorChar -eq '/') {
    $dir = (& mktemp -d "${env:TMPDIR:-/tmp}/pdfcraft-wc-XXXXXX").Trim()
    if ($LASTEXITCODE -ne 0 -or -not $dir) {
      $global:LASTEXITCODE = 0
      throw 'Failed to create private temporary directory for win_checks'
    }
    return $dir
  }
  $dir = Join-Path ([IO.Path]::GetTempPath()) ("pdfcraft-wc-$PID-" + [guid]::NewGuid())
  [void](New-Item -ItemType Directory -Path $dir)
  return $dir
}

# Validate and execute the compiled Rust packaging helper (`win_checks.rs`).
function Invoke-CraftWinChecks([string[]] $Arguments) {
  $src = Join-Path $PSScriptRoot 'win_checks.rs'
  $sources = @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter 'win_checks*.rs' | Sort-Object Name)
  $hashInput = ($sources | ForEach-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }) -join ''
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $hashBytes = $sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($hashInput))
  } finally {
    $sha.Dispose()
  }
  $srcHash = ([BitConverter]::ToString($hashBytes) -replace '-', '').Substring(0, 16)
  $ext = if ([IO.Path]::DirectorySeparatorChar -eq '\') { '.exe' } else { '' }
  if (-not $script:CraftWinChecksExe -or $script:CraftWinChecksSrcHash -ne $srcHash -or -not (Test-Path -LiteralPath $script:CraftWinChecksExe -PathType Leaf)) {
    if (-not $script:CraftWinChecksDir) { $script:CraftWinChecksDir = New-CraftPrivateCacheDir }
    $exe = Join-Path $script:CraftWinChecksDir "win_checks-$srcHash$ext"
    & rustc --edition=2024 -D warnings $src -o $exe
    if ($LASTEXITCODE -ne 0) {
      $code = $LASTEXITCODE
      $global:LASTEXITCODE = 0
      Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
      throw "Failed to compile win_checks.rs (exit $code)"
    }
    $script:CraftWinChecksExe = $exe
    $script:CraftWinChecksSrcHash = $srcHash
    $script:CraftWinChecksExeHash = (Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash
  } else {
    $actualHash = (Get-FileHash -LiteralPath $script:CraftWinChecksExe -Algorithm SHA256).Hash
    if ($actualHash -ne $script:CraftWinChecksExeHash) {
      throw 'Compiled win_checks binary integrity check failed'
    }
  }
  & $script:CraftWinChecksExe @Arguments
  $code = $LASTEXITCODE
  if ($code -ne 0) {
    $global:LASTEXITCODE = 0
    throw "win_checks $($Arguments[0]) failed with exit code $code"
  }
}

# Validate every declared native font notice before either Windows package is created.
# Checking all declared faces is deliberately conservative: size-capped faces may not be embedded.
function Get-CraftFontNotices([string] $FontRoot, [bool] $RequireChinese = $false) {
  $cmdArgs = @('font-notices', $FontRoot)
  if ($RequireChinese) { $cmdArgs += '--require-chinese' }
  foreach ($line in @(Invoke-CraftWinChecks $cmdArgs)) {
    if ($line) { Get-Item -LiteralPath $line }
  }
}

# MSI FileName may be short|long. Compare the long names with the staged inventory.
function Assert-CraftFontNoticeNames([string[]] $Expected = @(), [string[]] $Actual = @()) {
  $exp = @(if ($null -eq $Expected) { @() } else { $Expected })
  $act = @(if ($null -eq $Actual) { @() } else { $Actual })
  $cmdArgs = @('assert-notice-names', '--expected-count', "$($exp.Count)") + $exp + $act
  Invoke-CraftWinChecks $cmdArgs
}
