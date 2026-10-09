class TmuxAgentMonitor < Formula
  desc "Tmux-native agent monitor for Claude Code, Codex, OpenCode and Pi"
  homepage "https://github.com/g-battaglia/tmux-agent-monitor"
  license "MIT"
  head "https://github.com/g-battaglia/tmux-agent-monitor.git", branch: "main"

  depends_on "rust" => :build
  depends_on "tmux"

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "tmux-agent-monitor", shell_output("#{bin}/tmux-agent-monitor --help")
  end
end
