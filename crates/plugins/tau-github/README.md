# tau-github

The interface half of GitHub as where tau's repositories are hosted. It
draws the GitHub sign-in in two places: a step of first-run setup that
comes before the repositories, and a section of the Models screen with
who is signed in and Sign out. It reaches no network: what it asks goes
to its host half, and what it draws comes from the data the host half
publishes.

## What it provides

| Item                      | What it is                                                                                       |
| ------------------------- | ------------------------------------------------------------------------------------------------ |
| `GitHubUi`                | The plugin's `UiPlugin`: the setup step (`points::SETUP`) and the Models section (`points::MODELS`) |
| `GitHubData`, `SignIn`    | Its data: how the sign-in stands, `Unknown` until a host says                                    |
| `DeviceCode`              | A device code to enter on GitHub, where, and for how long                                        |
| `Act`                     | What the UI asks the host half: `SignIn`, `Check`, `Token`, `SignOut`                            |
| `Signed`                  | The host half's answer once a sign-in is saved; its reply ends the setup step                    |
| `Do`                      | What a sign-in button asks, in its arguments                                                     |
| `sign_in::form`           | The sign-in's tree: start, the code, the token form, or who is signed in                        |
| `step`, `section`         | The setup step and the Models section                                                            |
| `NAME`, `STEP`            | `"tau-github"`, the name both halves go by, and the step's name                                 |
| `install_url`, `APP_SLUG` | Where to give tau's GitHub App access to more repositories                                       |

## How it fits

This is the interface half (decisions 0017, 0030, 0045). Its partner is
[tau-github-host](../tau-github-host), which signs in, keeps the token
and fills the `tau.forge` slot the host clones, pushes and opens pull
requests through.

- Builds on `tau-ui-plugin` and `serde`.
- Listed in `tau-plugins`; `tau-ui` registers its host half, and its demo
  scripts its data, since the demo reaches no GitHub.

The step is placed with `SetupAt::BeforeRepositories`: the repositories
to pick come from the forge, so it signs in first. Its Continue, and the
reply to a saved sign-in, end it with `Handle::setup_done`. A token is
typed into a secret input, whose draft stays in the window that edits
it; only Enter gives it to the plugin, which sends it on as
`Act::Token`.

## Testing

```sh
cargo nextest run --release -p tau-github
```

`tests/sign_in.rs` draws the step and the section from each state of the
sign-in and checks what their buttons, the token's Enter and the host
half's reply ask.
