use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Client {
    pub ip: String,
    pub port: u16
}

fn ip_location(ip: impl Into<String>) -> String {
    "XX".to_string()
}

impl LuaUserData for Client {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("ip", |lua, this| lua.create_string(this.ip.as_str()));
        fields.add_field_method_get("port", |_, this| Ok(this.port));
        fields.add_field_function_get("geo", |lua, this| lua.create_string(ip_location(&this.borrow::<Client>().unwrap().ip).as_str()));
    }
}

#[derive(Debug)]
pub struct OnAcceptTlsBlock {
    pub enabled: bool,
    pub keyfile: Box<str>,
}

#[derive(Debug)]
pub struct OnAcceptExpectHttpResp {
    pub max_header_size: usize,
    pub max_header_count: usize,
    pub expected_method: Box<str>,
    pub header_timeout: u64,
    pub tls: OnAcceptTlsBlock,
}

impl FromLua for OnAcceptExpectHttpResp {
    fn from_lua(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
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

impl LuaUserData for OnAcceptExpectHttpResp {
    fn add_methods<F: LuaUserDataMethods<Self>>(fields: &mut F) {
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
            |_, this, method: Box<str>| {
                this.expected_method = method;
                Ok(())
            },
        );
        fields.add_method_mut(
            "set_tls",
            |_, this, tls_keyfile: Box<str>| {
                this.tls = OnAcceptTlsBlock { enabled: true, keyfile: tls_keyfile };
                Ok(())
            },
        );
    }
}

#[derive(Debug)]
struct OnAcceptDenyResp {
    pub reason: Box<str>,
}

impl LuaUserData for OnAcceptDenyResp {
}


impl FromLua for OnAcceptDenyResp {
    fn from_lua(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
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

#[derive(Debug)]
struct OnAcceptRedirectResp {
    pub status: u16,
    pub location: Box<str>,
}

impl LuaUserData for OnAcceptRedirectResp {
}

impl FromLua for OnAcceptRedirectResp {
    fn from_lua(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
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

#[derive(Debug)]
enum OnAcceptResp {
    ExpectHttp(OnAcceptExpectHttpResp),
    Deny(OnAcceptDenyResp),
    Redirect(OnAcceptRedirectResp),
}

impl FromLua for OnAcceptResp {
    fn from_lua(lua_value: LuaValue, _: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                if let Ok(resp) = ud.take::<OnAcceptExpectHttpResp>() {
                    Ok(OnAcceptResp::ExpectHttp(resp))
                } else if let Ok(resp) = ud.take::<OnAcceptDenyResp>() {
                    Ok(OnAcceptResp::Deny(resp))
                } else if let Ok(resp) = ud.take::<OnAcceptRedirectResp>() {
                    Ok(OnAcceptResp::Redirect(resp))
                } else {
                    Err(LuaError::FromLuaConversionError {
                        from: "UserData",
                        to: "OnAcceptResp".to_string(),
                        message: Some("Expected a userdata of type OnAcceptExpectHttpResp, OnAcceptDenyResp, or OnAcceptRedirectResp".to_string()),
                    })
                }
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnAcceptResp".to_string(),
                message: Some("Expected a userdata of type OnAcceptExpectHttpResp, OnAcceptDenyResp, or OnAcceptRedirectResp".to_string()),
            }),
        }
    }
}

pub struct Context;
impl LuaUserData for Context {}

pub struct Request<'buf> {
    pub method: &'buf str,
    pub path: &'buf str,
    pub headers: [(&'buf str, &'buf str); 64],
    pub headers_count: usize,
}

impl<'buf> LuaUserData for Request<'buf> {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("method", |lua, this| lua.create_string(this.method));
        fields.add_field_method_get("path", |lua, this| lua.create_string(this.path));
        fields.add_field_method_get("headers", |lua, this| {
            let table = lua.create_table()?;
            for i in 0..this.headers_count {
                let (k, v) = this.headers[i];
                table.set(k, v)?;
            }
            Ok(table)
        });
    }
}

#[derive(Debug)]
pub struct RespCacheBlock {
    pub enabled: bool,
    pub ttl: u32,
    pub key: Box<str>,
}

#[derive(Debug)]
pub struct HeadersPatchBlock {
    pub add_headers: HashMap<Box<str>, Box<str>>,
    pub remove_headers: Vec<Box<str>>,
}

#[derive(Debug)]
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

impl LuaUserData for OnRequestExpectHttpBodyResp {
    fn add_methods<F: LuaUserDataMethods<Self>>(methods: &mut F) {
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

        methods.add_method_mut("set_cache", |_, this, (ttl, key): (u32, Box<str>)| {
            this.cache = Some(RespCacheBlock {
                enabled: true,
                ttl,
                key,
            });
            Ok(())
        });
    }
}

#[derive(Debug)]
// we need to wrap these response in extra wrapper for configuring cache or static use for these lua fn returns
// or is there something like decorator?
pub enum OnRequestResp {
    ExpectHttpBody(OnRequestExpectHttpBodyResp),
    Deny(OnAcceptDenyResp),
    Redirect(OnAcceptRedirectResp),
}

impl FromLua for OnRequestResp {
    fn from_lua(lua_value: LuaValue, _: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                if let Ok(resp) = ud.take::<OnRequestExpectHttpBodyResp>() {
                    Ok(OnRequestResp::ExpectHttpBody(resp))
                } else if let Ok(resp) = ud.take::<OnAcceptDenyResp>() {
                    Ok(OnRequestResp::Deny(resp))
                } else if let Ok(resp) = ud.take::<OnAcceptRedirectResp>() {
                    Ok(OnRequestResp::Redirect(resp))
                } else {
                    Err(LuaError::FromLuaConversionError {
                        from: "UserData",
                        to: "OnRequestResp".to_string(),
                        message: Some("Expected a userdata of type OnRequestExpectHttpBodyResp, OnAcceptDenyResp, or OnAcceptRedirectResp".to_string()),
                    })
                }
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "OnRequestResp".to_string(),
                message: Some("Expected a userdata of type OnRequestExpectHttpBodyResp, OnAcceptDenyResp, or OnAcceptRedirectResp".to_string()),
            }),
        }
    }
}


pub struct Proxy {
    pub on_accept_fn: Rc<RefCell<Option<LuaFunction>>>,
    pub req_handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>>,
    pub listen_addr: Box<str>
}

impl LuaUserData for Proxy {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("listen_on", |_, this, addr: Box<str>| {
            this.listen_addr = addr;
            Ok(())
        });

        methods.add_method_mut("on_request", |_, this, (host, f): (Box<str>, LuaFunction)| {
            // fn(client, request, context) -> OnRequestExpectHttpBodyResp | Deny | Redirect
            this.req_handlers.borrow_mut().insert(host, f);
            Ok(())
        });

        methods.add_method_mut("on_accept", |_, this, f: LuaFunction| {
            this.on_accept_fn.replace(Some(f));
            Ok(())
        });

        methods.add_method("resp_expect_httpbody", |lua, _, upstream: Box<str>| {
            let resp = OnRequestExpectHttpBodyResp {
                upstream,
                max_body_size: 1048576,
                proxy_read_timeout: Some(30),
                client_body_timeout: Some(30),
                cache: None,
                headers_patch: None,
            };
            lua.create_userdata(resp)
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
            lua.create_userdata(resp)
        });

        methods.add_method("resp_deny", |lua, _, reason: Box<str>| {
            let resp = OnAcceptDenyResp { reason };
            lua.create_userdata(resp)
        });

        methods.add_method("resp_redirect", |lua, _, (status, location): (u16, Box<str>)| {
            let resp = OnAcceptRedirectResp { status, location };
            lua.create_userdata(resp)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_handler() -> LuaResult<()> {
        let path = "/";
        let method = "GET";
        let headers = [("Host", "example.com"), ("ContentLength", "64")];

        let mut hdrs: [(&str, &str); 64] = [("", ""); 64];
        hdrs[..headers.len()].copy_from_slice(&headers);

        let req_handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>> = Rc::new(RefCell::new(HashMap::new()));
        let on_accept_fn: Rc<RefCell<Option<LuaFunction>>> = Rc::new(RefCell::new(None));
        let proxy = Proxy{
            req_handlers: req_handlers.clone(),
            on_accept_fn: on_accept_fn.clone(),
            listen_addr: "".into(),
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


        lua.scope(|scope| {
            let client1 = scope.create_userdata(Client{
                ip: "127.0.0.1".into(),
                port: 6969
            });
            let client2 = scope.create_userdata(Client{
                ip: "10.100.1.1".into(),
                port: 6969
            });
            let client3 = scope.create_userdata(Client{
                ip: "192.168.1.1".into(),
                port: 6969
            });

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client1)?;
            println!("{:?}", resp);

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client2)?;
            println!("{:?}", resp);

            let resp = on_accept_fn.borrow().as_ref().unwrap().call::<OnAcceptResp>(client3)?;
            println!("{:?}", resp);
            Ok(())
        })?;

        println!();

        lua.scope(|scope| {
            let req = scope.create_userdata(Request{
                method,
                headers: hdrs,
                path,
                headers_count: headers.len(),
            })?;

            let client = scope.create_userdata(Client{
                ip: "127.0.0.1".into(),
                port: 6969
            })?;

            let context = scope.create_userdata(Context{})?;

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
