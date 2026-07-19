# emucap ↔ Dolphin(GameCube/Wii) 네이티브 포크 런치 헬퍼.
#
#   launch-native.ps1 <ISO> <EMUCAP_PORT> [NAME] [-Dolphin <forked exe>] [-User <dir>]
#
# GDB 브리지와 달리 포크된 Dolphin에 emucap 서버가 임베드돼 있다(Core/EmuCap.cpp). 이 스크립트는
# 세션 토큰/포트/콘텐츠를 환경변수로 넘겨 포크 Dolphin을 띄우기만 하면 된다 — 별도 브리지 프로세스도,
# GDB 스텁도, CachedInterpreter 강제도 없다(JIT 사용 가능 = 빠름). savestate/screenshot 포함 풀 제어.
param(
  [Parameter(Mandatory=$true)][string]$Iso,
  [Parameter(Mandatory=$true)][int]$EmucapPort,
  [string]$Name = "dolphin",
  [string]$Dolphin = "C:\dolphin-build\dolphin-src\Binary\x64\Dolphin.exe",
  [string]$User = "$env:LOCALAPPDATA\emucap\dolphin\user"
)
$ErrorActionPreference = "Stop"

New-Item -ItemType Directory -Force -Path "$User\Config" | Out-Null
@"
[Interface]
ConfirmStop = False
UsePanicHandlers = False
[Analytics]
Enabled = False
PermissionAsked = True
"@ | Out-File -Encoding ascii "$User\Config\Dolphin.ini"

# 포크 Dolphin의 EmuCap::Start 가 읽는 환경변수. 세션 토큰은 identity_guard 통과용(포트별 파일).
$tokenFile = "C:\emutmp\emucap_session_token_$EmucapPort"
$env:EMUCAP_PORT = "$EmucapPort"
$env:EMUCAP_SESSION_TOKEN = if (Test-Path $tokenFile) { (Get-Content $tokenFile -Raw).Trim() } else { "" }
$env:EMUCAP_NAME = $Name
$env:EMUCAP_CONTENT = $Iso

# 경로는 반드시 인용(공백 시 잘려 부팅 실패).
$p = Start-Process -FilePath $Dolphin `
  -ArgumentList @("--user", "`"$User`"", "--exec", "`"$Iso`"", "--batch") -PassThru
$p.Id | Out-File -Encoding ascii "$User\dolphin.pid"
Write-Output "[launch-native] Dolphin(fork) pid=$($p.Id) → emucap 127.0.0.1:$EmucapPort. emucap status 로 확인."
