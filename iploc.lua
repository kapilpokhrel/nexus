-- https://www.ip2location.com/documentation/ip2location-libraries/lua/get-started

ip2location = require('ip2location')
local ip2loc = ip2location:new('/home/kapil/IP2LOCATION-LITE-DB1.BIN')

local iploc = {}
iploc.get_country = function(ip)
    local result = ip2loc:get_all(ip)
    return result.country_short
end

return iploc
