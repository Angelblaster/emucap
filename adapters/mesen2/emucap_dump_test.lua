local dir = os.getenv("EMUCAP_ADAPTER_DIR") or "."
package.path = dir .. "/?.lua;" .. package.path
local Dump = require("emucap_dump")

local files = {}
local function fake_open(path, mode)
  assert(mode == "wb")
  local file = { chunks = {} }
  function file:write(data)
    self.chunks[#self.chunks + 1] = data
    return self
  end
  function file:close()
    files[path] = table.concat(self.chunks)
    return true
  end
  return file
end

local api = {
  memType = { workRam = 7 },
  read = function(address, memory_type, disable_side_effects)
    assert(memory_type == 7)
    assert(disable_side_effects == false)
    return address + 0x10
  end,
}
local regions = {
  { name = "main", mt = "workRam", base = 0x2000, size = 4 },
}
local path = [[dump with spaces/$() ; '" literal]]
local ok, result = Dump.write({ path = path }, regions, api, fake_open)
assert(ok, tostring(result))
assert(result.path == path)
assert(result.regions == 1)
assert(files[path .. "/main.bin"] == string.char(0x10, 0x11, 0x12, 0x13))
assert(files[path .. "/regions.json"] ==
  '[{"name":"main","memory_type":"workRam","base_address":8192,"size":4}]')

local opened = false
local failed, kind, message = Dump.write(
  { path = "missing" },
  regions,
  api,
  function()
    opened = true
    return nil, "directory missing"
  end)
assert(opened)
assert(not failed)
assert(kind == "io_error")
assert(message:match("directory missing"))

local missing, missing_kind = Dump.write({}, regions, api, fake_open)
assert(not missing)
assert(missing_kind == "bad_params")

print("ALL MESEN DUMP TESTS PASSED")
