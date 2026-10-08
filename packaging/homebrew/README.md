# Homebrew tap

Formula source for `g-battaglia/homebrew-agent-monitor`.
The formula builds `main` with the locked Rust dependencies. It is HEAD-only
until a tagged release and a checksummed source archive are published.
No services, Pi extensions, or agent clients are installed.

Install from the [public tap](https://github.com/g-battaglia/homebrew-agent-monitor):

```sh
brew tap g-battaglia/agent-monitor
brew install --HEAD g-battaglia/agent-monitor/agent-monitor
agent-monitor
```

Upgrade a development install with:

```sh
brew upgrade --fetch-HEAD g-battaglia/agent-monitor/agent-monitor
```

## Maintainer checks

Copy `Formula/agent-monitor.rb` to the tap repository's `Formula/` directory.
Keep the tap repository free of CI workflows and user-specific paths.

```sh
ruby -c Formula/agent-monitor.rb
brew style g-battaglia/agent-monitor/agent-monitor
brew audit --strict g-battaglia/agent-monitor/agent-monitor
```

A local tap can be checked before publication. A source build from GitHub,
however, only includes committed and pushed application changes; it cannot
include an uncommitted working tree.
