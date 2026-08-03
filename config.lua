proxy:register_hook("example.com", "auth", 60, "$header:Authorization")
proxy:register_hook("example.com", "iploc", 60, "$ip")
-- proxy:register_hook("example.com", "sleeptest", 0, "")
-- proxy:register_hook("example.com", "ratelimit", 0, "")

proxy:on_request("example.com", function(client, req, context)
    -- if context:get("ratelimit_allowed") == false then
    --     return proxy:resp_deny("rate limited")
    -- end

    resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
    user = context:get("user")
    if not user then
        return resp
    end
    resp:add_header("user", user)
    return resp
end)

proxy:listen_on("127.0.0.1:5000")
