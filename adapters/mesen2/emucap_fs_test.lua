local dir = os.getenv("EMUCAP_ADAPTER_DIR") or "."
package.path = dir .. "/?.lua;" .. package.path
local Fs = require("emucap_fs")

local commands = {}
local function successful_execute(command)
  commands[#commands + 1] = command
  return true, "exit", 0
end

local ok, err = Fs.ensure_relative_directory(
  "bundles/1730000000-retrospective/slices/f00120",
  successful_execute,
  "/")
assert(ok, tostring(err))
assert(commands[1] ==
  "mkdir -p -- 'bundles/1730000000-retrospective/slices/f00120'")

local windows_ok = Fs.ensure_relative_directory(
  "bundles/1730000000-retrospective/slices",
  successful_execute,
  "\\")
assert(windows_ok)
assert(commands[2] ==
  'if not exist "bundles\\1730000000-retrospective\\slices\\NUL" mkdir "bundles\\1730000000-retrospective\\slices" >NUL 2>&1')

local rejected = {
  "../outside",
  "/absolute",
  "C:/absolute",
  "has space",
  "a/$()/b",
  "a/'quoted'/b",
  "a/semicolon;/b",
  "a//b",
  "a\\b",
}
for _, path in ipairs(rejected) do
  local calls_before = #commands
  local accepted, rejection = Fs.ensure_relative_directory(path, successful_execute, "/")
  assert(not accepted, "unsafe path accepted: " .. path)
  assert(type(rejection) == "string" and rejection ~= "")
  assert(#commands == calls_before, "executor ran for rejected path: " .. path)
end

local failed, failure = Fs.ensure_relative_directory(
  "bundles/safe",
  function() return nil, "exit", 1 end,
  "/")
assert(not failed)
assert(failure:match("directory creation failed"))

print("ALL MESEN FS TESTS PASSED")
