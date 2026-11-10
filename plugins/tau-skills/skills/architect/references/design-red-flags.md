# Design red flags

Screen every candidate before you choose. A red flag is a reason to revise or reject the shape. The flags hold in any language.

## Shallow module

A shallow module exposes a large interface while hiding little complexity. Judge depth by the capability and policy hidden behind the public interface relative to the size of that interface. Prefer a simple interface backed by substantial behaviour.

Do not confuse a deep module with a deep call chain. A deep call chain scatters understanding across layers. A deep module concentrates capability behind one interface.

Look for these signs:

- Callers coordinate several methods to complete one operation.
- Public options expose internal stages or implementation choices.
- Learning the interface does not save the caller from learning the implementation.

An interface with one implementation whose methods mirror that implementation's methods one to one is a shallow module. It adds a name and a layer of indirection, and hides nothing. Keep the concrete type until a second implementation exists, or until the interface hides a real decision, such as a test double at an I/O boundary.

## Information leakage

Information leakage makes several modules depend on the same internal decision. A representation, policy or protocol detail appears in more than one place, so changing it requires coordinated edits.

Public re-exports of transport or wire types are leakage. Parse external data into domain types behind the interface. Keep storage schemas, framework objects and protocol details private.

A public re-export of a wire or storage type, such as a serialized message struct, a database row type or a third-party library's type, makes that type part of your interface. Every module that depends on yours now depends on its shape.

## Temporal decomposition

Temporal decomposition organizes modules by execution order instead of the knowledge they own. Separate load, validate, transform and save stages often repeat one representation and its invariants across several boundaries.

Group code around domain knowledge and ownership. Methods that run at different times can still belong to one module when they protect the same decisions.

## Pass-through method

A pass-through method forwards the same arguments to another method with the same shape. It adds a layer without hiding complexity.

Remove it or move responsibility to the module that can complete the operation. Keep a forwarding boundary only when it adds policy, adaptation, or a distinct abstraction.

## Split ownership

More than one module writes the same state or keeps its own copy of it. An agent that edits one writer can't see the others, so their rules diverge.

Give each piece of state one owner. Other modules read it or ask the owner to change it.

## Two ways to do one task

The design supports more than one way to do the same task. An agent copies whichever way it finds first, so every way keeps gaining callers.

Keep one way. Move callers off the others and delete them in the same change.

## Importable internals

A caller can import a module's internals. An agent takes the shortest path that compiles, so it imports them directly and they become part of the interface.

Make internals unreachable from outside the module, so an import from outside fails the build.

Use the visibility tools the repository's language offers: module-private or package-private visibility for internals, private submodules, internal-only folders, export lists. Expose only the interface from the module's entry point.

## Bare primitives across the boundary

An interface takes or returns a bare string, integer or path that means something specific: a user id, a validated email, a path already checked to be inside the repository. Callers can pass the wrong one, or an unchecked one, and it compiles.

Give it a type. A distinct type for each kind of ID or checked value, with a constructor that validates once, makes passing the wrong value a type error, per `principle-type-system-discipline`. Do not wrap values that carry no rule and never get confused with another.

## Hand-synced list

Two or more places list the same items, and adding an item means editing every list. An agent that sees one list updates only that one.

Keep one list and derive the others from it. If a list can't be derived, make the build fail when the lists disagree.
