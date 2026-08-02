use mlua::prelude::*;
use tealr::mlu::{FromLuaExact, FromToLua, TealData, TealDataMethods, TypedFunction};
use toml::de;
use std::{cell::RefCell, fmt::Debug};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tealr::{ToTypename, create_union_mlua};

use crate::lua_module::HookResult;

pub struct ClientInner {
    pub ip: String,
    pub port: u16
}

#[derive(Clone, tealr::mlu::UserData, ToTypename)]
pub struct Client(pub Arc<ClientInner>);

impl Client {
    pub fn new(ip: impl Into<String>, port: u16) -> Self {
        Self(Arc::new(ClientInner{
            ip: ip.into(),
            port
        }))
    }
}

fn ip_location(ip: impl Into<String>) -> String {
    "XX".to_string()
}

impl tealr::mlu::TealData for Client {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("ip", |lua, this| lua.create_string(this.0.ip.as_str()));
        fields.add_field_method_get("port", |_, this| Ok(this.0.port));
        fields.add_field_function_get("geo", |lua, this| lua.create_string(ip_location(&this.borrow::<Client>().unwrap().0.ip).as_str()));
    }
}

#[derive(Debug, Clone)]
pub struct OnAcceptTlsBlock {
    pub enabled: bool,
    pub keyfile: Box<str>,
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct OnAcceptExpectHttpResp {
    pub max_header_size: usize,
    pub max_header_count: usize,
    pub expected_method: Box<str>,
    pub header_timeout: u64,
    pub tls: OnAcceptTlsBlock,
}

impl FromLuaExact for OnAcceptExpectHttpResp {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                let resp: OnAcceptExpectHttpResp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnAcceptExpectHttpResp".to_string(),
                message: Some("Expected a userdata with 'max_header_size', 'max_header_count', 'expected_method', 'header_timeout', and 'tls' fields".to_string()),
            }),
        }
    }
}

impl tealr::mlu::TealData for OnAcceptExpectHttpResp {
    fn add_methods<F: tealr::mlu::TealDataMethods<Self>>(fields: &mut F) {
        fields.add_method_mut(
            "set_max_header_size",
            |_, this, size: usize| {
                this.max_header_size = size;
                Ok(())
            },
        );
        fields.add_method_mut(
            "set_max_header_count",
            |_, this, count: usize| {
                this.max_header_count = count;
                Ok(())
            },
        );
        fields.add_method_mut(
            "set_header_timeout",
            |_, this, timeout: u64| {
                this.header_timeout = timeout;
                Ok(())
            },
        );
        fields.add_method_mut(
            "set_expected_method",
            |_, this, method: String| {
                this.expected_method = method.into();
                Ok(())
            },
        );
        fields.add_method_mut(
            "set_tls",
            |_, this, tls_keyfile: String| {
                this.tls = OnAcceptTlsBlock { enabled: true, keyfile: tls_keyfile.into() };
                Ok(())
            },
        );
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct OnAcceptDenyResp {
    pub reason: Box<str>,
}

impl TealData for OnAcceptDenyResp {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("reason", |lua, this| lua.create_string(this.reason.to_string()));
    }
}


impl FromLuaExact for OnAcceptDenyResp {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                let resp: OnAcceptDenyResp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnAcceptDenyResp".to_string(),
                message: Some("Expected a userdata with 'reason' field".to_string()),
            }),
        }
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct OnAcceptRedirectResp {
    pub status: u16,
    pub location: Box<str>,
}

impl tealr::mlu::TealData for OnAcceptRedirectResp {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |lua, this| Ok(this.status));
        fields.add_field_method_get("location", |lua, this| lua.create_string(this.location.to_string()));
    }
}

impl FromLuaExact for OnAcceptRedirectResp {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                let resp: OnAcceptRedirectResp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnAcceptRedirectResp".to_string(),
                message: Some("Expected a userdata with 'status' and 'location' fields".to_string()),
            }),
        }
    }
}

create_union_mlua!(pub enum OnAcceptResp = OnAcceptExpectHttpResp | OnAcceptDenyResp | OnAcceptRedirectResp);


impl std::fmt::Debug for OnAcceptResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnAcceptResp::OnAcceptExpectHttpResp(x) => f.debug_tuple("OnAcceptExpectHttpResp").field(x).finish(),
            OnAcceptResp::OnAcceptDenyResp(x) => f.debug_tuple("OnAcceptDenyResp").field(x).finish(),
            OnAcceptResp::OnAcceptRedirectResp(x) => f.debug_tuple("OnAcceptRedirectResp").field(x).finish(),
        }
    }
}

tealr::create_generic_mlua!(X);

#[derive(Debug)]
pub struct ContextInner {
    pub ctx: HashMap<Box<str>, X>
}

#[derive(Clone, Debug, tealr::mlu::UserData, ToTypename)]
pub struct Context(Arc<RefCell<ContextInner>>);

impl Context {
    pub fn new() -> Self {
        Self(Arc::new(RefCell::new(
            ContextInner{
                ctx: HashMap::new()
            }
        ))
        )
    }

    pub fn insert(&mut self, k: Box<str>, v: LuaValue) {
        self.0.borrow_mut().ctx.insert(k, v.into());
    }
}

impl tealr::mlu::TealData for Context {
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |_, this, key: String| {
            if let Some(v) = this.0.borrow().ctx.get::<Box<str>>(&key.into()) {
                Ok(Some(v.clone()))
            } else {
                Ok(None)
            }
        });
    }
}


#[derive(Debug)]
pub struct RequestInner {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>
}

#[derive(Clone, Debug, tealr::mlu::UserData, ToTypename)]
pub struct Request(pub Arc<RequestInner>);

impl Request {
    pub fn new(method: impl Into<String>, path: impl Into<String>, headers: HashMap<String, String>) -> Self {
        Self(Arc::new(RequestInner{
            method: method.into(),
            path: path.into(),
            headers
        }))
    }
}

impl tealr::mlu::TealData for Request {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("method", |lua, this| lua.create_string(&this.0.method));
        fields.add_field_method_get("path", |lua, this| lua.create_string(&this.0.path));
        fields.add_field_method_get("headers", |lua, this| {
            let table = lua.create_table()?;
            for (k, v) in this.0.headers.iter() {
                table.set(k.clone(), v.clone())?;
            }
            Ok(table)
        });
    }
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method("header", |lua, this, h: String| {
            if let Some(v) = this.0.headers.get(&h) {
                Ok(Some(v.to_string()))
            } else {
                Ok(None)
            }
        });
    }
}

#[derive(Debug, Clone)]
pub struct RespCacheBlock {
    pub enabled: bool,
    pub ttl: u32,
    pub key: Box<str>,
}

#[derive(Debug, Clone)]
pub struct HeadersPatchBlock {
    pub add_headers: HashMap<Box<str>, Box<str>>,
    pub remove_headers: Vec<Box<str>>,
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct OnRequestExpectHttpBodyResp {
    pub upstream: Box<str>,
    pub max_body_size: usize,
    pub proxy_read_timeout: Option<u32>,
    pub client_body_timeout: Option<u32>,
    // bla bla bla
    pub cache: Option<RespCacheBlock>,
    pub headers_patch: Option<HeadersPatchBlock>,
    // query patch, cookies patch, rewrite_uri ...
}

impl FromLuaExact for OnRequestExpectHttpBodyResp {
    fn from_lua_exact(lua_value: LuaValue, _: &Lua) -> mlua::Result<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                let resp: OnRequestExpectHttpBodyResp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnAcceptExpectHttpResp".to_string(),
                message: Some("Expected a userdata".to_string()),
            }),
        }
    }
}
impl tealr::mlu::TealData for OnRequestExpectHttpBodyResp {
    fn add_methods<F: TealDataMethods<Self>>(methods: &mut F) {
        methods.add_method_mut("set_max_body_size", |_, this, size: usize| {
            this.max_body_size = size;
            Ok(())
        });
        methods.add_method_mut("set_proxy_read_timeout", |_, this, timeout: Option<u32>| {
            this.proxy_read_timeout = timeout;
            Ok(())
        });
        methods.add_method_mut("set_client_body_timeout", |_, this, timeout: Option<u32>| {
            this.client_body_timeout = timeout;
            Ok(())
        });

        methods.add_method_mut("set_cache", |_, this, (ttl, key): (u32, String)| {
            this.cache = Some(RespCacheBlock {
                enabled: true,
                ttl,
                key: key.into(),
            });
            Ok(())
        });

        methods.add_method_mut("add_header", |_, this, (key, val): (String, String)| {
            this.headers_patch.get_or_insert(
                HeadersPatchBlock { add_headers: HashMap::new(), remove_headers: Vec::new() }
            ).add_headers.insert(key.into(), val.into());
            Ok(())
        });

        methods.add_method_mut("remove_header", |_, this, key: String| {
            this.headers_patch.get_or_insert(
                HeadersPatchBlock { add_headers: HashMap::new(), remove_headers: Vec::new() }
            ).remove_headers.push(key.into());
            Ok(())
        })
    }
}

create_union_mlua!(pub enum OnRequestResp = OnRequestExpectHttpBodyResp | OnAcceptDenyResp | OnAcceptRedirectResp);

impl std::fmt::Debug for OnRequestResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnRequestResp::OnRequestExpectHttpBodyResp(x) => {
                f.debug_tuple("OnRequestExpectHttpBodyResp").field(x).finish()
            }
            OnRequestResp::OnAcceptDenyResp(x) => {
                f.debug_tuple("OnAcceptDenyResp").field(x).finish()
            }
            OnRequestResp::OnAcceptRedirectResp(x) => {
                f.debug_tuple("OnAcceptRedirectResp").field(x).finish()
            }
        }
    }
}

pub type OnAcceptFn = TypedFunction<Client, OnAcceptResp>;
pub type OnRequestFn = TypedFunction<(Client, Request, Context), OnRequestResp>;

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

#[derive(Debug)]
pub enum HookCacheKeyKind {
    Url,
    Ip,
    Header(String),
    Custom(String),
}

#[derive(Debug)]
pub struct Hook {
    pub name: String,
    pub cache_ttl: Option<Duration>,
    pub cache_key_kind: Option<HookCacheKeyKind>,
    pub cached_result: HashMap<String, CachedValue<HookResult>>,
}

#[derive(Debug, tealr::mlu::UserData, ToTypename)]
pub struct Proxy {
    pub on_accept_fn: Rc<RefCell<Option<LuaFunction>>>,
    pub req_handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>>,
    pub hooks: Rc<RefCell<HashMap<Box<str>, RefCell<Vec<Hook>>>>>,
    pub listen_addr: Box<str>
}

impl TealData for Proxy {
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("listen_on", |_, this, addr: String| {
            this.listen_addr = addr.into();
            Ok(())
        });

        // will introduce a new cache type
        methods.add_method_mut("register_hook", |_, this, (host, module_name, cache_ttl, cache_key): (String, String, usize, Option<String>)| {
            let key: Box<str> = host.into();
            if !this.hooks.borrow().contains_key(&key) {
                this.hooks.borrow_mut().insert(key.clone(), RefCell::new(Vec::new()));
            }
            let cache_ttl = if cache_ttl == 0 {
                None
            } else {
                Some(Duration::from_secs(cache_ttl as u64))
            };

            let cache_key_kind = match cache_key {
                Some(k) => {
                    if k == "$url" {
                        Some(HookCacheKeyKind::Url)
                    } else if k == "$ip" {
                        Some(HookCacheKeyKind::Ip)
                    } else if k.starts_with("$header:") {
                        let header_name = k.trim_start_matches("$header:").to_string();
                        Some(HookCacheKeyKind::Header(header_name))
                    } else {
                        Some(HookCacheKeyKind::Custom(k))
                    }
                }
                None => None,
            };

            let hook = Hook {
                name: module_name,
                cache_ttl,
                cache_key_kind,
                cached_result: HashMap::new(),
            };
            this.hooks.borrow().get(&key).unwrap().borrow_mut().insert(0, hook);
            Ok(())
        });

        methods.add_method_mut("on_request", |_, this, (host, f): (String, OnRequestFn)| {
            this.req_handlers.borrow_mut().insert(host.into(), f.into());
            Ok(())
        });

        //methods.add_method_mut("on_accept", |_, this, f: LuaFunction| {
        //    this.on_accept_fn.replace(Some(f));
        //    Ok(())
        //});
        methods.add_method_mut("on_accept", |_, this, f: OnAcceptFn| {
            this.on_accept_fn.replace(Some(f.into()));
            Ok(())
        });

        methods.add_method("resp_expect_httpbody", |lua, _, upstream: String| {
            let resp = OnRequestExpectHttpBodyResp {
                upstream: upstream.into(),
                max_body_size: 1048576,
                proxy_read_timeout: Some(30),
                client_body_timeout: Some(30),
                cache: None,
                headers_patch: None,
            };
            Ok(resp)
            //lua.create_userdata(resp)
        });

        methods.add_method("resp_expect_http", |lua, _, _: ()| {
            let resp = OnAcceptExpectHttpResp {
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

        methods.add_method("resp_deny", |lua, _, reason: String| {
            let resp = OnAcceptDenyResp { reason: reason.into() };
            Ok(resp)
        });

        methods.add_method("resp_redirect", |lua, _, (status, location): (u16, String)| {
            let resp = OnAcceptRedirectResp { status, location: location.into() };
            Ok(resp)
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

use super::*;

    #[test]
    fn register_handler() -> LuaResult<()> {
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert("Host".into(), "example.com".into());
        headers.insert("ContentLength".into(), "64".into());

        let req = Request::new("GET", "path", headers);

        let req_handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>> = Rc::new(RefCell::new(HashMap::new()));
        let on_accept_fn: Rc<RefCell<Option<LuaFunction>>> = Rc::new(RefCell::new(None));
        let hooks: Rc<RefCell<HashMap<Box<str>, RefCell<Vec<Hook>>>>> = Rc::new(RefCell::new(HashMap::new()));

        let proxy = Proxy{
            req_handlers: req_handlers.clone(),
            on_accept_fn: on_accept_fn.clone(),
            listen_addr: "".into(),
            hooks
        };
        let lua = Lua::new();

        let proxy_ud = lua.create_userdata(proxy).unwrap();
        lua.globals().set("proxy", proxy_ud)?;

        let source = lua.load(
            r#"


            proxy:on_request("example.com", function(client, req, context)
                print("Request from", client.ip, client.port)
                print(req.method, req.path)
                resp = proxy:resp_expect_httpbody("127.0.0.1:8000")
                resp:set_cache(60, "$url")
                return resp
            end)

            proxy:on_accept(function(client)
                print("Accepted connection from", client.ip, client.port)

                if client.ip == "10.100.1.1" then
                    local resp = proxy:resp_expect_http()
                    resp:set_max_header_size(16384)
                    resp:set_max_header_count(128)
                    resp:set_expected_method("GET")
                    resp:set_tls("/path/to/keyfile")
                    return resp

                elseif client.ip == "127.0.0.1" then
                    return proxy:resp_redirect(201, "https://example.com")

                else
                    return proxy:resp_deny("NOPE")
                end
            end)
        "#,
        );
        source.exec()?;


        let client1 = Client::new("127.0.0.1", 6969);
        let client2 = Client::new("10.100.1.1", 6969);
        let client3 = Client::new("192.168.1.1", 6969);
        lua.scope(|scope| {
            let client1 = scope.create_userdata(client1.clone());
            let client2 = scope.create_userdata(client2);
            let client3 = scope.create_userdata(client3);

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client1)?;
            println!("{:?}", resp);

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client2)?;
            println!("{:?}", resp);

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client3)?;
            println!("{:?}", resp);
            Ok(())
        })?;

        println!();

        let ctx = Context::new();
        lua.scope(|scope| {
            let req = scope.create_userdata(req)?;
            let client = scope.create_userdata(client1)?;
            let context = scope.create_userdata(ctx)?;

            let resp = req_handlers
                .borrow()
                .get("example.com")
                .unwrap()
                .call::<OnRequestResp>((client, req, context));
            println!("{:?}", resp);
            Ok(())
        })?;
        Ok(())
    }
}
