use crate::lua_config::{Client, LuaConf, OnAcceptHandlerResp, OnRequestHandlerResp, Request};
use mlua::prelude::*;
use std::io;

pub async fn run_on_request(
    host: &str,
    client: Client,
    req: Request,
    state: &LuaConf,
) -> io::Result<Option<OnRequestHandlerResp>> {
    let handler_fn = state.req_handlers.borrow();
    let Some(lua_fn) = handler_fn.get(host).cloned() else {
        return Ok(None);
    };

    let call_lua_fn = || {
        let h = state.handles.create()?;
        h.set_client(client.clone());
        h.set_req(req.clone());

        lua_fn.call::<OnRequestHandlerResp>((&h.client_ud, &h.req_ud))
    };

    let hook_host = host.to_owned();
    state
        .lua
        .set_hook(
            LuaHookTriggers {
                every_line: false,
                on_calls: false,
                on_returns: false,
                // Check every 1000 VM instructions
                every_nth_instruction: Some(10000),
            },
            move |_, _| {
                log::trace!("on_request handler for host '{hook_host}' is still executing");
                Ok(LuaVmState::Continue)
            },
        )
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let result = call_lua_fn();
    state.lua.remove_hook();
    result.map(Some).map_err(|e| {
        io::Error::other(format!(
            "on_request handler for host '{host}' from {}:{} failed: {e}",
            client.0.ip, client.0.port
        ))
    })
}

pub fn run_on_accept(state: &LuaConf, client: &Client) -> io::Result<Option<OnAcceptHandlerResp>> {
    let f = state.on_accept_fn.borrow().clone();
    let Some(f) = f else { return Ok(None) };

    let h = state
        .handles
        .create()
        .map_err(|e| io::Error::other(format!("failed to create Lua handles: {e}")))?;
    h.set_client(client.clone());
    let resp = f.call::<OnAcceptHandlerResp>(&h.client_ud).map_err(|e| {
        io::Error::other(format!(
            "on_accept handler for {}:{} failed: {e}",
            client.0.ip, client.0.port
        ))
    })?;

    Ok(Some(resp))
}
