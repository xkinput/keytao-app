local M = {}

local MARK = "☯"

local function valid_language(language)
  if type(language) ~= "string" or language == "" then
    return false
  end
  if language == "." or language == ".." then
    return false
  end
  return not language:find("[/\\\0]")
end

local function model_exists(language)
  if not valid_language(language) then
    return false
  end
  local file_name = language .. ".gram"
  local dirs = {}
  if rime_api then
    if type(rime_api.get_user_data_dir) == "function" then
      table.insert(dirs, rime_api.get_user_data_dir())
    end
    if type(rime_api.get_shared_data_dir) == "function" then
      table.insert(dirs, rime_api.get_shared_data_dir())
    end
  end
  for _, dir in ipairs(dirs) do
    if type(dir) == "string" and dir ~= "" then
      local path = dir:gsub("[/\\]+$", "") .. "/" .. file_name
      local file = io.open(path, "rb")
      if file then
        file:close()
        return true
      end
    end
  end
  return false
end

function M.init(env)
  env.enabled = false
  pcall(function()
    local config = env and env.engine and env.engine.schema and env.engine.schema.config
    if not config then return end
    if config:get_bool("keytao/grammar_hint") == false then return end
    local language = config:get_string("grammar/language")
    if model_exists(language) then
      env.enabled = true
    end
  end)
end

function M.tags_match(_, env)
  return env.enabled == true
end

function M.func(translation, env)
  for cand in translation:iter() do
    if env.enabled then
      local ok, out = pcall(function()
        local genuine = (cand.get_genuine and cand:get_genuine()) or cand
        if genuine.type ~= "sentence" then
          return cand
        end
        local text = cand.text or ""
        local comment = cand.comment or ""
        if text:find(MARK, 1, true) or comment:find(MARK, 1, true) then
          return cand
        end
        local updated = comment == "" and MARK or (comment .. " " .. MARK)
        pcall(function() cand.comment = updated end)
        if cand.comment == updated then
          return cand
        end
        pcall(function() genuine.comment = updated end)
        if cand.comment == updated then
          return cand
        end
        if ShadowCandidate then
          return ShadowCandidate(cand, cand.type, text, updated)
        end
        return cand
      end)
      if ok and out then
        yield(out)
      else
        yield(cand)
      end
    else
      yield(cand)
    end
  end
end

return M
