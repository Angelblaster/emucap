# Manual launcher for the native Dolphin adapter on Windows.
#
#   launch-native.ps1 <ISO> <EMUCAP_PORT> [NAME] [-Dolphin <forked exe>] [-User <dir>]
#
# Prefer the MCP launch tool. This fallback starts a compatible fork directly and passes the current
# listener identity through environment variables. It does not start a GDB relay.
param(
  [Parameter(Mandatory=$true)][string]$Iso,
  [Parameter(Mandatory=$true)][ValidateRange(1, 65535)][int]$EmucapPort,
  [string]$Name = "dolphin",
  [ValidateSet("gamecube", "wii")][string]$System = "gamecube",
  [string]$Dolphin = "C:\dolphin-build\dolphin-src\Binary\x64\Dolphin.exe",
  [string]$User = "$env:LOCALAPPDATA\emucap\dolphin\user"
)
$ErrorActionPreference = "Stop"

New-Item -ItemType Directory -Force -Path "$User\Config" | Out-Null
@"
[Core]
[Interface]
ConfirmStop = False
UsePanicHandlers = False
[DSP]
Backend = No Audio Output
Volume = 0
[Analytics]
Enabled = False
PermissionAsked = True
"@ | Out-File -Encoding ascii "$User\Config\Dolphin.ini"

# Resolve the same per-port compatibility token used by the Control MCP.
$runtimeBase = if ($env:EMUCAP_EMU_HOME) {
  $env:EMUCAP_EMU_HOME
} elseif ($env:LOCALAPPDATA) {
  Join-Path $env:LOCALAPPDATA "emucap"
} else {
  Join-Path ([System.IO.Path]::GetTempPath()) "emucap"
}
$tokenFile = if ($env:EMUCAP_SESSION_TOKEN_FILE) {
  $env:EMUCAP_SESSION_TOKEN_FILE
} else {
  Join-Path $runtimeBase "sessions\compatibility\session-token-$EmucapPort"
}
$env:EMUCAP_PORT = "$EmucapPort"
if (-not $env:EMUCAP_SESSION_TOKEN -and (Test-Path -LiteralPath $tokenFile -PathType Leaf)) {
  $env:EMUCAP_SESSION_TOKEN = (Get-Content -LiteralPath $tokenFile -TotalCount 1).Trim()
}
$env:EMUCAP_NAME = $Name
$env:EMUCAP_CONTENT = $Iso
$env:EMUCAP_SYSTEM = $System

$p = Start-Process -FilePath $Dolphin `
  -ArgumentList @("--user", "`"$User`"", "--exec", "`"$Iso`"", "--batch") -PassThru
$p.Id | Out-File -Encoding ascii "$User\dolphin.pid"
Write-Output "[launch-native] Dolphin fork pid=$($p.Id), Control MCP port=$EmucapPort"
