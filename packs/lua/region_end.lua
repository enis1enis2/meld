-- End-scanner reference for plain `@lua { ... }` blocks.
-- JUDGMENT CALL: the host uses the generic scanner until Wasm scanners land.
function find_region_end(text, start, marker)
  local depth = 1
  local quote = nil
  local i = start
  while i <= #text do
    local c = text:sub(i, i)
    if quote then
      if c == "\\" then i = i + 2
      elseif c == quote then quote = nil end
    elseif c == '"' or c == "'" then quote = c
    elseif c == "-" and text:sub(i + 1, i + 1) == "-" then
      if text:sub(i + 2, i + 2) == "[" then break end -- long comment: use raw delimiters
      while i <= #text and text:sub(i, i) ~= "\n" do i = i + 1 end
    elseif c == "{" then depth = depth + 1
    elseif c == "}" then
      depth = depth - 1
      if depth == 0 then return i - 1 end
    end
    i = i + 1
  end
  return -1
end

return { find_region_end = find_region_end }
