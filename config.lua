local blessing = require("blessing")
local iploc = require("iploc")

proxy:on_request("example.com", function(client, req)
    handler_resp = proxy:forward_http("127.0.0.1:8000")
    handler_resp:keep_alive(false)
    handler_resp:set_max_request_body_size(1048576)
    handler_resp:set_max_response_body_size(1048576)
    handler_resp:add_header("X_GREETING", "HELLO")
    handler_resp:add_header("COUNTRY", proxy:cached_call(
        "example.com",
        proxy:cache_param(60, client.ip),
        iploc.get_country,
        "103.5.150.16"
    ))
    handler_resp:add_header("X_BLESSING", blessing.get_random_blessing())
    return handler_resp
end)

proxy:listen_on("127.0.0.1:5000")
