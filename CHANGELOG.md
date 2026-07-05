# Changelog

Beta software — interfaces may still change.

## 0.4.0

### Added
- **PSP (PlayStation Portable)** adapter, via a headless PPSSPP fork with a WebSocket debugger bridge: memory, registers, screenshot, buttons, save/load state, disassemble, instruction stepping, execution and read/write breakpoints, and reset. Build with `adapters/ppsspp/build.sh`.
- PSP `display: true` HITL mode — the adapter opens a real PPSSPP window a human watches and plays (keyboard/gamepad) while the agent reads and injects over the debugger WebSocket, mirroring the NDS display mode; the GUI runs under an isolated per-session profile, so it never touches the operator's real PPSSPP config or saves.
- PSP `dump_memory` — bulk-export a memory region to `<dir>/<region>.bin` + `regions.json` for large regions, instead of inline hex.
- NDS `dump_memory` and `find_pattern` — bulk-export Main RAM to region files, and scan a region for a byte pattern, mirroring the other adapters.

### Fixed
- PSP: the debugger WebSocket listens on loopback only, so it is not reachable from other hosts.
- PSP: `reset` performs a real reboot and reports failure when the reboot fails, instead of acknowledging a no-op.
- PSP: `main` reads and writes outside PSP user RAM are rejected instead of aliasing onto other memory.
- PSP: duplicate memory and execution breakpoints on one address are ref-counted, so clearing one no longer disarms another.
- Mesen: the GBA BIOS is resolved from the documented firmware directory, and an already-staged BIOS is used when its source is gone.
- NDS: a memory read or register read before `poll_events` no longer resumes past a pending breakpoint stop.

### Changed
- `EMUCAP_PPSSPP_SRC` is read-only input: the build clones it into the owned work tree and never patches or builds in place.

## 0.3.1

### Fixed
- NDS: memory reads, writes, and breakpoints out of the selected region are rejected instead of reaching unrelated bus space.
- NDS: a breakpoint hit is no longer lost when a memory read or screenshot runs before `poll_events`.
- NDS: an emulator or bridge that crashes at startup fails the launch instead of reporting success and leaving a stray process.
- Mesen: GBA launches stage the BIOS (from `EMUCAP_GBA_BIOS`, else the mesen2 firmware directory) instead of hanging on a firmware prompt, and fail with a clear message when it is missing.

### Changed
- NDS `read_memory` is capped at 128 KB per call — read a larger region in chunks.
- Adapter READMEs are English only; `README.ko.md` at the repo root remains the Korean entry point.

## 0.3.0

### Added
- **Nintendo DS** adapter, via a headless DeSmuME fork with an ARM9/ARM7 GDB bridge: memory, registers, screenshot, buttons, touchscreen (`touch`), save/load state, disassemble, call_stack, reset, execution breakpoints, and an optional window for human-in-the-loop play (`display: true`). Build with `adapters/desmume-nds/build.sh`.
- **Game Boy, Game Boy Color, Game Boy Advance, and NES** on the Mesen2 adapter, with a GBA ARM7 disassembler.
- Game Gear / Master System VRAM write breakpoints.
- `owned_instance` in `status` — the pids and pidfiles this session started, for scoped cleanup.
- Optional `cpu` argument on `get_state` / `resume` / `pause` / `step` for multi-core backends (NDS ARM9/ARM7).
- `EMUCAP_DEADMAN_MS<=0` holds a freeze indefinitely.

### Changed
- Tool argument descriptions defer per-system specifics to `status` and each adapter's README.
- The DeSmuME fork build is pinned to a known-good commit.

### Fixed
- Mesen2 `get_state` after a freeze reports the frozen instant, not a drifted one.
- NDS screenshots and memory reads taken while the game runs are no longer stale or corrupted.
- NDS `reset` leaves the game paused; a failed launch no longer leaves a stray emulator process.
- NDS `step_instructions` steps by instruction, and `poll_events` reports breakpoint hits without noise.
- NDS timed `press_buttons` / `touch` require a running game.
- NDS screenshot and disassemble no longer fail intermittently, and parallel NDS sessions no longer collide on a GDB port.

## 0.2.0

### Added
- Game Gear / Master System on the Mesen2 adapter (Z80). Launch with `system: "gamegear"`; buttons and `sms*` memory types are documented in `adapters/mesen2/README.md`.
- PC-98 second floppy (`content_path2` → `-flop2`) for two-drive titles.
- `watch_register` accepts a capped `max_instructions` budget.

### Changed
- Mesen2 adapter split into a shared `emucap-core.lua` plus per-system entries (`emucap-snes.lua`, `emucap-sms.lua`).
- `read_memory` over the size cap now returns an error instead of truncating.
- Frame counts and input-hold durations are capped to fit the link deadline.

### Fixed
- Mesen2 work-RAM read/write breakpoints now fire (RAM offset → CPU-bus address); multi-byte value filters read the correct bytes.
- Mesen2 / Mednafen hot breakpoints no longer flood the emulator thread and drop the connection.
- PC-98 GDB-RSP stream no longer desyncs when a `run_frames` frame target coincides with a breakpoint hit while tracing; the frozen-idle loop no longer fork-storms.
- Mednafen Saturn rejects the unimplemented `physical` address space instead of silent 0-reads / no-op writes.
- TCP and broker links: poison on partial write, deferred deadline against endless `working` keepalives, and split-reply demux.
- `track` observe rejects truncated reads (a hashed prefix could give a false pass/fail).
- Flycast: Dreamcast addresses at or above `0x80000000` no longer truncate on a 32-bit `long` (Windows) — JSON numbers parse via `strtoull` ([#1](https://github.com/mcpads/emucap/pull/1), thanks @UzuCore). Build-hook injection is idempotent and CRLF-normalized.

## 0.1.0

Initial public snapshot.
