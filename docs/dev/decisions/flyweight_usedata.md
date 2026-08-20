As of today, 20th Augutst 2026, each lua state has a pool of handles (preinitialized lua userdata for lua fn parameters: `client`, `request`, `context`)
and in each request, we checkout one handle from the pool, sets the inner value for `client`, `request`, `context` and call with already created userdata.

```rust
let call_lua_fn = || {
    let h = state.pool.checkout();
    h.set_client(client.clone());
    h.set_req(req.clone());
    h.set_ctx(context.clone());

    lua_fn
        .call::<OnRequestResp>((&h.client_ud, &h.req_ud, &h.ctx_ud))
        .unwrap()
};
```

### Why?
Without lua scope, there are lots of create_userdata per request. And with more hooks attached, even more of those.
`create_userdata` itself doesn't do very much but with many of them, there's a gorwing cost attached in their cleanup.

So, With a flyweight pattern, we create a userdata of RefCell and all the fields/methods of userdata reads from inside it.
Kind of like a container which acts as a value.
And before each call, we set our value in that RefCell container as seen in above snippet and fields/methods will use that value.


### Why pool them? Why can't a state has one handle?

Well, it works only for the sync call. In the above snippet, imagine there is no checkout, we set out values in global handle.
We call sync function, it blocks until it is complete. It is good.

But we that function call were to be async, we set out data in the global handle, call async function, it yields,
another request get a chance, it updates the global handle with its value, first function gets the cpu but now it sees value of 2nd request.

With pool, once a handle is checked out, it won't be handed back until a function call is finished and next request will simply get another handle from the pool.
