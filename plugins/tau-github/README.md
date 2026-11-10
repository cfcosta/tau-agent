# tau-github

GitHub as where tau's repositories are hosted, through tau's GitHub App:
signing in with a device code or a personal access token, cloning,
pushing and pull requests. It fills the `tau.forge` slot and draws the
sign-in as a step of first-run setup, before the repositories, and as a
section of the Models screen.

A Luau package of tau's own (ADR 0050, gaps E and H).

| File                 | What it is                                                                                         |
| -------------------- | -------------------------------------------------------------------------------------------------- |
| `src/plugin.luau`    | The plugin: the forge, the sign-in's actions and job, `app`, `setup` and `models`                  |
| `src/github.luau`    | GitHub's web and API: the device sign-in, the account, repositories, pushes, pull requests, checks |
| `src/sign_in.luau`   | What the setup step and the Models screen draw of the sign-in                                      |
| `src/endpoints.luau` | Where GitHub is, and the app tau signs in through                                                  |
| `tests/sign_in.luau` | Signing in, polling, tokens and what the step draws, on a fake GitHub                              |
| `tests/forge.luau`   | The forge's answers, on a fake GitHub                                                              |

## The token

The plugin's Luau never sees the token. A device sign-in's poll asks the
host to `keep` the `access_token` field of GitHub's answer as the
`github` secret; a personal access token is typed into `ui.secret`,
whose text goes to tau's secret store; requests carry it with
`"Bearer " .. ctx.http.secret("github")`, and the forge clones and pushes
with it. Its store of the app keeps only who signed in and when the
token expires. A device token lasts eight hours, and renewing one needs
the app's secret, so an expired token is forgotten: sign in again.

A job each second polls a waiting device code at GitHub's interval (five
seconds more after a `slow_down`), checks a typed token once, and forgets
an expired one; it does nothing otherwise.

## Testing

```sh
target/release/tau-plugin test plugins/tau-github   # its Luau tests, no compiling
cargo nextest run --release -p tau-built-in-tests -E 'test(/^github::/)'
```

The second signs in against a fake GitHub through the host: the token
GitHub answers is kept as the secret and is in nothing the plugin keeps,
and the forge's requests carry it. `tau-luau-plugins-host`'s
`forge_urls` holds which Git URLs name a GitHub repository.

## Further reading

- [0050: tau's own plugins, in Luau](../../docs/decisions/0050-tau-s-plugins-in-luau.md), gaps E and H
- [0045: Everything is a plugin](../../docs/decisions/0045-everything-is-a-plugin.md), the `tau.forge` slot
