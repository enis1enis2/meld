-- Meld Value Model wire codec for Lua 5.4 (spec/value-model.md).
-- Uses lua-cjson for JSON framing; value tags are implemented here.
--
-- JUDGMENT CALL: a plain Lua array (contiguous integer keys 1..n) encodes as a
-- Meld list; any other plain table encodes as `$map`. Tagged constructors are
-- available for ambiguous cases. A decimal-digit string decodes as Meld int.

local cjson = require("cjson")

local Meld = {}
local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

Meld.NULL = setmetatable({}, { __meld = "null" })

local function tag(name, t)
  return setmetatable(t or {}, { __meld = name })
end

function Meld.map(t) return tag("map", t or {}) end
function Meld.variant(case, value) return tag("variant", { case = case, value = value }) end
function Meld.some(v) return tag("some", { value = v }) end
function Meld.ok(v) return tag("ok", { value = v }) end
function Meld.err(v) return tag("err", { value = v }) end
function Meld.handle(id, resource, borrow)
  return tag("handle", { id = id, resource = resource, borrow = not not borrow })
end
function Meld.fn(target) return tag("fn", { target = target }) end
-- JUDGMENT CALL: Lua has no distinct byte-string type, so bytes need an
-- explicit wrapper to survive a decode -> encode round-trip.
function Meld.bytes(s) return tag("bytes", { value = s }) end

local function meltag(v)
  if type(v) ~= "table" then return nil end
  local mt = getmetatable(v)
  return type(mt) == "table" and mt.__meld or nil
end

local function b64encode(s)
  local out, i = {}, 1
  while i <= #s do
    local a = s:byte(i)
    local b = i + 1 <= #s and s:byte(i + 1) or nil
    local c = i + 2 <= #s and s:byte(i + 2) or nil
    local n = a * 65536 + (b or 0) * 256 + (c or 0)
    local i1 = math.floor(n / 262144) % 64 + 1
    local i2 = math.floor(n / 4096) % 64 + 1
    local i3 = math.floor(n / 64) % 64 + 1
    local i4 = n % 64 + 1
    out[#out + 1] = B64:sub(i1, i1)
    out[#out + 1] = B64:sub(i2, i2)
    out[#out + 1] = b and B64:sub(i3, i3) or "="
    out[#out + 1] = c and B64:sub(i4, i4) or "="
    i = i + 3
  end
  return table.concat(out)
end

local function b64decode(s)
  local out = {}
  local i = 1
  while i <= #s do
    local n = 0
    for j = 0, 3 do
      local ch = s:sub(i + j, i + j)
      local v = B64:find(ch, 1, true)
      if not v or ch == "=" then break end
      n = n * 64 + (v - 1)
    end
    out[#out + 1] = string.char(math.floor(n / 65536) % 256)
    if s:sub(i + 2, i + 2) ~= "=" then out[#out + 1] = string.char(math.floor(n / 256) % 256) end
    if s:sub(i + 3, i + 3) ~= "=" then out[#out + 1] = string.char(n % 256) end
    i = i + 4
  end
  return table.concat(out)
end

local function is_decimal(s)
  return type(s) == "string" and #s > 0 and s:match("^%-?%d+$") ~= nil
end

local function is_array(t)
  local n = 0
  for k in pairs(t) do
    if type(k) ~= "number" or k ~= math.floor(k) then return false end
    n = math.max(n, k)
  end
  for k = 1, n do
    if t[k] == nil then return false end
  end
  return true
end

function Meld.encode(v)
  local k = meltag(v)
  if k == "null" or v == nil or v == cjson.null then return cjson.null end
  if type(v) == "boolean" then return v end
  if type(v) == "number" then
    if v ~= v then return "NaN" end
    if v == math.huge then return "Inf" end
    if v == -math.huge then return "-Inf" end
    if math.type(v) == "integer" then return tostring(v) end
    return v
  end
  if type(v) == "string" then return v end
  if type(v) ~= "table" then error("cannot encode Lua " .. type(v) .. " as Meld value") end
  if k == "variant" then
    return { ["$variant"] = { case = v.case, value = Meld.encode(v.value) } }
  elseif k == "some" then return { ["$some"] = Meld.encode(v.value) }
  elseif k == "ok" then return { ["$ok"] = Meld.encode(v.value) }
  elseif k == "err" then return { ["$err"] = Meld.encode(v.value) }
  elseif k == "handle" then
    return { ["$handle"] = { id = v.id, resource = v.resource, borrow = v.borrow } }
  elseif k == "fn" then return { ["$fn"] = v.target }
  elseif k == "bytes" then return { ["$bytes"] = b64encode(v.value) }
  elseif k == "map" then
    local out = {}
    for key, val in pairs(v) do out[tostring(key)] = Meld.encode(val) end
    return { ["$map"] = out }
  elseif is_array(v) then
    local out = setmetatable({}, cjson.empty_array_mt)
    for i = 1, #v do out[i] = Meld.encode(v[i]) end
    return out
  end
  local out = {}
  for key, val in pairs(v) do out[tostring(key)] = Meld.encode(val) end
  return { ["$map"] = out }
end

function Meld.decode(w)
  if w == nil or w == cjson.null then return Meld.NULL end
  if type(w) == "boolean" then return w end
  if type(w) == "number" then return w + 0.0 end -- JSON number -> Meld float
  if type(w) == "string" then
    if w == "NaN" then return 0 / 0 end
    if w == "Inf" then return math.huge end
    if w == "-Inf" then return -math.huge end
    if is_decimal(w) then
      -- JUDGMENT CALL: Lua integers are 64-bit. Keep larger decimal ints as
      -- their canonical string so arbitrary precision survives the bridge.
      local n = tonumber(w)
      local i = math.tointeger(n)
      if i ~= nil and tostring(i) == w then return i end
      return w
    end
    return w
  end
  if type(w) ~= "table" then error("cannot decode Meld wire value") end
  local k = meltag(w)
  if k then return w end -- already a tagged native value
  if w["$bytes"] then return Meld.bytes(b64decode(w["$bytes"])) end
  if w["$map"] then
    local out = {}
    for key, val in pairs(w["$map"]) do out[key] = Meld.decode(val) end
    return Meld.map(out)
  end
  if w["$variant"] then
    return Meld.variant(w["$variant"].case, Meld.decode(w["$variant"].value))
  end
  if w["$some"] then return Meld.some(Meld.decode(w["$some"])) end
  if w["$ok"] then return Meld.ok(Meld.decode(w["$ok"])) end
  if w["$err"] then return Meld.err(Meld.decode(w["$err"])) end
  if w["$handle"] then
    local h = w["$handle"]
    return Meld.handle(h.id, h.resource, h.borrow)
  end
  if w["$fn"] then return Meld.fn(w["$fn"]) end
  if is_array(w) then
    local out = {}
    for i = 1, #w do out[i] = Meld.decode(w[i]) end
    return out
  end
  local out = {}
  for key, val in pairs(w) do out[key] = Meld.decode(val) end
  return Meld.map(out)
end

Meld.json = cjson
return Meld
