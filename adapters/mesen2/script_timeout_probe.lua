-- Verify that the numeric --debug.scriptWindow.scriptTimeout override is applied.
-- The compatible host completes this >1s callback and writes EMUCAP_TIMEOUT_PROBE_MARKER; the pinned
-- unpatched baseline leaves ScriptTimeout at its 1s default and aborts before the marker.

local socket = require("socket.core")
local marker = assert(os.getenv("EMUCAP_TIMEOUT_PROBE_MARKER"), "probe marker path is required")
emu.addEventCallback(function()
  local deadline = socket.gettime() + 1.25
  while socket.gettime() < deadline do end

  local file = assert(io.open(marker, "wb"))
  file:write("completed\n")
  file:close()
  emu.exit(0)
end, emu.eventType.startFrame)
