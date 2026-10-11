use super::handles::{ClientHandle, RequestHandle};
use mlua::prelude::*;
use tealr::mlu::{FromLuaExact, TealData, TealDataMethods, TypedFunction};
use tealr::{ToTypename, create_union_mlua};

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
            LuaValue::UserData(ud) if ud.is::<Self>() => ud.take::<Self>(),
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
            LuaValue::UserData(ud) if ud.is::<Self>() => ud.take::<Self>(),
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "Deny".to_string(),
                message: Some("Expected a userdata with 'reason' field".to_string()),
            }),
        }
    }
}

#[derive(Debug, Clone, tealr::mlu::UserData, ToTypename)]
pub struct CustomHttpResponse {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl FromLuaExact for CustomHttpResponse {
    fn from_lua_exact(lua_value: LuaValue, _lua: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => ud.take::<Self>(),
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "CustomHttpResponse".to_string(),
                message: Some("Expected a userdata".to_string()),
            }),
        }
    }
}

impl tealr::mlu::TealData for CustomHttpResponse {
    fn add_fields<F: tealr::mlu::TealDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("status", |_, this| Ok(this.status));
        fields.add_field_method_get("reason", |lua, this| lua.create_string(&this.reason));
        fields.add_field_method_get("body", |lua, this| lua.create_string(&this.body));
    }

    fn add_methods<M: TealDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("set_status", |_, this, status: u16| {
            this.status = status;
            Ok(())
        });
        methods.add_method_mut("set_reason", |_, this, reason: String| {
            this.reason = reason;
            Ok(())
        });
        methods.add_method_mut("set_body", |_, this, body: String| {
            this.body = body;
            Ok(())
        });
        methods.add_method_mut("add_header", |_, this, (name, value): (String, String)| {
            this.headers.push((name, value));
            Ok(())
        });
    }
}

create_union_mlua!(pub enum OnAcceptHandlerResp = Deny | ExpectHttp | CustomHttpResponse);

impl std::fmt::Debug for OnAcceptHandlerResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnAcceptHandlerResp::ExpectHttp(x) => f.debug_tuple("ExpectHttp").field(x).finish(),
            OnAcceptHandlerResp::Deny(x) => f.debug_tuple("Deny").field(x).finish(),
            OnAcceptHandlerResp::CustomHttpResponse(x) => {
                f.debug_tuple("CustomHttpResponse").field(x).finish()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RespCacheBlock {
    pub enabled: bool,
    pub ttl: u32,
    pub key: Box<str>,
}

#[derive(Debug, Clone)]
pub struct HeaderPatchBlock {
    pub add_headers: Vec<(String, String)>,
    pub remove_headers: Vec<String>,
}

impl HeaderPatchBlock {
    pub fn empty() -> Self {
        Self {
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
    pub cache: Option<RespCacheBlock>,
    pub headers_patch: Option<HeaderPatchBlock>,
    pub keep_alive: bool,
}

impl FromLuaExact for ForwardHttp {
    fn from_lua_exact(lua_value: LuaValue, _: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) if ud.is::<Self>() => ud.take::<Self>(),
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
            let patch = this
                .headers_patch
                .get_or_insert_with(HeaderPatchBlock::empty);
            match patch.add_headers.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = val,
                None => patch.add_headers.push((key, val)),
            }
            Ok(())
        });
        methods.add_method_mut("remove_header", |_, this, key: String| {
            this.headers_patch
                .get_or_insert_with(HeaderPatchBlock::empty)
                .remove_headers
                .push(key);
            Ok(())
        });
        methods.add_method_mut("keep_alive", |_, this, val: bool| {
            this.keep_alive = val;
            Ok(())
        });
    }
}

create_union_mlua!(pub enum OnRequestHandlerResp = ForwardHttp | Deny | CustomHttpResponse);

impl std::fmt::Debug for OnRequestHandlerResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnRequestHandlerResp::ForwardHttp(x) => f.debug_tuple("ForwardHttp").field(x).finish(),
            OnRequestHandlerResp::Deny(x) => f.debug_tuple("Deny").field(x).finish(),
            OnRequestHandlerResp::CustomHttpResponse(x) => {
                f.debug_tuple("CustomHttpResponse").field(x).finish()
            }
        }
    }
}

pub type OnAcceptFn = TypedFunction<ClientHandle, OnAcceptHandlerResp>;
pub type OnRequestFn = TypedFunction<(ClientHandle, RequestHandle), OnRequestHandlerResp>;
