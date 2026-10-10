> [!NOTE]
> This was before the removal of modules


Modules don't recieve a context. They are independent, they only set a context

**Why was this done?**

Managing a dependencies between module would be a hassle.
Either we have to let config define explict order by themselves or detect which modules uses who's context
and do some kind of topological sort.

And that's nothing when we think about caching (invalidation, keys .. ). It is a mess.

With independent modules, each module can independently have their own caching scheme
And with independent modules, we can run them all simultaneously (have to sync context if they all access it)

And that is also a reason why for now, `on_request` isn't cached either.
We will implement it later depending on the response type like is it a static file response or is the response completely independent of the request .. Later.
