mod http;
mod lua_config;
mod net;
mod relay;
mod resolve;
mod session;

use crate::lua_config::{
    CacheParam, ClientHandle, CustomHttpResponse, Deny, ExpectHttp, ForwardHttp, LuaConf, Proxy,
    RequestHandle,
};
use mlua::prelude::*;
use monoio::net::TcpListener;
use std::fs;
use std::rc::Rc;
use std::time::Duration;
use tealr::TypeWalker;

pub const CONF_POOL_SIZE: usize = 8;

#[allow(unused_variables)]
#[monoio::main(threads = 8, timer_enabled = true)]
async fn main() {
    let types = TypeWalker::new()
        .process_type::<Proxy>()
        .process_type::<CacheParam>()
        .process_type::<ClientHandle>()
        .process_type::<RequestHandle>()
        .process_type::<ExpectHttp>()
        .process_type::<Deny>()
        .process_type::<CustomHttpResponse>()
        .process_type::<ForwardHttp>();

    let json = types.to_json().unwrap();
    if fs::write("conduit.json", json).is_err() {
        println!("Teal doc json write failed");
    }

    let conf_source = fs::read("config.lua").unwrap();

    let state = Rc::new(LuaConf::new(CONF_POOL_SIZE).unwrap());
    let source = state.lua.load(str::from_utf8(&conf_source).unwrap());
    source.exec().unwrap();

    let host: String = state
        .lua
        .globals()
        .get::<LuaAnyUserData>("proxy")
        .unwrap()
        .borrow::<Proxy>()
        .unwrap()
        .listen_addr
        .as_ref()
        .into();
    println!("{}", host);
    let listener = TcpListener::bind(host).unwrap();
    net::enable_defer_accept(&listener, Duration::from_secs(1)).unwrap();

    loop {
        let Ok((stream, peer_addr)) = listener.accept().await else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let state_pass = state.clone();
        monoio::spawn(async move {
            let _r = session::handle_req(stream, peer_addr, state_pass).await;
        });
    }
}
