As discussed in [[../notes/async_io_in_lua]]. We need to implement blocking io primitives
in rust side. This is not something we will do now but this is something that we could do.

And this also fits in for our idea of controlled and monitored sandboxing. If we replace io primitives with our implementation, monitoring their use will be a breeze.
