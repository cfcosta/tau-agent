---
name: principle-type-system-discipline
description: "Load when designing types, a function signature, or a data model in any statically typed language. Make illegal states unrepresentable, give semantic primitives their own types, exhaust variants, parse external data at boundaries, and derive types from schemas."
---

# Type System Discipline

Make the compiler reject impossible states, mixed-up primitives, and unhandled variants. Prefer defining errors and special cases out of existence over writing more handlers. The rules hold in any statically typed language. Use the nearest feature the repository's language has. Where it has none, get the same effect from a type nothing outside its module can build, with one constructor that checks the rule.

**Why:** The compiler checks every caller on every build, including callers written next year by someone who never read your comment. A runtime check or a convention catches only the cases somebody remembered.

- **Make illegal states unrepresentable.** If two fields must never be set together, or a field only matters when a flag is true, the type is too loose. Replace a bag of booleans and nullable fields with a tagged union (sum type) whose cases carry exactly the data each state has. A connection record with a `connected` flag, a nullable socket and a nullable error admits combinations that mean nothing, such as connected with an error. A union of three cases (idle, connected with its socket, failed with its error) admits none. Encode invariants in the shape, such as a non-empty list as a first element plus a list of the rest.
- **Give semantic primitives their own types.** `UserId` and `OrderId` are both strings underneath and must not be interchangeable. Give each kind of ID a distinct type that wraps the string, with the raw value behind module-private visibility so only its constructor can make one. Validate once in the constructor and trust the type downstream. Do the same for units: a duration in milliseconds as its own type, not a bare integer.
- **Exhaust variants.** Handle a tagged union you own exhaustively: name every case and write no default or catch-all branch. Adding a case then breaks the build at each place that must decide what the case means. A catch-all branch turns that compile error into a silent default. Leave a union open to new cases only at a public boundary, where other packages must keep compiling as cases grow. Inside your own code, keep handling exhaustive. If the compiler does not check exhaustiveness, turn on the lint that does, or add a test that fails on an unhandled case.
- **External data is untyped until parsed.** RPC payloads, JSON, IPC messages, CLI arguments, config files, environment variables, and database rows. At each boundary, one parse step turns the input into the typed model: a parsing constructor, or a decoder for the serialization format that builds domain types directly. Make it reject unknown fields where a typo should fail. Return the parsed value or a parse error, not a boolean, so the proof stays in the type. Convert a number with a checked conversion that fails when the value does not fit, not with a cast that silently truncates. Load the `principle-boundary-discipline` skill for where these parse steps go.
- **Derive types from authoritative schemas.** When a protobuf file, an OpenAPI spec, a GraphQL schema, a database migration, or a design-token file owns a shape, generate the type from it or check it against it. A query library that checks each SQL query against the real schema at build time ties row types to that schema, so a migration that drops a column breaks the build instead of production.

**The test:**
1. List the values your type can hold. Is any of them a state the program must never be in?
2. Add a case to the union in your head. Does the compiler point at every place that needs a decision?
3. Swap two arguments at a call site. Does it still compile?

A "yes" to the first or third, or a "no" to the second, means the type needs work.

You skipped this when a function opens with checks its argument types should have ruled out, when a comment says "only set when X" or "call after Y", or when adding a case to a union broke nothing at compile time.

Distinct from `principle-boundary-discipline`, which says where validation and error handling live. This principle shapes the types so the code inside the boundary needs no checks. Distinct from `principle-model-the-domain`, which picks the structure (a state machine, a table, a reducer). This principle makes the compiler hold that structure in place.
