#!/usr/bin/env lua
-- Meld T2 worker for Lua 5.4, conformance level L2.
-- Protocol: spec/t2-protocol.md. Wire values: spec/value-model.md.
-- Requires lua-cjson (declared in toolchain.toml).

local script = arg and arg[0] or "worker.lua"
local dir = script:match("^(.*[/\\])") or "./"
package.path = dir .. "?.lua;" .. package.path

local Meld = require("marshal")
local cjson = Meld.json

local PROTOCOL = "meld-t2/1"
local LEVEL = "L2"
local CAPS = { "values", "stdout", "eval" }

local stdout_lines = {}
local handles = {}
local namespace = {}

namespace.print = function(...)
  local parts = {}
  for i = 1, select("#", ...) do parts[i] = tostring((select(i, ...))) end
  stdout_lines[#stdout_lines + 1] = table.concat(parts, "\t")
end
setmetatable(namespace, { __index = _G })

local function raise(code, message, source)
  error({ code = code, message = message, source = source }, 0)
end

local function empty_array() return Meld.empty_array() end

local function initialize(params)
  if params.protocol ~= PROTOCOL then
    raise("E_PROTOCOL", "expected " .. PROTOCOL .. ", got " .. tostring(params.protocol))
  end
  return { protocol = PROTOCOL, level = LEVEL, caps = CAPS }
end

local function execute(params)
  stdout_lines = {}
  local region_id = tostring(params.region_id or "")
  local name = "<meld:" .. region_id .. ">"
  local chunk, err = load(tostring(params.code or ""), name, "t", namespace)
  if not chunk then
    raise("RuntimeError", tostring(err), { region_id = region_id, lang = "lua" })
  end
  local ok, runerr = pcall(chunk)
  if not ok then
    local message = type(runerr) == "table" and runerr.message or tostring(runerr)
    raise("RuntimeError", message, { region_id = region_id, lang = "lua" })
  end
  local lines = stdout_lines
  stdout_lines = {}
  return { stdout = Meld.array(lines), exports = empty_array() }
end

-- JUDGMENT CALL: Lua has expressions but no expression-only compile mode, so
-- inline islands are wrapped in `return ...` inside the region namespace.
local function evaluate(params)
  stdout_lines = {}
  local region_id = tostring(params.region_id or "")
  local name = "<meld-inline:" .. region_id .. ">"
  -- Delimiter padding is Meld syntax; workers accept expression text only.
  local code = tostring(params.code or ""):match("^%s*(.-)%s*$")
  local chunk, err = load("return " .. code, name, "t", namespace)
  if not chunk then
    raise("RuntimeError", tostring(err), { region_id = region_id, lang = "lua" })
  end
  local ok, value = pcall(chunk)
  if not ok then
    local message = type(value) == "table" and value.message or tostring(value)
    raise("RuntimeError", message, { region_id = region_id, lang = "lua" })
  end
  local lines = stdout_lines
  stdout_lines = {}
  return { value = Meld.encode(value), stdout = Meld.array(lines) }
end

local function fetch(params)
  local name = params.name
  local value = namespace[name]
  if value == nil then raise("UnknownBinding", "no binding named " .. tostring(name)) end
  return { value = Meld.encode(value) }
end

local function import_value(params)
  local value = Meld.decode(params.value)
  namespace[params.name] = value
  local k = getmetatable(value)
  if type(k) == "table" and k.__meld == "handle" and not value.borrow then
    handles[value.id] = value
  end
  return { ok = true }
end

local function call(params)
  local target = params.fn
  local args = {}
  for i, wire in ipairs(params.args or {}) do args[i] = Meld.decode(wire) end
  local name = type(target) == "table" and target["$fn"] or target
  local fn = type(name) == "string" and namespace[name] or nil
  if type(fn) ~= "function" then
    raise("TypeMismatch", tostring(name) .. " is not a callable binding")
  end
  return { value = Meld.encode(fn(table.unpack(args))) }
end

local function drop(params)
  handles[params.handle_id] = nil
  return { ok = true }
end

local function shutdown() return { ok = true } end

local DISPATCH = {
  initialize = initialize,
  execute = execute,
  eval = evaluate,
  fetch = fetch,
  import = import_value,
  call = call,
  drop = drop,
  shutdown = shutdown,
}

local function emit(obj)
  io.write(Meld.encode_json(obj), "\n")
  io.flush()
end

for line in io.lines() do
  if line and line:match("%S") then
    local ok, req = pcall(cjson.decode, line)
    if not ok then
      emit({ id = 0, error = { code = "E_PROTOCOL", message = tostring(req), source = cjson.null } })
    else
      local id = req.id or 0
      local method = req.method or ""
      local params = req.params or {}
      local handled, result = pcall(function()
        local handler = DISPATCH[method]
        if not handler then raise("E_PROTOCOL", "unknown method " .. tostring(method)) end
        return handler(params)
      end)
      if handled then
        emit({ id = id, result = result })
      else
        local err = result
        if type(err) == "table" and err.code then
          emit({ id = id, error = { code = err.code, message = err.message, source = err.source or cjson.null } })
        else
          emit({ id = id, error = { code = "InternalError", message = tostring(err), source = cjson.null } })
        end
      end
      if method == "shutdown" then break end
    end
  end
end
