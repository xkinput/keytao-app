-- Top-up processor by TsFreddie, copied from the supplied keytao fixture unchanged.
local function string2set(str)
    local t = {}
    for i = 1, #str do
        local c = str:sub(i,i)
        t[c] = true
    end
    return t
end

local function topup(env)
    if not env.engine.context:get_selected_candidate() then
        if env.auto_clear then
            env.engine.context:clear()
        end
    else
        env.engine.context:commit()
    end
end

local function processor(key_event, env)
    local engine = env.engine
    local schema = engine.schema
    local context = engine.context

    local input = context.input
    local input_len = #input

    local min_len = env.topup_min
    if context:get_option('danzi_mode') then
        min_len = env.topup_min_danzi
    end

    if key_event:release() or key_event:ctrl() or key_event:alt() then
        return 2
    end

    local ch = key_event.keycode

    if ch < 0x20 or ch >= 0x7f then
        return 2
    end

    local key = string.char(ch)
    local prev = string.sub(input, -1)
    local first = string.sub(input, 1, 1)
    if #first == 0 then
        first = key
    end

    local is_alphabet = env.alphabet[key] or false
    local is_topup = env.topup_set[key] or false
    local is_prev_topup = env.topup_set[prev] or false
    local is_first_topup = env.topup_set[first] or false


    if env.topup_command and is_first_topup then
        return 2
    end

    if not is_alphabet then
        return 2
    end

    if is_prev_topup and not is_topup then
        topup(env)
    elseif not is_prev_topup and not is_topup and input_len >= min_len then
        topup(env)
    elseif input_len >= env.topup_max then
        topup(env)
    end

    return 2
end

local function init(env)
    local config = env.engine.schema.config

    env.topup_set = string2set(config:get_string("topup/topup_with"))
    env.alphabet = string2set(config:get_string("speller/alphabet"))
    env.topup_min = config:get_int("topup/min_length")
    env.topup_min_danzi = config:get_int("topup/min_length_danzi") or env.topup_min
    env.topup_max = config:get_int("topup/max_length")
    env.auto_clear = config:get_bool("topup/auto_clear") or false
    env.topup_command = config:get_bool("topup/topup_command") or false
    env.enabled = true
end

return { init = init, func = processor }
