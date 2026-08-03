local socket = require("socket")

local SLEEP_MS = 50
local BLOCKING = false

sleeptest:add_incomming_hook(function(client, req)
    if BLOCKING then
        socket.sleep(SLEEP_MS / 1000)
    else
        conduit.sleep(SLEEP_MS)
    end
    return sleeptest:resp_pass()
end)
