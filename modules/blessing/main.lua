local backend_blessings = {
    "May your queries use indexes.",
    "May your cache be warm.",
    "May your locks be short.",
    "May your uptime be long.",
    "May your latency stay low.",
    "May your CPU stay cool.",
    "May your memory never leak.",
    "May your threads cooperate.",
    "May your mutexes never deadlock.",
    "May your packets arrive in order.",
    "May your packets never drop.",
    "May your ACKs arrive promptly.",
    "May your sockets stay open.",
    "May your load be balanced.",
    "May your coffee stay hot.",
    "May your congestion be gentle.",
    "May your bandwidth be plentiful."
}

blessing:add_incomming_hook(function(client, req)
    local resp = blessing:resp_update_ctx()
    local random_blession = backend_blessings[math.random(#backend_blessings)]
    resp:add_to_ctx("blessing", random_blession)
    return resp
end)
