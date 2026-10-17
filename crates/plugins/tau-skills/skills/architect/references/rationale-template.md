# Rationale template

The prose that ships with the type sketch. One page. Sentence-case headings, no boilerplate. Replace the italic notes with real content.

## Problem

*One paragraph. What we are trying to do, and what about the existing system or constraints makes the shape non-obvious. If Phase A turned up constraints the design must honour (existing types to interoperate with, callers we can't break, invariants that cross our boundary), name them here so the reader sees the constraints you saw.*

## Usage (caller's view)

*Write this first, before the type sketch. Show the README or quickstart the consumer reads, plus two or three realistic call sites in their own code: what they import, what they call, what comes back. The type sketch in Shape is derived from this. The two must agree. When they diverge, reconcile the sketch to the usage, not the reverse. The caller's experience is the spec. The types serve it.*

## Shape

*The recommended architecture. Data structures first. Then how data flows through the signatures. Name the load-bearing decisions. State which invariants are encoded in types, where validation lives, and what the system deliberately does not do. Judge interface depth explicitly: what complexity the public interface hides, what stays exposed to callers, and why the interface is no larger than needed. Name the principle behind each decision (for example "per `principle-boundary-discipline`"). Don't restate it.*

## Synthesis decision

*Filled in by the `arena` run, or by your own comparison when you wrote the candidates yourself. Which candidate became the base and why, what was taken from each of the others, and what was rejected and why.*

## Tradeoffs accepted

*One bullet per tradeoff the chosen shape makes, in the form "we accept X in exchange for Y". Name anything a future reader might mistake for an oversight, including things that look like premature optimization or premature simplification.*

## Alternatives considered

*Required. Name at least one concrete alternative shape, with one line on why it lost. Judge each alternative on interface depth, not implementation simplicity alone: name the complexity it exposes to callers and the complexity it hides. Two or three alternatives belong here when the design space had real contenders. One is fine when the constraints forced the answer, phrased as "this was the only viable shape because...". Avoid listing flavours of the same shape. This section covers design alternatives the chosen shape rejected, not the other arena candidates.*

## Open questions and risks

*Things you noticed during the sketch that the person needs to weigh in on, and risks worth flagging before implementation starts. Phrase them as questions, so the person's answer is the resolution. If one needs an answer before you can go on, put it to the person with `ask`.*

## Next implementation step

*The first thing to build against the sketch. One sentence: what you would start writing right after synthesis, or after the checkpoint if there was one.*
