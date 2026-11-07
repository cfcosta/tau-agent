# tau-phones

Lets phones reach tau on this computer (decision 0013), as a plugin
(ADR 0045). Once "Allow phones" is on, it listens for them, pairs them
by a code they scan, and carries the window's mirror both ways: what the
window applies goes out, and what a phone sends comes back to it.

What crosses to phones, and what a phone may ask, are the kernel's
(`tau_ui::mirror`). This plugin reaches the window only through the
`tau.app` service (`tau_ui_plugin::app::App`), so it carries nothing a
phone may not have.

| Module   | What it holds                                                                    |
| -------- | -------------------------------------------------------------------------------- |
| `lib`    | The UI half: `PhonesData`, `Act`, the Phones page and its Everywhere entry       |
| `bridge` | `Phones`: the server, the pairing code, the settings in `~/.config/tau/phones`   |
| `half`   | The host half: makes `Phones` from the host's window and starts it as tau starts |

The plugin is `computer_only`: its data holds the pairing code, so none
of it goes to a phone.
