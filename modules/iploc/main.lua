ip2location = require('ip2location')
local ip2loc = ip2location:new('/home/kapil/IP2LOCATION-LITE-DB1.BIN')

iploc:add_incomming_hook(function(client, req)
    local result = ip2loc:get_all('8.8.8.8')
    local resp = iploc:resp_update_ctx()
    resp:add_to_ctx("country", result.country_short)
    return resp
end)
