local blessing = require("blessing")
local iploc = require("iploc")

proxy:on_request("example.com", function(client, req)
    resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
    resp:add_header("X_GREETING", "HELLO")
    resp:add_header("COUNTRY", proxy:cached_call(
        proxy:cache_param(60, "example.com:" .. client.ip),
        iploc.get_country,
        "103.5.150.16"
    ))
    resp:add_header("X_BLESSING", blessing.get_random_blessing())
    return resp
end)

proxy:keep_alive(false)
proxy:listen_on("127.0.0.1:5000")
