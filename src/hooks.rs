use crate::lua_config::{
    CachedValue, Client, Context, HookCacheKeyKind, LuaState, OnRequestResp, Request,
};
use crate::lua_module::{HookResult, ProxyModule, ProxyModuleTable};
use mlua::prelude::*;
use std::borrow::Cow;
use std::io;
use std::rc::Rc;
use std::time::Duration;

/// Deep-copies an arbitrary Lua value from its original state into a completely different Lua state.
fn transfer_value_between_states(
    val: LuaValue,
    dest_lua: &Lua
) -> mlua::Result<LuaValue> {
    match val {
        LuaValue::Nil => Ok(LuaValue::Nil),
        LuaValue::Boolean(b) => Ok(LuaValue::Boolean(b)),
        LuaValue::Integer(i) => Ok(LuaValue::Integer(i)),
        LuaValue::Number(n) => Ok(LuaValue::Number(n)),

        LuaValue::String(s) => {
            let bytes = s.as_bytes();
            Ok(LuaValue::String(dest_lua.create_string(bytes)?))
        }

        LuaValue::Table(t) => {
            let dest_table = dest_lua.create_table()?;
            for pair in t.pairs::<LuaValue, LuaValue>() {
                let (k, v) = pair?;
                let new_k = transfer_value_between_states(k, dest_lua)?;
                let new_v = transfer_value_between_states(v, dest_lua)?;
                dest_table.set(new_k, new_v)?;
            }
            Ok(LuaValue::Table(dest_table))
        }

        _ => Err(mlua::Error::RuntimeError(
            "Cannot move functions, threads, or raw userdata pointers between independent states".into()
        )),
    }
}

async fn run_hooks(
    host: &str,
    client: &Client,
    req: &Request,
    mut context: Context,
    state: &LuaState,
    module_table: &ProxyModuleTable
) {
    let hooks = &state.hooks;
    let n = hooks.borrow().get(host).map(|v| v.borrow().len()).unwrap_or(0);
    for i in 0..n {
        let (module, cache_ttl, key) = hook_at(host, i, client, req, state, module_table);

        let cached: Option<HookResult> = if let Some(k) = &key {
            hooks.borrow().get(host).unwrap().borrow()[i].cached_result.get(k.as_ref()).and_then(|c| c.get().cloned())
        } else {
            None
        };
        let resp: Option<HookResult> = if cached.is_some() {
            cached
        } else {
            let f = module.hook_handler.borrow().as_ref().cloned();
            if let Some(f) = f {
                let h = module.pool.checkout();
                h.set_client(client.clone());
                h.set_req(req.clone());

                // yields here if the module awaits (e.g. conduit.sleep), letting
                // other requests on this thread run. `h` is held across the await,
                // so this request keeps its own pool entry while suspended.
                let r = f.call_async::<HookResult>((&h.client_ud, &h.req_ud)).await.unwrap();

                if let (Some(t), Some(k)) = (cache_ttl, key) {
                    let map = hooks.borrow();
                    let mut hv = map.get(host).unwrap().borrow_mut();
                    hv[i].cached_result.insert(k.into_owned(), CachedValue::new(r.clone(), t));
                }
                Some(r)
            } else {
                None
            }
        };
        if let Some(HookResult::ContinueWithCtx(update_ctx)) = resp {
            for (key, val) in update_ctx.updates {
                if let Ok(v) = transfer_value_between_states(val, &state.lua) {
                    context.insert(key, v);
                }
            }
        }
    }
}

fn hook_at<'a>(
    host: &str,
    idx: usize,
    client: &'a Client,
    req: &'a Request,
    state: &LuaState,
    module_table: &ProxyModuleTable
) -> (Rc<ProxyModule>, Option<Duration>, Option<Cow<'a, str>>) {
    let map = state.hooks.borrow();
    let hv = map.get(host).unwrap().borrow();
    let h = &hv[idx];

    let (cache_ttl, key) = if let Some(ref c) = h.cacheability {
        (
            Some(c.ttl),
            Some(match &c.key {
                HookCacheKeyKind::Url => Cow::Borrowed(req.0.path()),
                HookCacheKeyKind::Header(hh) => Cow::Borrowed(req.0.header(hh).unwrap_or("")),
                HookCacheKeyKind::Ip => Cow::Owned(client.0.ip.to_string()),
                HookCacheKeyKind::Custom(c) => Cow::Owned(c.clone()),
            })
        )
    } else {
        (None, None)
    };

    let module = module_table.modules.get(h.name.as_str()).unwrap().clone();
    (module, cache_ttl, key)
}

pub async fn resolve_backend(
    host: &str,
    client: Client,
    req: Request,
    context: Context,
    state: &LuaState,
    module_table: &ProxyModuleTable,
) -> io::Result<(String, Option<Vec<(String, String)>>)> {
    run_hooks(host, &client, &req, context.clone(), state, module_table).await;

    let handler_fn = state.req_handlers.borrow();
    let lua_fn = handler_fn.get(host).cloned().unwrap();

    let call_lua_fn = || {
        let h = state.pool.checkout();
        h.set_client(client.clone());
        h.set_req(req.clone());
        h.set_ctx(context.clone());

        lua_fn
            .call::<OnRequestResp>((&h.client_ud, &h.req_ud, &h.ctx_ud))
            .unwrap()
    };

    let handle_resp = |resp: OnRequestResp| {
        if let OnRequestResp::OnRequestExpectHttpBodyResp(b) = resp {
            Ok((b.upstream, b.headers_patch.map(|p| p.add_headers)))
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidData, "unexpected response type"))
        }
    };

    state.lua.set_hook(
        LuaHookTriggers {
            every_line: false,
            on_calls: false,
            on_returns: false,
            // Check every 1000 VM instructions
            every_nth_instruction: Some(1000),
        },
        move |_, _| {
            // error after 1000 VM instructions
            Err(mlua::Error::RuntimeError("timeout exceeded".to_string()))
            // or we can check elapsed time and if not timeout, `Ok(LuaVmState::Continue)`
        },
    ).unwrap();

    let resp = call_lua_fn();
    state.lua.remove_hook();
    handle_resp(resp)
}
