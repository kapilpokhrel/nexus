proxy:on_request("example.com", function(client, req, context)
    resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
    return resp
end)

proxy:listen_on("127.0.0.1:5000")
