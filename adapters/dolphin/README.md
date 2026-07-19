# Dolphin (GameCube / Wii) 어댑터

emucap 을 GameCube/Wii(PowerPC Gekko/Broadway)로 확장한다. **두 가지 방식**을 제공한다.

| | 네이티브 포크 (권장) | GDB-스텁 브리지 |
|---|---|---|
| 빌드 | Dolphin 소스 빌드 필요 (`build.ps1`) | 불필요 (mainline Dolphin) |
| CPU | JIT 가능 (빠름) | CachedInterpreter 강제 (GDB 한계) |
| 메모리·레지스터·BP·step·pause/resume | ✅ | ✅ |
| **savestate · screenshot · 입력(set_input)** | ✅ | ❌ (GDB RSP에 없음) |
| 레지스터 | pc·GPR·lr·ctr·xer·msr·cr | GPR 위주 |

둘 다 emucap-mcp 리스너에 TCP 클라이언트로 접속해 NDJSON `{"v":1,"id","method","params"}`
요청에 `{"id","ok","result|error"}` 로 답한다(다른 어댑터와 동일). `system:"gc"`.

---

## 1) 네이티브 포크 (풀 제어)

emucap 서버(`EmuCap.cpp`/`.h`)를 Dolphin 소스에 임베드한다. 별도 스레드 소켓 서버가
Dolphin 내부 API(`CPUThreadGuard`·`Memory::CopyFromEmu`·`PowerPC` 레지스터·`BreakPoints`·
`State::Save/Load`·`FrameDumper`·`GCPad` 오버라이드)로 각 메서드를 처리한다.

**빌드:**
```powershell
powershell -ExecutionPolicy Bypass -File build.ps1
# Dolphin 소스를 핀 커밋으로 클론 → EmuCap.cpp/h 배치 → patches/ 적용 → MSBuild Release x64
# 산출물: <src>\Binary\x64\Dolphin.exe   (전제: VS2022 + MSVC + Windows SDK, Git)
```

**실행:**
```powershell
# emucap-mcp status 로 listening_port 확보 후:
powershell -ExecutionPolicy Bypass -File launch-native.ps1 "<ISO>" <EMUCAP_PORT>
```
`launch-native.ps1` 은 세션 토큰/포트/콘텐츠를 환경변수(EMUCAP_PORT / EMUCAP_SESSION_TOKEN /
EMUCAP_NAME / EMUCAP_CONTENT)로 넘겨 포크 Dolphin 을 `--batch --exec` 로 띄운다. 포크의
`EmuCap::Start`(EMUCAP_PORT 있을 때만) 가 리스너로 접속한다. 브리지 프로세스·GDB 불필요.

**패치 파일**(`patches/`, Dolphin 415ec4de 기준):
- `DolphinLib.props.patch` — EmuCap.cpp/h 를 프로젝트에 등록
- `Core.cpp.patch` — EmuThread(run loop 직전)에 `EmuCap::Start`/Stop 훅
- `GCPad.cpp.patch` — `GetStatus` 폴 지점에 `EmuCap::ApplyInputOverride` (set_input)

**제공 메서드:** read/write_memory · get_state · pause · resume · step · set/clear/list_breakpoint ·
poll_events · save_state · load_state · screenshot · set_input.
- `memory_type: main`(MEM1), **address 는 절대 EA**(0x8000_0000 기반, 디스어셈블 그대로).
- `set_input`: 버튼명 A/B/X/Y/START/Z/L/R/UP/DOWN/LEFT/RIGHT + stickX/Y·substickX/Y·triggerL/R.
  빈 배열이면 중립(뗌). GCPad::GetStatus 에서 덮으므로 running·frozen 무관하게 결정론적.
- `screenshot`: 다음 present 에 임시 PNG 저장→읽어 `png_base64` 반환(코어 실행 중일 때).

## 2) GDB-스텁 브리지 (빌드 없이)

mainline Dolphin 내장 GDB 스텁을 `emucap-gdb-bridge.py` 로 중계한다. 포크/빌드 불필요.

**실행:**
```powershell
powershell -ExecutionPolicy Bypass -File launch.ps1 "<ISO>" <EMUCAP_PORT>
```
Dolphin.ini 에 `GDBPort=2159`, `CPUCore=5`(CachedInterpreter; JIT 는 GDB 미지원)를 심고
`--batch --exec` 로 띄운 뒤 GDB 포트가 열리면 브리지를 붙인다. GDB 스텁은 **단발**(첫 클라이언트
분리 시 리스너를 닫음)이라 브리지가 유일한 지속 클라이언트여야 한다 — 별도 GDB 접속 금지.

## 공통 주의

- **ISO 경로는 반드시 인용**한다. 공백이 있으면 잘려 "파일 없음" 경고로 부팅 실패한다.
- 이미 떠 있는 Dolphin 인스턴스는 건드리지 않는다(다중 세션 안전). 이 스크립트가 띄운 pid 만
  `<user>\dolphin.pid` 로 추적한다.

## 이 어댑터가 만들어진 이유

GameCube Biohazard 4 일본판 MDT 텍스트가 병렬 2-스트림(디인터리버 `0x800B01B8`)으로 저장돼
정적 디코드가 뒤섞인다. 런타임에 그 출력 버퍼를 덤프해 base 텍스트를 확정하려면 GameCube 라이브
제어가 필요했고, emucap 이 GC 를 지원하지 않아 이 어댑터를 추가했다.
