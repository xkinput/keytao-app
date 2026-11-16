local filter = dofile("crates/keytao-app-core/src/keytao_remove_filter.lua")
local original_open = io.open
local content = "你好\tnihk\n你们\tnimf\n你好\tabcd\n"
rime_api = { get_user_data_dir = function() return "/user" end }
io.open = function(path)
  assert(path == "/user/keytao.user.remove.txt")
  if content == nil then return nil end
  local lines = content:gmatch("[^\n]+")
  return { lines = function() return lines end, close = function() end }
end

local env = { engine = { context = { input = "ni" } } }
local function candidates(input, start, finish)
  env.engine.context.input = input
  local source = {
    { text = "你好", start = start or 0, _end = finish or #input },
    { text = "你们", start = start or 0, _end = finish or #input },
    { text = "其他", start = start or 0, _end = finish or #input },
  }
  local result = {}
  yield = function(cand) table.insert(result, cand.text) end
  filter.func({ iter = function()
    local index = 0
    return function() index = index + 1; return source[index] end
  end }, env)
  return table.concat(result, ",")
end

filter.init(env)
assert(candidates("ni") == "其他")
assert(candidates("nihk") == "你们,其他")
assert(candidates("abcd") == "你们,其他")
assert(candidates("nihkx") == "你好,你们,其他")
assert(candidates("xniz", 1, 3) == "其他")
content = "你们\tnimf\r\ninvalid\n"
filter.init(env)
assert(candidates("ni") == "你好,其他")
content = nil
filter.init(env)
assert(candidates("ni") == "你好,你们,其他")
io.open = function() error("unreadable") end
filter.init(env)
assert(candidates("ni") == "你好,你们,其他")
io.open = original_open
print("remove_filter: passed")
