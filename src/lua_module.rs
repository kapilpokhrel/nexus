use crate::lua_config::HandlePool;
use mlua::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CtxUpdate {
    pub updates: HashMap<Box<str>, LuaValue>,
}

impl LuaUserData for CtxUpdate {
    fn add_methods<F: LuaUserDataMethods<Self>>(methods: &mut F) {
        methods.add_method_mut("add_to_ctx", |_, this, (key, val): (Box<str>, LuaValue)| {
            this.updates.insert(key, val);
            Ok(())
        });
    }
}

#[derive(Debug, Clone)]
pub struct PassResp();
impl LuaUserData for PassResp {}

#[derive(Debug, Clone)]
pub enum HookResult {
    ContinueWithCtx(CtxUpdate),
    // should add deny and redirect too
    Pass(PassResp), // no opinion
}

impl FromLua for HookResult {
    fn from_lua(lua_value: LuaValue, _: &Lua) -> LuaResult<Self> {
        match lua_value {
            LuaValue::UserData(ud) => {
                if let Ok(resp) = ud.take::<CtxUpdate>() {
                    Ok(HookResult::ContinueWithCtx(resp))
                } else if let Ok(resp) = ud.take::<PassResp>() {
                    Ok(HookResult::Pass(resp))
                } else {
                    Err(LuaError::FromLuaConversionError {
                        from: "UserData",
                        to: "HookResult".to_string(),
                        message: Some(
                            "Expected a userdata of type CtxUpdate or PassResp".to_string(),
                        ),
                    })
                }
            }
            _ => Err(LuaError::FromLuaConversionError {
                from: lua_value.type_name(),
                to: "HookResult".to_string(),
                message: Some("Expected a userdata of type CtxUpdate or PassResp".to_string()),
            }),
        }
    }
}

pub const HOOK_POOL_SIZE: usize = 8;

pub struct ProxyModule {
    pub hook_handler: RefCell<Option<LuaFunction>>,
    pub lua: Lua,
    pub pool: HandlePool,
}

impl ProxyModule {
    pub fn new(lua: Lua) -> Self {
        let pool = HandlePool::new(lua.clone(), HOOK_POOL_SIZE);
        ProxyModule {
            hook_handler: RefCell::new(None),
            lua,
            pool,
        }
    }
}

#[derive(Clone)]
struct Module(Rc<ProxyModule>);

impl LuaUserData for Module {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut("add_incomming_hook", |_, this, f: LuaFunction| {
            // f must return
            this.0.hook_handler.replace(Some(f));
            Ok(())
        });

        methods.add_method("resp_update_ctx", |lua, _, _: ()| {
            let resp = CtxUpdate {
                updates: HashMap::new(),
            };
            lua.create_userdata(resp)
        });

        methods.add_method("resp_pass", |lua, _, _: ()| lua.create_userdata(PassResp()));
    }
}

pub struct ProxyModuleTable {
    pub modules: HashMap<Box<str>, Rc<ProxyModule>>,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct ModuleManifest {
    name: String,
    requires: Vec<String>,
}

impl ProxyModuleTable {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut modules: HashMap<Box<str>, Rc<ProxyModule>> = HashMap::new();

        // search for modules in /modules directory and load them
        // each module has its own directly and every module should have main.lua file which will be loaded and executed
        // manifest.toml too for each module which will contain the module name and description and version; and permission later
        let modules_dir = std::path::Path::new("./modules");
        if modules_dir.exists() && modules_dir.is_dir() {
            for entry in std::fs::read_dir(modules_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    let manifest_path = path.join("manifest.toml");
                    let main_path = path.join("main.lua");
                    if !manifest_path.exists() || !main_path.exists() {
                        continue;
                    }
                    let manifest = std::fs::read_to_string(manifest_path)?;
                    let manifest: ModuleManifest = toml::from_str(&manifest)?;
                    let module_name = manifest.name;
                    let module_requires = manifest.requires;
                    let main_script = std::fs::read_to_string(main_path)?;

                    let lua = unsafe { Lua::unsafe_new() };

                    // Save the original require
                    let original_require: LuaFunction = lua.globals().get("require")?;

                    let require = lua.create_function(move |_, name: String| {
                        if module_requires.contains(&name.to_string()) {
                            original_require.call::<LuaValue>(name)
                        } else {
                            Err(mlua::Error::RuntimeError(format!(
                                "module '{}' is not permitted",
                                name
                            )))
                        }
                    })?;

                    lua.globals().set("require", require)?;

                    // async primitive that yields: `conduit.sleep(ms)`
                    let conduit = lua.create_table()?;
                    let sleep = lua.create_async_function(|_, ms: u64| async move {
                        monoio::time::sleep(Duration::from_millis(ms)).await;
                        Ok(())
                    })?;
                    conduit.set("sleep", sleep)?;
                    lua.globals().set("conduit", conduit)?;

                    let module = Rc::new(ProxyModule::new(lua.clone()));
                    let module_ud = lua.create_userdata(Module(module.clone()))?;
                    lua.globals().set(module_name.clone(), module_ud)?;
                    lua.load(&main_script).exec()?;

                    modules.insert(module_name.into(), module.clone());
                }
            }
        }
        Ok(ProxyModuleTable { modules })
    }
}

#[cfg(test)]
mod tests {
    use crate::lua_config::{Client, Context, Request};

    use super::*;

    #[test]
    fn modules_load() -> LuaResult<()> {
        let module_table = ProxyModuleTable::new().unwrap();
        let ctx = Context::new();

        let req = Request::from_parts(
            "GET",
            "path",
            &[("Host", "example.com"), ("ContentLength", "64")],
        );

        let client = Client::new("127.0.0.1".parse().unwrap(), 6969);
        for (name, module) in module_table.modules {
            let f = module.hook_handler.borrow().as_ref().cloned();
            if let Some(f) = f {
                let h = module.pool.checkout();
                h.set_client(client.clone());
                h.set_req(req.clone());
                h.set_ctx(ctx.clone());
                let x = f.call::<HookResult>((&h.client_ud, &h.req_ud, &h.ctx_ud));
                println!("{:?}, {:?}", name, x);
            }
        }
        Ok(())
    }
}
