mod http;
mod lua_config;
mod net;
mod relay;
mod resolve;
mod session;

use crate::lua_config::{
    CacheParam, ClientHandle, CustomHttpResponse, Deny, ExpectHttp, FileResponse, ForwardHttp,
    LuaConf, Proxy, RequestHandle,
};
use mlua::prelude::*;
use monoio::net::TcpListener;
use std::fs;
use std::rc::Rc;
use std::time::Duration;
use tealr::TypeWalker;

#[allow(unused_variables)]
#[monoio::main(threads = 8, timer_enabled = true)]
async fn main() {
    unsafe { std::env::set_var("RUST_LOG", "warn") };
    let _ = env_logger::try_init();

    let types = TypeWalker::new()
        .process_type::<Proxy>()
        .process_type::<CacheParam>()
        .process_type::<ClientHandle>()
        .process_type::<RequestHandle>()
        .process_type::<ExpectHttp>()
        .process_type::<Deny>()
        .process_type::<CustomHttpResponse>()
        .process_type::<FileResponse>()
        .process_type::<ForwardHttp>();

    if let Ok(json) = types.to_json() {
        if fs::write("nexus.json", json).is_err() {
            log::error!("Teal doc json write failed");
        }
    } else {
        log::error!("Teal doc json generation failed");
    }

    let conf_source = fs::read("config.lua").expect("config.lua not found in working directory");

    let state = Rc::new(LuaConf::new().expect("lua configuration state initialization failed"));

    let source = state
        .lua
        .load(str::from_utf8(&conf_source).expect("invalid utf-8 in config.lua"));
    source.exec().expect("config soruce execution failed");

    let host: String = state
        .lua
        .globals()
        .get::<LuaAnyUserData>("proxy")
        .expect("proxy not defined in config")
        .borrow::<Proxy>()
        .unwrap()
        .listen_addr
        .as_ref()
        .into();
    println!("{}", host);
    let listener = TcpListener::bind(host).expect("TCP binding failed");

    if net::enable_defer_accept(&listener, Duration::from_secs(1)).is_err() {
        log::warn!("failed to enable TCP defer accept");
    };

    loop {
        let (stream, peer_addr) = match listener.accept().await {
            Ok(connection) => connection,
            Err(e) => {
                log::error!("TCP accept failed: {e}");
                continue;
            }
        };
        if stream.set_nodelay(true).is_err() {
            log::debug!("TCP no delay set failed")
        }
        let state_pass = state.clone();
        monoio::spawn(async move {
            let r = session::handle_req(stream, peer_addr, state_pass).await;
            if let Err(e) = r {
                log::error!("client {peer_addr}: {e}");
            }
        });
    }
}
