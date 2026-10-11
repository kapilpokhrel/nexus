use crate::lua_config::{Client, LuaConf, OnAcceptHandlerResp, OnRequestHandlerResp, Request};
use mlua::prelude::*;
use std::io;

pub async fn run_on_request(
    host: &str,
    client: Client,
    req: Request,
    state: &LuaConf,
) -> io::Result<OnRequestHandlerResp> {
    let handler_fn = state.req_handlers.borrow();
    let lua_fn = handler_fn.get(host).cloned().unwrap();

    let call_lua_fn = || {
        let h = state.pool.checkout();
        h.set_client(client.clone());
        h.set_req(req.clone());

        lua_fn
            .call::<OnRequestHandlerResp>((&h.client_ud, &h.req_ud))
            .unwrap()
    };

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
                println!("on_request function is executing too much instructions");
                Ok(LuaVmState::Continue)
                // error after 1000 VM instructions
                // Err(mlua::Error::RuntimeError("timeout exceeded".to_string()))
                // or we can check elapsed time and if not timeout, `Ok(LuaVmState::Continue)`
            },
        )
        .unwrap();

    let resp = call_lua_fn();
    state.lua.remove_hook();
    Ok(resp)
}

pub fn run_on_accept(state: &LuaConf, client: &Client) -> io::Result<Option<OnAcceptHandlerResp>> {
    let f = state.on_accept_fn.borrow().clone();
    let Some(f) = f else { return Ok(None) };

    let h = state.pool.checkout();
    h.set_client(client.clone());
    let resp = f
        .call::<OnAcceptHandlerResp>(&h.client_ud)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(Some(resp))
}
