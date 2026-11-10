# tau-notify

Shows tau's notices on the desktop while its window is not in front: a
chat asks the person something, is ready to land, failed or left
conflicts on main, or something else waits on them (tau-ui's
`notify.rs` decides when). Clicking one opens its chat. Its setting,
"Notify me while tau is in the background", on until switched off,
stops them.

It is a Luau plugin tau comes with (ADR 0050): `src/plugin.luau` fills
the `tau.notify` slot, and its `notify` hook shows each notice with
`ctx.desktop.notice`, which tau-luau-plugins-host gives that hook alone
and sends to the desktop's notification server over D-Bus.

`tau-plugin test plugins/tau-notify` runs its Luau tests: notices shown
in order, none when switched off, on by default.
