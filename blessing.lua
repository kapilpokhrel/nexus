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

local blessing = {}
function blessing.get_random_blessing()
    return backend_blessings[math.random(#backend_blessings)]
end

return blessing
