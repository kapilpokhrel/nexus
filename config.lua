proxy:register_hook("example.com", "auth", 0)
proxy:on_request("example.com", function(client, req, context)
    resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
    user = context:get("user")
    if not user then
        return resp
    end
    resp:add_header("user", user)
    return resp
end, 0)

proxy:listen_on("127.0.0.1:5000")
