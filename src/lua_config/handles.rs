use mlua::prelude::*;
use std::cell::RefCell;
use std::net::IpAddr;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use tealr::ToTypename;
use tealr::mlu::TealDataMethods;

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

    #[allow(dead_code)]
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
