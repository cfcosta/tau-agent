---
name: principle-boundary-discipline
description: "Load when wiring validation, error handling, parsing, or framework and storage adapters. Concentrate guards at system boundaries (CLI, config, network, database, external APIs), trust internal types, and keep business logic in pure functions."
---

# Boundary Discipline

Place validation, type narrowing and error handling at system boundaries. Trust internal code unconditionally. Business logic lives in pure functions. The shell around it is thin and mechanical.

**Why:** Scattered validation is noisy and redundant, and it gives a false sense of safety: each check covers only the path it sits on. Logic kept out of framework wiring can be tested without the framework.

**The pattern:**
- **At boundaries** (CLI arguments, config files, environment variables, database rows, external APIs, network protocols): validate, return errors, handle defensively.
- **Inside the system:** typed data, propagated errors, no re-validation. Trust the types.
- **Across the boundary:** expose domain concepts, not the boundary's private representation. Keep general-purpose mechanism inside and special-purpose policy at the edge.

**Applications:**
- A config file is parsed and checked once, at load, into a `Config` type, with errors that name the bad key. Business logic takes the `Config`. It never reads the file or the environment again.
- A database row is turned into a domain type in the storage layer. Callers get a `Session`, not a row, a column map, or a record of nullable fields that mirrors the table.
- A provider's wire types (the JSON request and response shapes of an HTTP API) stay inside the provider module. The rest of the code sees domain messages and errors, so a provider change touches one module.
- Do not re-export transport, storage, framework or wire types through the public interface.
- No redundant null checks deep in call chains when the boundary already validated.
- Parse functions are pure transforms from raw bytes to typed state. Prompt construction is structured state in, string out. Scoring and assessment are pure transforms from state to results.
- Business logic has no framework dependencies, so its tests need no database, server or UI.

**The tests:**
- "Is this data crossing a system boundary right now?" If not, validation here is redundant.
- "Can this be a pure function that the shell just calls?" If yes, extract it.

Distinct from `principle-type-system-discipline`, which shapes the types so the code inside the boundary needs no checks. This principle says where the checks and the parsing go.
