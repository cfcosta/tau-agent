# Codemode `map`

`map(items, fn, concurrency?)` runs `fn(item, index)` for each item and returns
an array of settled records in item order. Indices start at 1. Completion order
does not affect result order.

```luau
local results = map(array({ "a", "b", "c" }), function(item, index)
    return { index = index, reply = tools.echo({ text = item }) }
end, 2)
-- Each result is { ok = true, value = ... } or
--                { ok = false, error = "..." }.
```

`items` must be a dense array marked with `array(...)` or produced as a JSON
array by `json.decode`, a tool, or `load`. An empty marked array is valid and
returns `[]`. Sparse arrays, mixed keys, objects, non-arrays, and arrays with
more than 10,000 items fail before any callback starts.

`fn` must be a function. Its first return value becomes `value`. Further
return values are ignored. No return or a first return of `nil` stores
`json.null`, so every successful record has a `value` field. A returned
`json.null` also remains JSON null. A callback error becomes an error string
in that item's record; other callbacks continue. The map call itself raises
for invalid arguments or if the whole script is cancelled or times out.

`concurrency` defaults to 4 and must be an integer from 1 through 32. It
limits running callback coroutines, including callbacks that start nested
tool calls. Queued callbacks start as running callbacks finish. When the
script ends, times out, is cancelled, or calls `exit()`, pending work is
dropped and queued callbacks do not start. Tool calls have their usual side
effects and are not rolled back.
