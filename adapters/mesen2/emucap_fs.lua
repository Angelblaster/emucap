local Fs = {}

local function validate_relative_path(path)
  if type(path) ~= "string" or path == "" then
    return false, "directory path must be a non-empty string"
  end
  if #path > 512 then
    return false, "directory path exceeds 512 bytes"
  end
  if path:sub(1, 1) == "/" or path:sub(1, 1) == "\\" or path:match("^%a:") then
    return false, "directory path must be relative"
  end
  if path:find("\\", 1, true) or path:find("//", 1, true) or path:sub(-1) == "/" then
    return false, "directory path must use non-empty slash-separated components"
  end

  for component in path:gmatch("[^/]+") do
    if component == "." or component == ".."
        or not component:match("^[A-Za-z0-9_][A-Za-z0-9_.%-]*$") then
      return false, "directory path contains an unsafe component: " .. component
    end
  end
  return true
end

function Fs.ensure_relative_directory(path, execute, directory_separator)
  local valid, validation_error = validate_relative_path(path)
  if not valid then return false, validation_error end

  execute = execute or os.execute
  directory_separator = directory_separator
    or (package.config and package.config:sub(1, 1))
    or "/"

  local command
  if directory_separator == "\\" then
    local windows_path = path:gsub("/", "\\")
    command = 'if not exist "' .. windows_path .. '\\NUL" mkdir "' .. windows_path
      .. '" >NUL 2>&1'
  else
    command = "mkdir -p -- '" .. path .. "'"
  end

  local ok, exit_kind, exit_code = execute(command)
  if ok == true or ok == 0 then return true end
  return false, string.format(
    "directory creation failed (%s, %s): %s",
    tostring(exit_kind), tostring(exit_code), path)
end

return Fs
