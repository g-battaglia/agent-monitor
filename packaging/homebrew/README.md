# Homebrew packaging

Canonical formula and compatibility alias for
[`g-battaglia/homebrew-tmux-agent-monitor`](https://github.com/g-battaglia/homebrew-tmux-agent-monitor).
The HEAD-only formula builds upstream `main` with locked Rust dependencies.
It installs tmux, but no services, agent clients, or Pi extensions.

```sh
brew tap g-battaglia/tmux-agent-monitor
brew install --HEAD g-battaglia/tmux-agent-monitor/tmux-agent-monitor
tmux-agent-monitor
brew upgrade --fetch-HEAD g-battaglia/tmux-agent-monitor/tmux-agent-monitor
```

## Maintenance

Keep `Formula/tmux-agent-monitor.rb` and `Aliases/agent-monitor` synchronized
with the public tap. The alias targets the canonical formula; it does not
install a second or legacy-named binary. Old GitHub repository URLs redirect
to the renamed repositories. Local checkout/tap directories can retain their
old names without changing the formula.

```sh
ruby -c Formula/tmux-agent-monitor.rb
brew style g-battaglia/tmux-agent-monitor/tmux-agent-monitor
brew audit --strict g-battaglia/tmux-agent-monitor/tmux-agent-monitor
```

No CI workflows or user-specific paths belong in the tap. A HEAD build uses
committed upstream source, not an uncommitted local working tree.
