mod http;
mod hooks;
mod lua_config;
mod lua_module;
mod net;
mod relay;
mod session;

use crate::lua_config::{ClientHandle, ContextHandle, LuaState, OnAcceptDenyResp, OnAcceptExpectHttpResp, OnAcceptRedirectResp, OnRequestExpectHttpBodyResp, Proxy, RequestHandle};
use crate::lua_module::ProxyModuleTable;
use mlua::prelude::*;
use monoio::net::TcpListener;
use tealr::TypeWalker;
use std::fs;
use std::rc::Rc;
use std::time::Duration;

pub const CONF_POOL_SIZE: usize = 8;

#[allow(unused_variables)]
#[monoio::main(threads = 8, timer_enabled = true)]
async fn main() {
    let types = TypeWalker::new()
        .process_type::<Proxy>()
        .process_type::<ClientHandle>()
        .process_type::<RequestHandle>()
        .process_type::<ContextHandle>()
        .process_type::<OnAcceptExpectHttpResp>()
        .process_type::<OnAcceptDenyResp>()
        .process_type::<OnAcceptRedirectResp>()
        .process_type::<OnRequestExpectHttpBodyResp>();

    let json = types.to_json().unwrap();
    fs::write("conduit.json", json);

    let conf_source = fs::read("config.lua").unwrap();

    let module_table = Rc::new(ProxyModuleTable::new());
    let state = Rc::new(LuaState::new(CONF_POOL_SIZE).unwrap());

    // reusing handles means lua allocates so little that the default incremental
    // GC stops keeping up; generational collects it at no measurable cost
    state.lua.gc_gen(20, 100);

    let source = state.lua.load(str::from_utf8(&conf_source).unwrap());
    source.exec().unwrap();

    let host: String = state.lua.globals().get::<LuaAnyUserData>("proxy").unwrap().borrow::<Proxy>().unwrap().listen_addr.as_ref().into();
    println!("{}", host);
    let listener = TcpListener::bind(host).unwrap();
    net::enable_defer_accept(&listener, Duration::from_secs(1)).unwrap();

    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let state_pass = state.clone();
        let module_table_pass = module_table.clone();
        monoio::spawn(async move {
            let _r = session::forward_http(stream, state_pass, module_table_pass).await;
        });
    }
}
