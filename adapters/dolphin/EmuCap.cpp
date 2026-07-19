// Copyright 2026 emucap
// SPDX-License-Identifier: GPL-2.0-or-later
//
// Dolphin(GameCube/Wii) 네이티브 emucap 어댑터. 별도 스레드에서 emucap-mcp 리스너로
// 접속해 NDJSON 요청을 Dolphin 내부 API로 번역한다. GDB-스텁 브리지와 달리 savestate/
// screenshot 까지 제공한다(입력/frame 은 후속 훅 필요 — 아래 TODO).

#include "Core/EmuCap.h"

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <deque>
#include <fstream>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <arpa/inet.h>
#include <sys/socket.h>
#include <unistd.h>
using SOCKET = int;
#define INVALID_SOCKET (-1)
#define closesocket close
#endif

#include <picojson.h>

#include "Common/Config/Config.h"
#include "Common/SocketContext.h"
#include "Core/Config/MainSettings.h"
#include "Core/Core.h"
#include "Core/HW/CPU.h"
#include "Core/HW/Memmap.h"
#include "Core/PowerPC/BreakPoints.h"
#include "Core/PowerPC/Gekko.h"
#include "Core/PowerPC/JitInterface.h"
#include "Core/PowerPC/PowerPC.h"
#include "Core/State.h"
#include "Core/System.h"
#include "InputCommon/GCPadStatus.h"
#include "VideoCommon/FrameDumper.h"

namespace EmuCap
{
namespace
{
std::thread s_thread;
std::atomic<bool> s_stop{false};
std::atomic<bool> s_started{false};

std::mutex s_ev_mutex;
std::deque<picojson::value> s_events;

std::map<int, u32> s_breakpoints;  // id -> address
int s_next_bp = 1;
bool s_bp_reported = false;  // 현재 halt에 대해 이미 breakpoint_hit 를 보고했는지

// set_input 오버라이드(패드별). engaged면 GCPad::GetStatus 결과를 이 값으로 덮는다.
std::mutex s_input_mutex;
struct InputOverride
{
  bool engaged = false;
  GCPadStatus status;  // 기본 생성자가 중립(스틱 중앙) 값으로 초기화
};
InputOverride s_input[4];

std::string Base64(const uint8_t* data, size_t n)
{
  static const char* T = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  std::string out;
  out.reserve((n + 2) / 3 * 4);
  size_t i = 0;
  for (; i + 3 <= n; i += 3)
  {
    uint32_t v = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2];
    out.push_back(T[(v >> 18) & 63]);
    out.push_back(T[(v >> 12) & 63]);
    out.push_back(T[(v >> 6) & 63]);
    out.push_back(T[v & 63]);
  }
  if (n - i == 1)
  {
    uint32_t v = data[i] << 16;
    out.push_back(T[(v >> 18) & 63]);
    out.push_back(T[(v >> 12) & 63]);
    out.push_back('=');
    out.push_back('=');
  }
  else if (n - i == 2)
  {
    uint32_t v = (data[i] << 16) | (data[i + 1] << 8);
    out.push_back(T[(v >> 18) & 63]);
    out.push_back(T[(v >> 12) & 63]);
    out.push_back(T[(v >> 6) & 63]);
    out.push_back('=');
  }
  return out;
}

u16 ButtonBit(const std::string& name)
{
  if (name == "A") return PAD_BUTTON_A;
  if (name == "B") return PAD_BUTTON_B;
  if (name == "X") return PAD_BUTTON_X;
  if (name == "Y") return PAD_BUTTON_Y;
  if (name == "START" || name == "Start") return PAD_BUTTON_START;
  if (name == "Z") return PAD_TRIGGER_Z;
  if (name == "L") return PAD_TRIGGER_L;
  if (name == "R") return PAD_TRIGGER_R;
  if (name == "UP" || name == "Up") return PAD_BUTTON_UP;
  if (name == "DOWN" || name == "Down") return PAD_BUTTON_DOWN;
  if (name == "LEFT" || name == "Left") return PAD_BUTTON_LEFT;
  if (name == "RIGHT" || name == "Right") return PAD_BUTTON_RIGHT;
  return 0;
}

std::string EnvOr(const char* key, const char* fallback)
{
  const char* v = std::getenv(key);
  return v ? std::string(v) : std::string(fallback);
}

std::string ToHex(const uint8_t* data, size_t n)
{
  static const char* k = "0123456789abcdef";
  std::string out;
  out.reserve(n * 2);
  for (size_t i = 0; i < n; ++i)
  {
    out.push_back(k[data[i] >> 4]);
    out.push_back(k[data[i] & 0xF]);
  }
  return out;
}

bool FromHex(const std::string& s, std::vector<uint8_t>& out)
{
  if (s.size() % 2)
    return false;
  auto nib = [](char c) -> int {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
  };
  out.clear();
  for (size_t i = 0; i < s.size(); i += 2)
  {
    int hi = nib(s[i]), lo = nib(s[i + 1]);
    if (hi < 0 || lo < 0)
      return false;
    out.push_back(static_cast<uint8_t>((hi << 4) | lo));
  }
  return true;
}

// params 에서 정수 얻기(숫자 또는 "0x.." 문자열 허용).
bool GetU64(const picojson::object& p, const char* key, uint64_t& out)
{
  auto it = p.find(key);
  if (it == p.end())
    return false;
  if (it->second.is<double>())
  {
    out = static_cast<uint64_t>(it->second.get<double>());
    return true;
  }
  if (it->second.is<std::string>())
  {
    const std::string& s = it->second.get<std::string>();
    out = std::strtoull(s.c_str(), nullptr, s.rfind("0x", 0) == 0 ? 16 : 0);
    return true;
  }
  return false;
}

void PushEvent(picojson::value ev)
{
  std::lock_guard<std::mutex> lk(s_ev_mutex);
  s_events.push_back(std::move(ev));
}

// ── 메서드 핸들러 ── (성공 시 result object 반환, 실패 시 GdbError 대신 throw std::string)

picojson::value MakeError(const std::string& kind, const std::string& msg)
{
  picojson::object e;
  e["kind"] = picojson::value(kind);
  e["message"] = picojson::value(msg);
  return picojson::value(e);
}

// CPU 스레드와 경합 없이 PowerPC/메모리에 접근하기 위한 가드. 코어가 안 떠 있으면 실패.
struct SafeAccess
{
  Core::System& system;
  Core::CPUThreadGuard guard;
  explicit SafeAccess(Core::System& sys) : system(sys), guard(sys) {}
};

picojson::object Hello(Core::System&, const picojson::object&)
{
  picojson::object r;
  r["protocol_version"] = picojson::value(1.0);
  r["name"] = picojson::value(EnvOr("EMUCAP_NAME", "dolphin"));
  r["system"] = picojson::value(std::string("gc"));
  r["adapter"] = picojson::value(std::string("dolphin-native"));
  picojson::array methods;
  for (const char* m :
       {"read_memory", "write_memory", "get_state", "status", "pause", "resume", "step",
        "set_breakpoint", "clear_breakpoint", "list_breakpoints", "poll_events", "save_state",
        "load_state", "screenshot", "set_input"})
  {
    methods.push_back(picojson::value(std::string(m)));
  }
  r["methods"] = picojson::value(methods);
  picojson::array mt;
  mt.push_back(picojson::value(std::string("main")));
  r["memory_types"] = picojson::value(mt);
  const std::string tok = EnvOr("EMUCAP_SESSION_TOKEN", "");
  if (!tok.empty())
    r["session_token"] = picojson::value(tok);
  const std::string content = EnvOr("EMUCAP_CONTENT", "");
  if (!content.empty())
    r["content"] = picojson::value(content);
  return r;
}

picojson::object Status(Core::System& system, const picojson::object&)
{
  picojson::object r;
  const Core::State st = Core::GetState(system);
  r["connected"] = picojson::value(true);
  r["state"] = picojson::value(std::string(st == Core::State::Paused ? "frozen" : "running"));
  r["adapter"] = picojson::value(std::string("dolphin-native"));
  // exec BP 진단 필드(경량 — CPUThreadGuard 없이 읽는다): 브레이크포인트가 왜 히트/미히트
  // 하는지 런타임 근거로 확인한다. dbg_effective 는 Config::IsDebuggingEnabled()(=
  // MAIN_ENABLE_DEBUGGING && !achievements-hardcore) 로, 코어가 BP 를 체크하려면 true 여야 한다.
  // cpu_core: 0=Interpreter, 1=JIT64, 4=JITARM64, 5=CachedInterpreter.
  r["dbg_config"] = picojson::value(Config::Get(Config::MAIN_ENABLE_DEBUGGING));
  r["dbg_effective"] = picojson::value(Config::IsDebuggingEnabled());
  r["cpu_core"] = picojson::value(static_cast<double>(static_cast<int>(Config::Get(Config::MAIN_CPU_CORE))));
  const auto& bps = system.GetPowerPC().GetBreakPoints();
  r["breaking_enabled"] = picojson::value(bps.IsBreakingEnabled());
  r["dolphin_bp_count"] = picojson::value(static_cast<double>(bps.GetBreakPoints().size()));
  return r;
}

picojson::object ReadMemory(Core::System& system, const picojson::object& p)
{
  uint64_t addr = 0, len = 0;
  if (!GetU64(p, "address", addr) || !GetU64(p, "length", len))
    throw std::string("address/length 필요");
  std::vector<uint8_t> buf(static_cast<size_t>(len));
  {
    SafeAccess sa(system);
    system.GetMemory().CopyFromEmu(buf.data(), static_cast<u32>(addr), buf.size());
  }
  picojson::object r;
  r["hex"] = picojson::value(ToHex(buf.data(), buf.size()));
  return r;
}

picojson::object WriteMemory(Core::System& system, const picojson::object& p)
{
  uint64_t addr = 0;
  auto it = p.find("hex");
  if (!GetU64(p, "address", addr) || it == p.end() || !it->second.is<std::string>())
    throw std::string("address/hex 필요");
  std::vector<uint8_t> data;
  if (!FromHex(it->second.get<std::string>(), data))
    throw std::string("잘못된 hex");
  {
    SafeAccess sa(system);
    system.GetMemory().CopyToEmu(static_cast<u32>(addr), data.data(), data.size());
  }
  picojson::object r;
  r["written"] = picojson::value(static_cast<double>(data.size()));
  return r;
}

picojson::object GetState(Core::System& system, const picojson::object&)
{
  picojson::object state;
  {
    SafeAccess sa(system);
    const auto& ppc = system.GetPPCState();
    state["cpu.pc"] = picojson::value(static_cast<double>(ppc.pc));
    for (int i = 0; i < 32; ++i)
      state["cpu.r" + std::to_string(i)] = picojson::value(static_cast<double>(ppc.gpr[i]));
    state["cpu.lr"] = picojson::value(static_cast<double>(ppc.spr[SPR_LR]));
    state["cpu.ctr"] = picojson::value(static_cast<double>(ppc.spr[SPR_CTR]));
    state["cpu.xer"] = picojson::value(static_cast<double>(ppc.spr[SPR_XER]));
    state["cpu.msr"] = picojson::value(static_cast<double>(ppc.msr.Hex));
    state["cpu.cr"] = picojson::value(static_cast<double>(ppc.cr.Get()));
  }
  picojson::object r;
  r["state"] = picojson::value(state);
  return r;
}

picojson::object Pause(Core::System& system, const picojson::object&)
{
  Core::SetState(system, Core::State::Paused);
  picojson::object r;
  r["state"] = picojson::value(std::string("frozen"));
  return r;
}

picojson::object Resume(Core::System& system, const picojson::object&)
{
  Core::SetState(system, Core::State::Running);
  picojson::object r;
  r["state"] = picojson::value(std::string("running"));
  return r;
}

picojson::object Step(Core::System& system, const picojson::object&)
{
  auto& cpu = system.GetCPU();
  cpu.SetStepping(true);
  cpu.StepOpcode();
  picojson::object r;
  r["status"] = picojson::value(std::string("completed"));
  return r;
}

picojson::object SetBreakpoint(Core::System& system, const picojson::object& p)
{
  auto kind_it = p.find("kind");
  if (kind_it != p.end() && kind_it->second.is<std::string>() &&
      kind_it->second.get<std::string>() != "exec")
  {
    throw std::string("exec BP만 지원(read/write watch는 미구현)");
  }
  uint64_t addr = 0;
  if (!GetU64(p, "start", addr))
    throw std::string("start 필요");
  {
    SafeAccess sa(system);
    system.GetPowerPC().GetBreakPoints().Add(static_cast<u32>(addr));
    // 캐시된 블록(JIT·CachedInterpreter)에는 BP 체크가 컴파일돼 있지 않으므로, 전체 캐시를
    // 비워 재컴파일 시 체크가 삽입되게 한다(4바이트 InvalidateICache 만으로는 불충분).
    system.GetJitInterface().ClearCache(sa.guard);
  }
  const int id = s_next_bp++;
  s_breakpoints[id] = static_cast<u32>(addr);
  picojson::object r;
  r["id"] = picojson::value(static_cast<double>(id));
  return r;
}

picojson::object ClearBreakpoint(Core::System& system, const picojson::object& p)
{
  uint64_t id = 0;
  if (!GetU64(p, "id", id))
    throw std::string("id 필요");
  auto it = s_breakpoints.find(static_cast<int>(id));
  if (it == s_breakpoints.end())
    throw std::string("그런 breakpoint 없음");
  {
    SafeAccess sa(system);
    system.GetPowerPC().GetBreakPoints().Remove(it->second);
    system.GetJitInterface().ClearCache(sa.guard);
  }
  s_breakpoints.erase(it);
  picojson::object r;
  r["cleared"] = picojson::value(static_cast<double>(id));
  return r;
}

picojson::object ListBreakpoints(Core::System&, const picojson::object&)
{
  picojson::array bps;
  for (const auto& [id, addr] : s_breakpoints)
  {
    picojson::object b;
    b["id"] = picojson::value(static_cast<double>(id));
    b["kind"] = picojson::value(std::string("exec"));
    b["start"] = picojson::value(static_cast<double>(addr));
    b["end"] = picojson::value(static_cast<double>(addr));
    bps.push_back(picojson::value(b));
  }
  picojson::object r;
  r["breakpoints"] = picojson::value(bps);
  return r;
}

picojson::object PollEvents(Core::System& system, const picojson::object&)
{
  // BP-히트 감지. 주의: read_memory/get_state가 쓰는 CPUThreadGuard(PauseAndLock)도 코어를
  // 잠깐 State::Stepping 으로 만든다. 그래서 단순 stepping 에지는 오탐/누락이 난다.
  // 코어가 halt(stepping) 이고 PC가 우리가 등록한 BP 주소일 때만 진짜 히트로 본다
  // (CheckBreakpoint가 halt 직전 ppc_state.pc = current_pc 로 맞추므로 PC == BP 주소).
  // 에지(!s_prev_stepping) 방식은 pause/step/CPUThreadGuard 가 IsStepping 을 true 로 만들면
  // s_prev 가 고착돼 진짜 히트의 상승 에지를 놓친다. 대신 "코어가 halt 이고 PC 가 등록된 BP
  // 주소면 이 halt 당 한 번만 보고"한다(에지 무의존). 코어가 다시 running 이 되면 리셋.
  const bool stepping = system.GetCPU().IsStepping();
  if (stepping)
  {
    u32 pc = 0;
    {
      SafeAccess sa(system);
      pc = system.GetPPCState().pc;
    }
    bool at_bp = false;
    for (const auto& [bp_id, bp_addr] : s_breakpoints)
    {
      if (bp_addr == pc)
      {
        at_bp = true;
        break;
      }
    }
    if (at_bp && !s_bp_reported)
    {
      picojson::object ev;
      ev["type"] = picojson::value(std::string("breakpoint_hit"));
      ev["pc"] = picojson::value(static_cast<double>(pc));
      PushEvent(picojson::value(ev));
      s_bp_reported = true;
    }
  }
  else
  {
    s_bp_reported = false;
  }

  picojson::array out;
  {
    std::lock_guard<std::mutex> lk(s_ev_mutex);
    while (!s_events.empty())
    {
      out.push_back(std::move(s_events.front()));
      s_events.pop_front();
    }
  }
  picojson::object r;
  r["events"] = picojson::value(out);
  r["dropped"] = picojson::value(0.0);
  return r;
}

picojson::object SaveState(Core::System& system, const picojson::object& p)
{
  uint64_t slot = 0;
  auto fn = p.find("filename");
  if (fn != p.end() && fn->second.is<std::string>())
    State::SaveAs(system, fn->second.get<std::string>());
  else if (GetU64(p, "slot", slot))
    State::Save(system, static_cast<int>(slot));
  else
    State::Save(system, 1);
  picojson::object r;
  r["status"] = picojson::value(std::string("saved"));
  return r;
}

picojson::object LoadState(Core::System& system, const picojson::object& p)
{
  uint64_t slot = 0;
  auto fn = p.find("filename");
  if (fn != p.end() && fn->second.is<std::string>())
    State::LoadAs(system, fn->second.get<std::string>());
  else if (GetU64(p, "slot", slot))
    State::Load(system, static_cast<int>(slot));
  else
    State::Load(system, 1);
  picojson::object r;
  r["status"] = picojson::value(std::string("loaded"));
  return r;
}

picojson::object SetInput(Core::System&, const picojson::object& p)
{
  int pad = 0;
  uint64_t v = 0;
  if (GetU64(p, "pad", v))
    pad = static_cast<int>(v);
  if (pad < 0 || pad > 3)
    throw std::string("pad 는 0..3");

  std::lock_guard<std::mutex> lk(s_input_mutex);
  auto engaged = p.find("engaged");
  if (engaged != p.end() && engaged->second.is<bool>() && !engaged->second.get<bool>())
  {
    s_input[pad].engaged = false;
    picojson::object r;
    r["engaged"] = picojson::value(false);
    return r;
  }

  GCPadStatus st;  // 중립 기본값
  if (auto it = p.find("buttons"); it != p.end() && it->second.is<picojson::array>())
  {
    u16 bits = 0;
    for (const auto& b : it->second.get<picojson::array>())
      if (b.is<std::string>())
        bits |= ButtonBit(b.get<std::string>());
    st.button = bits;
  }
  if (GetU64(p, "stickX", v)) st.stickX = static_cast<u8>(v);
  if (GetU64(p, "stickY", v)) st.stickY = static_cast<u8>(v);
  if (GetU64(p, "substickX", v)) st.substickX = static_cast<u8>(v);
  if (GetU64(p, "substickY", v)) st.substickY = static_cast<u8>(v);
  if (GetU64(p, "triggerL", v)) st.triggerLeft = static_cast<u8>(v);
  if (GetU64(p, "triggerR", v)) st.triggerRight = static_cast<u8>(v);
  s_input[pad].status = st;
  s_input[pad].engaged = true;

  picojson::object r;
  r["engaged"] = picojson::value(true);
  r["pad"] = picojson::value(static_cast<double>(pad));
  return r;
}

picojson::object Screenshot(Core::System&, const picojson::object&)
{
  // emucap 서버는 응답에 png_base64 를 요구한다. Dolphin 의 SaveScreenShot 은 다음 present
  // 에 지정 파일로 PNG 를 쓰므로(코어 실행 중일 때), 임시 파일로 저장→읽어 base64 로 돌려준다.
  const char* tmpdir = std::getenv("TEMP");
  std::string path = (tmpdir ? std::string(tmpdir) : std::string(".")) + "/emucap_shot.png";
  std::remove(path.c_str());
  if (!g_frame_dumper)
    throw std::string("frame dumper 미초기화(비디오 백엔드 없음)");
  // Core::SaveScreenShot 은 name 을 "<폴더>/<name>.png" 로 조합해버리므로, 전체 경로를 그대로
  // 쓰도록 FrameDumper 를 직접 호출한다. 다음 present 에 이 경로로 PNG 가 써진다.
  g_frame_dumper->SaveScreenshot(path);

  std::vector<uint8_t> bytes;
  for (int i = 0; i < 60; ++i)  // 최대 ~1.5s 대기(실행 중이면 한두 프레임 내 도착)
  {
    std::this_thread::sleep_for(std::chrono::milliseconds(25));
    std::ifstream f(path, std::ios::binary | std::ios::ate);
    if (!f)
      continue;
    const std::streamsize sz = f.tellg();
    if (sz <= 0)
      continue;
    f.seekg(0);
    bytes.resize(static_cast<size_t>(sz));
    f.read(reinterpret_cast<char*>(bytes.data()), sz);
    break;
  }
  std::remove(path.c_str());
  if (bytes.empty())
    throw std::string("screenshot 캡처 실패(코어가 present 안 함 — 일시정지 중이면 먼저 resume)");

  picojson::object r;
  r["png_base64"] = picojson::value(Base64(bytes.data(), bytes.size()));
  r["bytes"] = picojson::value(static_cast<double>(bytes.size()));
  return r;
}

using Handler = picojson::object (*)(Core::System&, const picojson::object&);

Handler Lookup(const std::string& m)
{
  if (m == "hello") return Hello;
  if (m == "status") return Status;
  if (m == "read_memory") return ReadMemory;
  if (m == "write_memory") return WriteMemory;
  if (m == "get_state") return GetState;
  if (m == "pause") return Pause;
  if (m == "resume") return Resume;
  if (m == "step") return Step;
  if (m == "set_breakpoint") return SetBreakpoint;
  if (m == "clear_breakpoint") return ClearBreakpoint;
  if (m == "list_breakpoints") return ListBreakpoints;
  if (m == "poll_events") return PollEvents;
  if (m == "save_state") return SaveState;
  if (m == "load_state") return LoadState;
  if (m == "screenshot") return Screenshot;
  if (m == "set_input") return SetInput;
  return nullptr;
}

void SendLine(SOCKET sock, const std::string& line)
{
  std::string out = line + "\n";
  send(sock, out.data(), static_cast<int>(out.size()), 0);
}

void ServeSession(Core::System& system, SOCKET sock)
{
  std::string buf;
  char chunk[4096];
  while (!s_stop.load())
  {
    size_t nl;
    while ((nl = buf.find('\n')) != std::string::npos)
    {
      std::string line = buf.substr(0, nl);
      buf.erase(0, nl + 1);
      if (line.empty())
        continue;

      picojson::value req;
      const std::string perr = picojson::parse(req, line);
      if (!perr.empty() || !req.is<picojson::object>())
        continue;
      const picojson::object& env = req.get<picojson::object>();
      double id = 0;
      if (auto it = env.find("id"); it != env.end() && it->second.is<double>())
        id = it->second.get<double>();
      const std::string method =
          env.count("method") ? env.at("method").to_str() : std::string();
      picojson::object params;
      if (auto it = env.find("params"); it != env.end() && it->second.is<picojson::object>())
        params = it->second.get<picojson::object>();

      picojson::object resp;
      resp["id"] = picojson::value(id);
      Handler h = Lookup(method);
      if (!h)
      {
        resp["ok"] = picojson::value(false);
        resp["error"] = MakeError("unknown_method", method);
      }
      else
      {
        try
        {
          resp["ok"] = picojson::value(true);
          resp["result"] = picojson::value(h(system, params));
        }
        catch (const std::string& e)
        {
          resp["ok"] = picojson::value(false);
          resp["error"] = MakeError("emulator_error", e);
        }
        catch (const std::exception& e)
        {
          resp["ok"] = picojson::value(false);
          resp["error"] = MakeError("adapter_error", e.what());
        }
      }
      SendLine(sock, picojson::value(resp).serialize());
    }

    const int got = recv(sock, chunk, sizeof(chunk), 0);
    if (got <= 0)
      return;
    buf.append(chunk, static_cast<size_t>(got));
  }
}

void ThreadMain(Core::System& system, unsigned short port)
{
  Common::SocketContext socket_context;  // Windows WSAStartup RAII
  while (!s_stop.load())
  {
    SOCKET sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (sock == INVALID_SOCKET)
    {
      std::this_thread::sleep_for(std::chrono::milliseconds(200));
      continue;
    }
    sockaddr_in addr{};
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port);
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    if (connect(sock, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) == 0)
      ServeSession(system, sock);
    closesocket(sock);
    if (!s_stop.load())
      std::this_thread::sleep_for(std::chrono::milliseconds(200));
  }
}
}  // namespace

void ApplyInputOverride(int pad_num, GCPadStatus* status)
{
  if (pad_num < 0 || pad_num > 3 || status == nullptr)
    return;
  std::lock_guard<std::mutex> lk(s_input_mutex);
  if (s_input[pad_num].engaged)
    *status = s_input[pad_num].status;
}

void Start(Core::System& system)
{
  if (s_started.exchange(true))
    return;
  const char* port_env = std::getenv("EMUCAP_PORT");
  if (!port_env || !*port_env)
    return;
  const unsigned short port = static_cast<unsigned short>(std::atoi(port_env));
  if (port == 0)
    return;
  // exec 브레이크포인트는 IsDebuggingEnabled() 일 때만 코어가 체크한다(config 의존 제거 —
  // emucap 어댑터가 붙으면 항상 디버깅을 켠다). CachedInterpreter/JIT 모두 이 플래그를 본다.
  Config::SetBaseOrCurrent(Config::MAIN_ENABLE_DEBUGGING, true);
  s_stop.store(false);
  s_thread = std::thread([&system, port] { ThreadMain(system, port); });
}

void Stop()
{
  s_stop.store(true);
  if (s_thread.joinable())
    s_thread.join();
  s_started.store(false);
}
}  // namespace EmuCap
