local function b64decode(data)
    local b = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'

    data = data:gsub('[^' .. b .. '=]', '')

    return (data:gsub('.', function(x)
        if x == '=' then
            return ''
        end

        local r, f = '', (b:find(x, 1, true) - 1)

        for i = 6, 1, -1 do
            r = r .. ((f % 2^i - f % 2^(i - 1) > 0) and '1' or '0')
        end

        return r
    end):gsub('%d%d%d?%d?%d?%d?%d?%d?', function(x)
        if #x ~= 8 then
            return ''
        end

        local c = 0
        for i = 1, 8 do
            if x:sub(i, i) == '1' then
                c = c + 2^(8 - i)
            end
        end
        return string.char(c)
    end))
end

local sqlite3 = require("lsqlite3")

local db = sqlite3.open("/home/kapil/dev/conduit/users.db")

function lookup_user(username)
    local stmt = db:prepare(
        "SELECT username FROM users WHERE username = ?"
    )
    stmt:bind(1, username)

    if stmt:step() == sqlite3.ROW then
        local row = stmt:get_named_values()
        return row.username
    end
end

auth:add_incomming_hook("example.com", function(client, req, context)
    local authz = req:header("Authorization")
    local resp = auth:resp_update_ctx()
    if not authz then
        return resp
    end

    local b64 = authz:match("^Basic%s+(.+)$")
    if not b64 then
        return resp
    end

    local decoded = b64decode(b64)
    local username = decoded:match("^([^:]+):")

    if not username then
        return resp
    end

    local user = lookup_user(username)
    if not user then
        return resp
    end

    resp:add_to_ctx("user", user)
    return resp
end)
