-- proxy:register_hook("example.com", "iploc", proxy:cacheability(60, "$ip"))
-- proxy:register_hook("example.com", "blessing", nil)
proxy:on_request("example.com", function(client, req, context)
    resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
    resp:add_header("X_GREETING", "HELLO")
    return resp
end)

proxy:listen_on("127.0.0.1:5000")
