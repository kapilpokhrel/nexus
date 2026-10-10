use mlua::prelude::*;
use std::collections::HashMap;
use std::net::IpAddr;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{cell::Cell, cell::RefCell, fmt::Debug};
use tealr::mlu::{FromLuaExact, TealData, TealDataMethods, TypedFunction};
use tealr::{ToTypename, create_union_mlua};

#[derive(Clone)]
pub struct OnRequestParamSlot {
    pub client: Option<Client>,
    pub req: Option<Request>,
}

macro_rules! teal_name {
    ($t:ty, $n:literal) => {
        impl ToTypename for $t {
            fn to_typename() -> tealr::Type {
                tealr::Type::new_single($n, tealr::KindOfType::External)
            }
        }
    };
}

pub struct OnRequestHandleUDs {
    slot: Rc<RefCell<OnRequestParamSlot>>,
    pub client_ud: LuaAnyUserData,
    pub req_ud: LuaAnyUserData,
}

impl OnRequestHandleUDs {
    pub fn new(lua: &Lua, slot: Rc<RefCell<OnRequestParamSlot>>) -> Self {
        let client_ud = lua.create_userdata(ClientHandle(slot.clone())).unwrap();
        let req_ud = lua.create_userdata(RequestHandle(slot.clone())).unwrap();
        Self {
            slot,
            client_ud,
            req_ud,
        }
    }

    pub fn set_client(&self, client: Client) {
        self.slot.borrow_mut().client = Some(client);
    }

    pub fn set_req(&self, req: Request) {
        self.slot.borrow_mut().req = Some(req);
    }

    pub fn clear(&self) {
        self.slot.borrow_mut().client = None;
        self.slot.borrow_mut().req = None;
    }
}

pub struct HandlePool {
    lua: Lua,
    pool: Vec<Rc<OnRequestHandleUDs>>,
}

impl HandlePool {
    pub fn new(lua: Lua, pool_size: usize) -> Self {
        let mut pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let slot = Rc::new(RefCell::new(OnRequestParamSlot {
                client: None,
                req: None,
            }));
            pool.push(Rc::new(OnRequestHandleUDs::new(&lua, slot)));
        }
        Self { lua, pool }
    }

    pub fn checkout(&self) -> Rc<OnRequestHandleUDs> {
        for uds in &self.pool {
            if Rc::strong_count(uds) == 1 {
                uds.clear();
                return uds.clone();
            }
        }
        //no free entry: this request gets its own, which is dropped rather than pooled
        let slot = Rc::new(RefCell::new(OnRequestParamSlot {
            client: None,
            req: None,
        }));
        Rc::new(OnRequestHandleUDs::new(&self.lua, slot))
    }
}

pub struct ClientInner {
    pub ip: IpAddr,
    pub port: u16,
}

#[derive(Clone)]
pub struct Client(pub Arc<ClientInner>);

impl Client {
    pub fn new(ip: IpAddr, port: u16) -> Self {
        Self(Arc::new(ClientInner { ip, port }))
    }
}

#[derive(tealr::mlu::UserData)]
pub struct ClientHandle(pub Rc<RefCell<OnRequestParamSlot>>);
teal_name!(ClientHandle, "Client");

impl tealr::mlu::TealData for ClientHandle {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("ip", |lua, this| {
            lua.create_string(this.0.borrow().client.as_ref().unwrap().0.ip.to_string())
        });
        fields.add_field_method_get("port", |_, this| {
            Ok(this.0.borrow().client.as_ref().unwrap().0.port)
        });
    }
}

#[derive(Debug, Clone)]
pub struct OnAcceptTlsBlock {
    pub enabled: bool,
    pub keyfile: Box<str>,
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct ExpectHttp {
    pub max_header_size: usize,
    pub max_header_count: usize,
    pub expected_method: Box<str>,
    pub header_timeout: u64,
    pub tls: OnAcceptTlsBlock,
}

impl FromLuaExact for ExpectHttp {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => {
                let resp: ExpectHttp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "ExpectHttp".to_string(),
                message: Some("Expected a userdata with 'max_header_size', 'max_header_count', 'expected_method', 'header_timeout', and 'tls' fields".to_string()),
            }),
        }
    }
}

impl tealr::mlu::TealData for ExpectHttp {
    fn add_methods<F: tealr::mlu::TealDataMethods<Self>>(fields: &mut F) {
        fields.add_method_mut("set_max_header_size", |_, this, size: usize| {
            this.max_header_size = size;
            Ok(())
        });
        fields.add_method_mut("set_max_header_count", |_, this, count: usize| {
            this.max_header_count = count;
            Ok(())
        });
        fields.add_method_mut("set_header_timeout", |_, this, timeout: u64| {
            this.header_timeout = timeout;
            Ok(())
        });
        fields.add_method_mut("set_expected_method", |_, this, method: String| {
            this.expected_method = method.into();
            Ok(())
        });
        fields.add_method_mut("set_tls", |_, this, tls_keyfile: String| {
            this.tls = OnAcceptTlsBlock {
                enabled: true,
                keyfile: tls_keyfile.into(),
            };
            Ok(())
        });
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct Deny {
    pub reason: Box<str>,
}

impl TealData for Deny {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("reason", |lua, this| {
            lua.create_string(this.reason.to_string())
        });
    }
}

impl FromLuaExact for Deny {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => {
                let resp: Deny = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "Deny".to_string(),
                message: Some("Expected a userdata with 'reason' field".to_string()),
            }),
        }
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct Redirect {
    pub status: u16,
    pub location: Box<str>,
}

impl tealr::mlu::TealData for Redirect {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |_lua, this| Ok(this.status));
        fields.add_field_method_get("location", |lua, this| {
            lua.create_string(this.location.to_string())
        });
    }
}

impl FromLuaExact for Redirect {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => {
                let resp: Redirect = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "Redirect".to_string(),
                message: Some(
                    "Expected a userdata with 'status' and 'location' fields".to_string(),
                ),
            }),
        }
    }
}

create_union_mlua!(pub enum OnAcceptHandlerResp = Deny | Redirect | ExpectHttp);

impl std::fmt::Debug for OnAcceptHandlerResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnAcceptHandlerResp::ExpectHttp(x) => {
                f.debug_tuple("ExpectHttp").field(x).finish()
            }
            OnAcceptHandlerResp::Deny(x) => {
                f.debug_tuple("Deny").field(x).finish()
            }
            OnAcceptHandlerResp::Redirect(x) => {
                f.debug_tuple("Redirect").field(x).finish()
            }
        }
    }
}

tealr::create_generic_mlua!(X);

#[derive(Debug)]
pub struct RequestInner {
    head: Box<[u8]>,
    method: Range<usize>,
    path: Range<usize>,
    headers: Vec<(Range<usize>, Range<usize>)>,
}

impl RequestInner {
    #[inline]
    fn at(&self, r: &Range<usize>) -> &str {
        unsafe { std::str::from_utf8_unchecked(&self.head[r.clone()]) }
    }

    pub fn method(&self) -> &str {
        self.at(&self.method)
    }

    pub fn path(&self) -> &str {
        self.at(&self.path)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| self.at(n) == name)
            .map(|(_, v)| self.at(v))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(n, v)| (self.at(n), self.at(v)))
    }
}

#[derive(Clone, Debug)]
pub struct Request(pub Arc<RequestInner>);

impl Request {
    /// `head` is the request header directly read from the socket; the ranges are
    /// offsets into it
    pub fn new(
        head: &[u8],
        method: Range<usize>,
        path: Range<usize>,
        headers: Vec<(Range<usize>, Range<usize>)>,
    ) -> Self {
        Self(Arc::new(RequestInner {
            head: head.into(),
            method,
            path,
            headers,
        }))
    }

    /// For tests
    pub fn from_parts(method: &str, path: &str, headers: &[(&str, &str)]) -> Self {
        let mut head = Vec::new();
        head.extend_from_slice(method.as_bytes());
        let m = 0..head.len();
        head.push(b' ');
        let p_start = head.len();
        head.extend_from_slice(path.as_bytes());
        let p = p_start..head.len();
        head.extend_from_slice(b" HTTP/1.1\r\n");
        let mut hs = Vec::with_capacity(headers.len());
        for (n, v) in headers {
            let n_start = head.len();
            head.extend_from_slice(n.as_bytes());
            let nr = n_start..head.len();
            head.extend_from_slice(b": ");
            let v_start = head.len();
            head.extend_from_slice(v.as_bytes());
            hs.push((nr, v_start..head.len()));
            head.extend_from_slice(b"\r\n");
        }
        Request::new(&head, m, p, hs)
    }
}

#[derive(tealr::mlu::UserData)]
pub struct RequestHandle(pub Rc<RefCell<OnRequestParamSlot>>);
teal_name!(RequestHandle, "Request");

impl tealr::mlu::TealData for RequestHandle {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("method", |lua, this| {
            lua.create_string(this.0.borrow().req.as_ref().unwrap().0.method())
        });
        fields.add_field_method_get("path", |lua, this| {
            lua.create_string(this.0.borrow().req.as_ref().unwrap().0.path())
        });
        fields.add_field_method_get("headers", |lua, this| {
            let table = lua.create_table()?;
            for (k, v) in this.0.borrow().req.as_ref().unwrap().0.iter() {
                table.set(k, v)?;
            }
            Ok(table)
        });
    }
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method("header", |_, this, h: String| {
            Ok(this
                .0
                .borrow()
                .req
                .as_ref()
                .unwrap()
                .0
                .header(&h)
                .map(str::to_string))
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
    pub add_headers: Vec<(String, String)>,
    pub remove_headers: Vec<String>,
}

impl HeadersPatchBlock {
    fn empty() -> Self {
        HeadersPatchBlock {
            add_headers: Vec::new(),
            remove_headers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct ForwardHttp {
    pub upstream: String,
    pub max_body_size: usize,
    pub proxy_read_timeout: Option<u32>,
    pub client_body_timeout: Option<u32>,
    // bla bla bla
    pub cache: Option<RespCacheBlock>,
    pub headers_patch: Option<HeadersPatchBlock>,

    // do we honour client keep-alive or not? if false, we close the connection after the response
    pub keep_alive: bool,
    // query patch, cookies patch, rewrite_uri ...
}

impl FromLuaExact for ForwardHttp {
    fn from_lua_exact(lua_value: LuaValue, _: &Lua) -> mlua::Result<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => {
                let resp: ForwardHttp = ud.take::<Self>()?;
                Ok(resp)
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "ForwardHttp".to_string(),
                message: Some("Expected a userdata".to_string()),
            }),
        }
    }
}
impl tealr::mlu::TealData for ForwardHttp {
    fn add_methods<F: TealDataMethods<Self>>(methods: &mut F) {
        methods.add_method_mut("set_max_body_size", |_, this, size: usize| {
            this.max_body_size = size;
            Ok(())
        });
        methods.add_method_mut("set_proxy_read_timeout", |_, this, timeout: Option<u32>| {
            this.proxy_read_timeout = timeout;
            Ok(())
        });
        methods.add_method_mut(
            "set_client_body_timeout",
            |_, this, timeout: Option<u32>| {
                this.client_body_timeout = timeout;
                Ok(())
            },
        );

        methods.add_method_mut("set_cache", |_, this, (ttl, key): (u32, String)| {
            this.cache = Some(RespCacheBlock {
                enabled: true,
                ttl,
                key: key.into(),
            });
            Ok(())
        });

        methods.add_method_mut("add_header", |_, this, (key, val): (String, String)| {
            let p = this
                .headers_patch
                .get_or_insert_with(HeadersPatchBlock::empty);
            match p.add_headers.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = val,
                None => p.add_headers.push((key, val)),
            }
            Ok(())
        });

        methods.add_method_mut("remove_header", |_, this, key: String| {
            this.headers_patch
                .get_or_insert_with(HeadersPatchBlock::empty)
                .remove_headers
                .push(key);
            Ok(())
        });

        methods.add_method_mut("keep_alive", |_, this, val: bool| {
            this.keep_alive = val;
            Ok(())
        })
    }
}

create_union_mlua!(pub enum OnRequestHandlerResp = ForwardHttp | Deny | Redirect);

impl std::fmt::Debug for OnRequestHandlerResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnRequestHandlerResp::ForwardHttp(x) => f
                .debug_tuple("ForwardHttp")
                .field(x)
                .finish(),
            OnRequestHandlerResp::Deny(x) => {
                f.debug_tuple("Deny").field(x).finish()
            }
            OnRequestHandlerResp::Redirect(x) => {
                f.debug_tuple("Redirect").field(x).finish()
            }
        }
    }
}

pub type OnAcceptFn = TypedFunction<ClientHandle, OnAcceptHandlerResp>;
pub type OnRequestFn = TypedFunction<(ClientHandle, RequestHandle), OnRequestHandlerResp>;

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
            cached_results: HashMap::new()
        }
    }
}

impl TealData for Proxy {
    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("listen_on", |_, this, addr: String| {
            this.listen_addr = addr.into();
            Ok(())
        });

        methods.add_method_mut("cached_call", |_, this, (host, cache_info, func, args): (String, CacheParam, LuaFunction, LuaVariadic<LuaValue>)| {
            let key = host.clone() + &cache_info.key;
            let cached =  this.cached_results.get(key.as_str());
            if let Some(cached) = cached && let Some(val) = cached.get() {
                return val.clone();
            }
            let retval = func.call::<LuaValue>(args);
            this.cached_results.insert(cache_info.key.clone(), CachedValue::new(retval.clone(), cache_info.ttl));
            retval
        });

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

        methods.add_method(
            "redirect",
            |_lua, _, (status, location): (u16, String)| {
                let resp = Redirect {
                    status,
                    location: location.into(),
                };
                Ok(resp)
            },
        );
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
