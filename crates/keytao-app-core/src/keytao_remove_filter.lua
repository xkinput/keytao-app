local M = {}

function M.init(env)
  env.removed = {}
  -- Reload on every initialization, including the automatic deploy after edits.
  pcall(function()
    local path = rime_api.get_user_data_dir() .. "/keytao.user.remove.txt"
    local file = io.open(path, "r")
    if not file then return end
    local removed = {}
    local ok = pcall(function()
      for line in file:lines() do
        local body = line:gsub("\r$", "")
        local word, code = body:match("^([^\t]+)\t([^\t]+)$")
        if word and code and not code:find("%s") then
          removed[word] = removed[word] or {}
          table.insert(removed[word], code)
        end
      end
    end)
    file:close()
    if ok then env.removed = removed end
  end)
end

function M.tags_match(_, env)
  return next(env.removed) ~= nil
end

function M.func(translation, env)
  local input = env.engine.context.input
  for cand in translation:iter() do
    local hidden = false
    for _, code in ipairs(env.removed[cand.text] or {}) do
      local segment = input:sub(cand.start + 1, cand._end)
      if code:sub(1, #segment) == segment then
        hidden = true
        break
      end
    end
    if not hidden then yield(cand) end
  end
end

return M
