-- https://redis.io/tutorials/howtos/ratelimiting
local redis = require("redis")

local MAX_REQUESTS = 10000
local WINDOW_SECONDS = 1

local LUA_SCRIPT = [[
local key = KEYS[1]
local max_requests = tonumber(ARGV[1])
local window_seconds = tonumber(ARGV[2])

local count = redis.call('INCR', key)

if count == 1 then
  redis.call('EXPIRE', key, window_seconds)
end

local pttl = redis.call('PTTL', key)

return { count, pttl }
]]

local rdb = redis.connect("127.0.0.1", 6379)

local function attempt(key)
    local result = rdb:eval(LUA_SCRIPT, 1, key, MAX_REQUESTS, WINDOW_SECONDS)

    local count = result[1]
    local pttl = result[2]

    local allowed = count <= MAX_REQUESTS
    local remaining = math.max(0, MAX_REQUESTS - count)

    local retry_after = nil
    if not allowed then
        retry_after = pttl > 0 and pttl / 1000 or WINDOW_SECONDS
    end

    return allowed, remaining, retry_after
end

ratelimit:add_incomming_hook(function(client, req)
    local resp = ratelimit:resp_update_ctx()

    local allowed, remaining, retry_after = attempt("ratelimit:" .. client.ip)

    resp:add_to_ctx("ratelimit_allowed", allowed)
    resp:add_to_ctx("ratelimit_limit", MAX_REQUESTS)
    resp:add_to_ctx("ratelimit_remaining", remaining)
    if retry_after then
        resp:add_to_ctx("ratelimit_retry_after", retry_after)
    end

    return resp
end)
