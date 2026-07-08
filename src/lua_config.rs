use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct Request<'buf> {
    pub ip: String,
    pub method: &'buf str,
    pub path: &'buf str,
    pub headers: [(&'buf str, &'buf str); 64],
    pub headers_count: usize,
}

impl<'buf> LuaUserData for Request<'buf> {
    fn add_fields<F: LuaUserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("ip", |lua, this| lua.create_string(this.ip.as_str()));
        fields.add_field_method_get("method", |lua, this| lua.create_string(this.method));
        fields.add_field_method_get("path", |lua, this| lua.create_string(this.path));
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

pub struct Proxy {
    pub route_handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>>,
    pub static_route_map: Rc<RefCell<HashMap<Box<str>, Box<str>>>>,
    pub listen_addr: Box<str>
}

impl LuaUserData for Proxy {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("on_route", |_, this, (host, f): (Box<str>, LuaFunction)| {
            this.route_handlers.borrow_mut().insert(host, f);
            Ok(())
        });

        methods.add_method_mut("on_route_static", |_, this, (host, f): (Box<str>, Box<str>)| {
            this.static_route_map.borrow_mut().insert(host, f);
            Ok(())
        });
        // hybrid of these two, we can use on_route_cached that takes a function and cache period but will heavily cache it so that it behaves like static

        methods.add_method_mut("listen_on", |_, this, addr: Box<str>| {
            this.listen_addr = addr;
            Ok(())
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
        let ip = "10.100.1.1";

        let mut hdrs: [(&str, &str); 64] = [("", ""); 64];
        hdrs[..headers.len()].copy_from_slice(&headers);
        let request = Request {
            ip: ip.to_string(),
            method,
            headers: hdrs,
            path,
            headers_count: headers.len(),
        };

        let handlers: Rc<RefCell<HashMap<Box<str>, LuaFunction>>> = Rc::new(RefCell::new(HashMap::new()));
        let static_route_map: Rc<RefCell<HashMap<Box<str>, Box<str>>>> = Rc::new(RefCell::new(HashMap::new()));
        let proxy = Proxy{
            route_handlers: handlers.clone(),
            static_route_map,
            listen_addr: "".into(),
        };
        let lua = Lua::new();

        let proxy_ud = lua.create_userdata(proxy).unwrap();
        lua.globals().set("proxy", proxy_ud)?;

        let source = lua.load(
            r#"
            proxy:on_route("example.com", function(req)
                print(req.ip, req.path, req.method)
                print("HEADERS:")
                for key, value in pairs(req.headers) do
                    print(key, value)
                end
            end)
        "#,
        );
        source.exec()?;

        lua.scope(|scope| {
            let req = scope.create_userdata(request)?;
            handlers
                .borrow()
                .get("example.com")
                .unwrap()
                .call::<()>(req)
        })?;
        Ok(())
    }
}
