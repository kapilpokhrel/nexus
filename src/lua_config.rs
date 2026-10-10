use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tealr::ToTypename;
use tealr::mlu::{TealData, TealDataMethods};

mod handler_response;
mod handles;

pub use handler_response::{
    Deny, ExpectHttp, ForwardHttp, OnAcceptFn, OnAcceptHandlerResp, OnAcceptTlsBlock, OnRequestFn,
    OnRequestHandlerResp, Redirect
};
pub use handles::{Client, ClientHandle, HandlePool, Request, RequestHandle};

tealr::create_generic_mlua!(X);
#[derive(Debug)]
pub struct CachedValue<T> {
    value: T,
    expiry: Instant,
}

impl<T> CachedValue<T> {
    pub fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expiry: Instant::now() + ttl,
        }
    }

    pub fn get(&self) -> Option<&T> {
        if Instant::now() < self.expiry {
            Some(&self.value)
        } else {
            None
        }
    }
}

#[derive(Debug, tealr::mlu::UserData, ToTypename)]
pub struct CacheParam {
    pub ttl: Duration,
    pub key: Box<str>,
}

impl TealData for CacheParam {}
impl FromLua for CacheParam {
    fn from_lua(lua_value: LuaValue, _: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => {
                let resp: CacheParam = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "CacheParam".to_string(),
                message: Some("Expected a userdata".to_string()),
            }),
        }
    }
}

pub type HandlerMap = Rc<RefCell<HashMap<Box<str>, LuaFunction>>>;
pub type OnAcceptFnSlot = Rc<RefCell<Option<LuaFunction>>>;
#[derive(Debug, tealr::mlu::UserData, ToTypename)]
pub struct Proxy {
    pub on_accept_fn: OnAcceptFnSlot,
    pub req_handlers: HandlerMap,
    pub listen_addr: Box<str>,
    pub cached_results: HashMap<Box<str>, CachedValue<Result<LuaValue, LuaError>>>,
}

impl Proxy {
    fn new(on_accept_fn: OnAcceptFnSlot, req_handlers: HandlerMap, listen_addr: String) -> Self {
        Self {
            on_accept_fn,
            req_handlers,
            listen_addr: listen_addr.into(),
            cached_results: HashMap::new(),
        }
    }
}

impl TealData for Proxy {
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("listen_on", |_, this, addr: String| {
            this.listen_addr = addr.into();
            Ok(())
        });

        methods.add_method_mut(
            "cached_call",
            |_,
             this,
             (host, cache_info, func, args): (
                String,
                CacheParam,
                LuaFunction,
                LuaVariadic<LuaValue>,
            )| {
                let key = host.clone() + &cache_info.key;
                let cached = this.cached_results.get(key.as_str());
                if let Some(cached) = cached
                    && let Some(val) = cached.get()
                {
                    return val.clone();
                }
                let retval = func.call::<LuaValue>(args);
                this.cached_results.insert(
                    key.into(),
                    CachedValue::new(retval.clone(), cache_info.ttl),
                );
                retval
            },
        );

        methods.add_method_mut(
            "cache_param",
            |_, _this, (cache_ttl, cache_key): (u64, String)| {
                Ok(CacheParam {
                    ttl: Duration::from_secs(cache_ttl),
                    key: cache_key.into(),
                })
            },
        );

        methods.add_method_mut("on_request", |_, this, (host, f): (String, OnRequestFn)| {
            this.req_handlers.borrow_mut().insert(host.into(), f.into());
            Ok(())
        });

        methods.add_method_mut("on_accept", |_, this, f: OnAcceptFn| {
            this.on_accept_fn.replace(Some(f.into()));
            Ok(())
        });

        methods.add_method("forward_http", |_lua, _, upstream: String| {
            let resp = ForwardHttp {
                upstream,
                max_body_size: 1048576,
                proxy_read_timeout: Some(30),
                client_body_timeout: Some(30),
                cache: None,
                headers_patch: None,
                keep_alive: true,
            };
            Ok(resp)
            //lua.create_userdata(resp)
        });

        methods.add_method("expect_http", |_lua, _, _: ()| {
            let resp = ExpectHttp {
                max_header_size: 8192,
                max_header_count: 64,
                expected_method: "".into(),
                header_timeout: 5,
                tls: OnAcceptTlsBlock {
                    enabled: false,
                    keyfile: "".into(),
                },
            };
            Ok(resp)
        });

        methods.add_method("deny", |_lua, _, reason: String| {
            let resp = Deny {
                reason: reason.into(),
            };
            Ok(resp)
        });

        methods.add_method("redirect", |_lua, _, (status, location): (u16, String)| {
            let resp = Redirect {
                status,
                location: location.into(),
            };
            Ok(resp)
        });
    }
}

pub struct LuaConf {
    pub lua: Lua,
    pub req_handlers: HandlerMap,
    pub on_accept_fn: OnAcceptFnSlot,
    pub pool: HandlePool,
}

impl LuaConf {
    pub fn new(pool_size: usize) -> Result<Self, LuaError> {
        let req_handlers: HandlerMap = Rc::new(RefCell::new(HashMap::new()));
        let on_accept_fn: OnAcceptFnSlot = Rc::new(RefCell::new(None));

        let proxy = Proxy::new(on_accept_fn.clone(), req_handlers.clone(), "".into());
        let lua = Lua::new();

        // reusing handles means lua allocates so little that the default incremental
        // GC stops keeping up; generational collects it at no measurable cost
        lua.gc_gen(20, 100);

        let proxy_ud = lua.create_userdata(proxy).unwrap();
        lua.globals().set("proxy", proxy_ud)?;

        let handles_pool = HandlePool::new(lua.clone(), pool_size);

        Ok(LuaConf {
            lua,
            req_handlers,
            on_accept_fn,
            pool: handles_pool,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_handler() -> LuaResult<()> {
        let req = Request::from_parts(
            "GET",
            "path",
            &[("Host", "example.com"), ("ContentLength", "64")],
        );

        let lua_state = LuaConf::new(1)?;

        let source = lua_state.lua.load(
            r#"


            proxy:on_request("example.com", function(client, req)
                print("Request from", client.ip, client.port)
                print(req.method, req.path)
                handler_resp = proxy:forward_http("127.0.0.1:8000")
                handler_resp:set_cache(60, "$url")
                return handler_resp
            end)

            proxy:on_accept(function(client)
                print("Accepted connection from", client.ip, client.port)
                if client.ip == "10.100.1.1" then
                    local handler_resp = proxy:expect_http()
                    handler_resp:set_max_header_size(16384)
                    handler_resp:set_max_header_count(128)
                    handler_resp:set_expected_method("GET")
                    handler_resp:set_tls("/path/to/keyfile")
                    return handler_resp

                elseif client.ip == "127.0.0.1" then
                    return proxy:redirect(201, "https://example.com")

                else
                    return proxy:deny("NOPE")
                end
            end)
        "#,
        );
        source.exec()?;

        let client1 = Client::new("127.0.0.1".parse().unwrap(), 6969);
        let client2 = Client::new("10.100.1.1".parse().unwrap(), 6969);
        let client3 = Client::new("192.168.1.1".parse().unwrap(), 6969);

        let handle_ud = lua_state.pool.checkout();
        handle_ud.set_client(client1.clone());
        let resp = lua_state
            .on_accept_fn
            .borrow()
            .as_ref()
            .unwrap()
            .call::<OnAcceptHandlerResp>(&handle_ud.client_ud)?;
        println!("{:?}", resp);

        handle_ud.set_client(client2.clone());
        let resp = lua_state
            .on_accept_fn
            .borrow()
            .as_ref()
            .unwrap()
            .call::<OnAcceptHandlerResp>(&handle_ud.client_ud)?;
        println!("{:?}", resp);

        handle_ud.set_client(client3.clone());
        let resp = lua_state
            .on_accept_fn
            .borrow()
            .as_ref()
            .unwrap()
            .call::<OnAcceptHandlerResp>(&handle_ud.client_ud)?;
        println!("{:?}", resp);

        println!();

        handle_ud.set_client(client1.clone());
        handle_ud.set_req(req.clone());

        let resp = lua_state
            .req_handlers
            .borrow()
            .get("example.com")
            .unwrap()
            .call::<OnRequestHandlerResp>((&handle_ud.client_ud, &handle_ud.req_ud));
        println!("{:?}", resp);

        Ok(())
    }
}
