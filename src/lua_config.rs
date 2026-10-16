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
    CustomHttpResponse, Deny, ExpectHttp, FileResponse, ForwardHttp, OnAcceptFn,
    OnAcceptHandlerResp, OnRequestFn, OnRequestHandlerResp,
};
pub use handles::{Client, ClientHandle, HandleFactory, Request, RequestHandle};

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
        methods.document_type("Proxy is the main interface for configuring the proxy server. It allows you to set up request handlers, accept handlers, and configure various aspects of the proxy's behavior.");
        methods.document("listen_on(address) sets the address and port on which the proxy server will listen for incoming connections. The address should be in the format 'IP:PORT'.");
        methods.add_method_mut("listen_on", |_, this, addr: String| {
            this.listen_addr = addr.into();
            // other validity checks required too
            if this.listen_addr.is_empty() {
                Err(mlua::Error::RuntimeError(
                    "listen_on: address cannot be empty".into(),
                ))
            } else {
                Ok(())
            }
        });

        methods.document("cached_call(host, cache_info, func, args) calls the given Lua function with the provided arguments and caches the result based on the specified host and cache parameters. If a cached result exists and is still valid, it returns the cached value instead of calling the function again.");
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
                this.cached_results
                    .insert(key.into(), CachedValue::new(retval.clone(), cache_info.ttl));
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

        methods.document("on_request(host, func) registers a request handler function for the specified host. The function will be called whenever a request is received for that host. The function should accept two arguments: a ClientHandle and a RequestHandle, and it should return an appropriate response.");
        methods.add_method_mut("on_request", |_, this, (host, f): (String, OnRequestFn)| {
            // other validity checks can be added here, like valid host format, etc.
            if host.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "on_reqeust: host address cannot be empty".into(),
                ));
            }
            this.req_handlers.borrow_mut().insert(host.into(), f.into());
            Ok(())
        });

        methods.document("on_accept(func) registers an accept handler function that will be called whenever a new client connection is accepted. The function should accept a ClientHandle and return an appropriate response.");
        methods.add_method_mut("on_accept", |_, this, f: OnAcceptFn| {
            this.on_accept_fn.replace(Some(f.into()));
            Ok(())
        });

        methods.document("forward_http(upstream) creates a ForwardHttp response that forwards the request to the specified upstream server. The upstream parameter should be a valid address in the format 'IP:PORT'.");
        methods.add_method("forward_http", |_lua, _, upstream: String| {
            // same as previous; other validity checks required
            if upstream.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "forward_http: upstream address cannot be empty".into(),
                ));
            }
            let resp = ForwardHttp {
                upstream,
                max_request_body_size: 1048576,
                max_response_body_size: 1048576,
                max_response_header_size: 8192,
                max_response_header_count: 64,
                connect_timeout: Some(5),
                proxy_read_timeout: Some(30),
                client_body_timeout: Some(30),
                cache: None,
                headers_patch: None,
                keep_alive: true,
            };
            Ok(resp)
            //lua.create_userdata(resp)
        });

        methods.document("expect_http() creates an ExpectHttp response for on_accept that expects an HTTP request from the client. This can be used to enforce that the client sends a valid HTTP request before proceeding with further processing.");
        methods.add_method("expect_http", |_lua, _, _: ()| Ok(ExpectHttp::default()));

        methods.document("deny(reason) creates a Deny response that denies the connection with the specified reason. The reason should be a string explaining why the connection is denied. Rignt now it just closes the connection silently");
        methods.add_method("deny", |_lua, _, reason: String| {
            let resp = Deny {
                reason: reason.into(),
            };
            Ok(resp)
        });

        methods.add_method("custom_http_response", |_lua, _, ()| {
            Ok(CustomHttpResponse {
                status: 200,
                reason: "OK".to_string(),
                headers: Vec::new(),
                body: String::new(),
            })
        });

        methods.add_method("file_response", |_lua, _, path: String| {
            if path.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "file_response: path cannot be empty".into(),
                ));
            }
            Ok(FileResponse { path })
        });
    }
}

pub struct LuaConf {
    pub lua: Lua,
    pub req_handlers: HandlerMap,
    pub on_accept_fn: OnAcceptFnSlot,
    pub handles: HandleFactory,
}

impl LuaConf {
    pub fn new() -> Result<Self, LuaError> {
        let req_handlers: HandlerMap = Rc::new(RefCell::new(HashMap::new()));
        let on_accept_fn: OnAcceptFnSlot = Rc::new(RefCell::new(None));

        let proxy = Proxy::new(on_accept_fn.clone(), req_handlers.clone(), "".into());
        let lua = Lua::new();

        lua.gc_gen(20, 100);

        let proxy_ud = lua.create_userdata(proxy)?;
        lua.globals().set("proxy", proxy_ud)?;

        let handles = HandleFactory::new(lua.clone());

        Ok(LuaConf {
            lua,
            req_handlers,
            on_accept_fn,
            handles,
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
            "/path?item=one&item=two",
            &[("Host", "example.com"), ("ContentLength", "64")],
        );

        let lua_state = LuaConf::new()?;

        let source = lua_state.lua.load(
            r#"


            proxy:on_request("example.com", function(client, req)
                print("Request from", client.ip, client.port)
                print(req.method, req.path)
                if req.path == "/path" then
                    assert(req.query == "item=one&item=two")
                end
                if req.path == "/file" then
                    assert(req.query == nil)
                    return proxy:file_response("README.md")
                end
                if client.ip == "127.0.0.1" then
                    local response = proxy:custom_http_response()
                    response:set_status(201)
                    response:set_reason("Created")
                    response:set_body("Created")
                    response:add_header("Content-Type", "text/plain")
                    assert(not pcall(function()
                        response:add_header("X-Test", "ok\r\nInjected: yes")
                    end))
                    assert(not pcall(function()
                        response:add_header("Content-Length", "1")
                    end))
                    assert(not pcall(function()
                        response:set_max_response_header_size(65537)
                    end))

                    local request = proxy:forward_http("127.0.0.1:8000")
                    assert(not pcall(function() request:add_header("Bad Header", "value") end))
                    assert(not pcall(function() request:add_header("X-Test", "bad\r\nvalue") end))
                    return response
                end
                handler_resp = proxy:forward_http("127.0.0.1:8000")
                handler_resp:set_max_request_body_size(1048576)
                handler_resp:set_max_response_body_size(1048576)
                handler_resp:set_max_response_header_size(8192)
                handler_resp:set_max_response_header_count(64)
                handler_resp:set_connect_timeout(5)
                handler_resp:set_cache(60, "$url")
                return handler_resp
            end)

            proxy:on_accept(function(client)
                print("Accepted connection from", client.ip, client.port)
                if client.ip == "10.100.1.1" then
                    local handler_resp = proxy:expect_http()
                    handler_resp:set_max_header_size(16384)
                    handler_resp:set_max_header_count(64)
                    handler_resp:set_tls("/path/to/keyfile")
                    assert(not pcall(function()
                        handler_resp:set_max_header_count(65)
                    end))
                    assert(not pcall(function()
                        handler_resp:set_max_header_size(65537)
                    end))
                    return handler_resp

                elseif client.ip == "192.168.1.1" then
                    local response = proxy:custom_http_response()
                    response:set_status(403)
                    response:set_reason("Forbidden")
                    response:set_body("Forbidden")
                    return response

                else
                    return proxy:deny("NOPE")
                end
            end)

        "#,
        );
        source.exec()?;

        let client1 = Client::new("127.0.0.1".parse()?, 6969);
        let client2 = Client::new("10.100.1.1".parse()?, 6969);
        let client3 = Client::new("192.168.1.1".parse()?, 6969);

        let handle_ud = lua_state.handles.create()?;
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
        assert!(matches!(
            resp,
            Ok(OnRequestHandlerResp::CustomHttpResponse(_))
        ));
        println!("{:?}", resp);

        handle_ud.set_req(Request::from_parts("GET", "/file", &[]));
        let resp = lua_state
            .req_handlers
            .borrow()
            .get("example.com")
            .unwrap()
            .call::<OnRequestHandlerResp>((&handle_ud.client_ud, &handle_ud.req_ud))?;
        assert!(matches!(resp, OnRequestHandlerResp::FileResponse(_)));

        Ok(())
    }
}
