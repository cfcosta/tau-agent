# tau-github-host

GitHub through tau's GitHub App, as where tau's repositories are hosted:
the host half of [tau-github](../tau-github). It signs in as tau-github's
UI asks, keeps the sign-in in tau's config directory, and fills the
`tau.forge` slot the host lists, clones, pushes and opens pull requests
through.

## What it provides

| Item                             | What it is                                                                                  |
| -------------------------------- | ------------------------------------------------------------------------------------------- |
| `GitHubHost`                     | The `HostHalf`: the plugin's data (how the sign-in stands), its `act`, and the forge's fill |
| `GitHub`                         | The `tau.forge` slot's filler (`Forge`), and the sign-in going on (`act`, `sign_in_state`) |
| `Api`, `Endpoints`               | GitHub's web and API, at their usual addresses or at a fake server's                        |
| `DeviceStart`, `Poll`, `CLIENT_ID` | A device sign-in: the code to show, each poll's answer, and the app's client id           |
| `Token`                          | The saved sign-in, `github.json` in tau's config directory, readable only by the user      |
| `url::repository_of`             | The full name a Git URL names on GitHub, in any form Git takes                             |

## How it fits

The host half (decisions 0017, 0030, 0045). tau-github's UI sends it an
`Act`:

- `sign_in` asks GitHub for a device code, shows it as the plugin's data
  (`SignIn::Waiting`), and polls in the background until the person
  approves, the code expires, or another sign-in starts.
- `check` polls at once, when the person says they approved.
- `token` checks a personal access token against `GET /user`.
- `sign_out` forgets the saved sign-in.

A saved sign-in answers `Signed { user }`. Each change asks the
interface for the catalog again (`HostCx::refresh`), whose data the
plugin draws and whose `pull_requests` tells the host to list the
forge's repositories for setup. A device sign-in needs no secret, so none
ships with tau; its tokens expire after eight hours, and renewing one
needs the app's secret, so an expired token counts as signed out.

- Builds on `tau-agent`, `tau-ai` (its HTTP client and private files),
  `tau-github`, `tau-ui-plugin`, `reqwest`, `gix` (Git URLs) and `tokio`.
- Used by `tau-ui`, which registers `GitHubHost`.

## Testing

```sh
cargo nextest run --release -p tau-github-host
```

Against a fake GitHub on localhost (`tests/support`): `tests/api.rs`
signs in and lists repositories, `tests/url.rs` reads Git URLs, and
`tests/sign_in.rs` carries out the UI's acts and checks the data they
leave.
