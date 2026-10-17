---
name: principle-test-behavior-not-implementation
description: "Load when you write, change, review, or keep a test. Assert what the code's users can observe, so the test fails when that behavior breaks and keeps passing when only the internals change."
---

# Test Behavior, Not Implementation

A test is a claim about what the code does for its users: a caller, an end user, or another service. Behavior is what those users can observe through the interface they use. That includes return values, errors, state they can read back, and effects on systems outside the code, such as a request sent, a row written, or a message emitted. Implementation is what they cannot observe: private helpers, internal call order, intermediate data, and which internal collaborator did the work.

**Why:** A test pinned to the implementation needs a rewrite on every refactor and still catches no bug. A test looser than the contract passes while the behavior is broken. Both cost maintenance and give false confidence.

Judge each test against two imagined edits:

- **Break the behavior.** Make the code return a wrong value, skip an effect, or do the effect twice. If the test still passes, it proves nothing. The usual causes are an assertion looser than the contract, an expected value computed by the code under test, a restated constant or prompt string that never runs the code that reads it, and an assertion on data the test built itself.
- **Change the internals.** Keep every observable result the same and restructure the code underneath. If the test now fails, it is pinned to the implementation.

Keep a test that catches the first edit and survives the second. Rewrite or delete the rest. To write a test that does both, drive the code through its users' interface with a concrete input. Derive the expected value from the requirement, not from a run of the code. Assert what the contract promises, no less and no more.

The contract decides, not the shape of the assertion. When "an empty cart never charges" is the promise, an assertion that the payment client got no call is behavior. Pair it with a case where the charge happens, so the test shows it can see a call. The payload a mocked external service receives is behavior, because that service observes it. A call to an internal helper is implementation, because no user can see it. An empty list is the exact answer when no record matches, and a hollow check when the test only needs some result. An exact error message pins wording no caller reads. The error type callers branch on is the contract.

A test can pin an invariant instead of a call, such as a key that must exist in two tables or a compile-time type check. The invariant is the behavior.

You skipped this when a test broke during a refactor that changed no output, or when you can break the code by hand and the test stays green.

Distinct from `principle-prove-it-works`, which checks the real artifact once, before you call a task done. This principle decides what a lasting test asserts. The `tdd` skill uses it to write the test before the code.
