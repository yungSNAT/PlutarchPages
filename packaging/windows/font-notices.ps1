# Validate one non-comment manifest row via the compiled Rust packaging helper (`win_checks.rs`).
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
  $hash = ([BitConverter]::ToString($hashBytes) -replace '-', '').Substring(0, 16)
  $ext = if ([IO.Path]::DirectorySeparatorChar -eq '\') { '.exe' } else { '' }
  $exe = Join-Path ([IO.Path]::GetTempPath()) "pdfcraft-win-checks-$hash$ext"
  if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) "pdfcraft-win-checks-$hash.$PID.tmp$ext"
    & rustc --edition=2024 -D warnings $src -o $tmp
    if ($LASTEXITCODE -ne 0) {
      Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
      throw "Failed to compile win_checks.rs (exit $LASTEXITCODE)"
    }
    try {
      Move-Item -LiteralPath $tmp -Destination $exe -Force
    } catch {
      if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw }
      Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
  }
  & $exe @Arguments
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
