# Incident and postmortem context

Append this to an investigator's brief when the target code looks defensive: null checks, retries, timeouts, rate limits, feature flags, guards, out-of-memory handling. Defensive code is often the residue of an incident. Run incident-flavoured searches inside your own source.

- **Source control:** commit descriptions like "fix for incident", "add defensive check", "hotfix", or a revert followed by "re-apply with ...". A change merged within hours of its first commit is a hotfix signal.
- **Tickets or issues:** labels or titles such as `incident`, `sev`, `postmortem`, `reliability`, `outage`, `regression`, and action items linked from a postmortem.
- **Team chat:** incident channels and threads around the date the target code was added, and searches for the exact error string the code handles.
- **Docs:** postmortem pages and their action-item lists.
- **Observability and error tracking:** a spike, alert or new error group in the days before the change, and whether it stopped after the change shipped. Timing alone is suggestive, not causal: check what else shipped in the same window.
