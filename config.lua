-- proxy:on_route("example.com", function(req)
--     return "127.0.0.1:8000"
-- end)
proxy:on_route_static("example.com", "127.0.0.1:8000")

proxy:listen_on("127.0.0.1:5000")
