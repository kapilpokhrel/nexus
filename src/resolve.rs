use crate::lua_config::{Client, LuaConf, OnAcceptHandlerResp, OnRequestHandlerResp, Request};
use mlua::prelude::*;
use std::io;

/// Headers the config handler asked to add to the upstream request.
pub type ExtraHeaders = Option<Vec<(String, String)>>;
pub type Backend = (String, ExtraHeaders, bool); // address, extra headers, keep-alive

///// Deep-copies an arbitrary Lua value from its original state into a completely different Lua state.
// fn transfer_value_between_states(val: LuaValue, dest_lua: &Lua) -> mlua::Result<LuaValue> {
//     match val {
//         LuaValue::Nil => Ok(LuaValue::Nil),
//         LuaValue::Boolean(b) => Ok(LuaValue::Boolean(b)),
//         LuaValue::Integer(i) => Ok(LuaValue::Integer(i)),
//         LuaValue::Number(n) => Ok(LuaValue::Number(n)),
//
//         LuaValue::String(s) => {
//             let bytes = s.as_bytes();
//             Ok(LuaValue::String(dest_lua.create_string(bytes)?))
//         }
//
//         LuaValue::Table(t) => {
//             let dest_table = dest_lua.create_table()?;
//             for pair in t.pairs::<LuaValue, LuaValue>() {
//                 let (k, v) = pair?;
//                 let new_k = transfer_value_between_states(k, dest_lua)?;
//                 let new_v = transfer_value_between_states(v, dest_lua)?;
//                 dest_table.set(new_k, new_v)?;
//             }
//             Ok(LuaValue::Table(dest_table))
//         }
//
//         _ => Err(mlua::Error::RuntimeError(
//             "Cannot move functions, threads, or raw userdata pointers between independent states"
//                 .into(),
//         )),
//     }
// }

pub async fn resolve_backend(
    host: &str,
    client: Client,
    req: Request,
    state: &LuaConf,
) -> io::Result<Backend> {
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

    let handle_resp = |resp: OnRequestHandlerResp| {
        if let OnRequestHandlerResp::ForwardHttp(b) = resp {
            Ok((
                b.upstream,
                b.headers_patch.map(|p| p.add_headers),
                b.keep_alive,
            ))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected response type",
            ))
        }
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
    handle_resp(resp)
}

/// Runs `proxy:on_accept(client)` if the config defined one. `false` means the
/// connection was denied and should be dropped without reading a request.
/// Only understand Deny handler response right now
pub fn accept_allowed(state: &LuaConf, client: &Client) -> io::Result<bool> {
    let f = state.on_accept_fn.borrow().clone();
    let Some(f) = f else { return Ok(true) };

    let h = state.pool.checkout();
    h.set_client(client.clone());
    let resp = f
        .call::<OnAcceptHandlerResp>(&h.client_ud)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    Ok(!matches!(resp, OnAcceptHandlerResp::Deny(_)))
}
