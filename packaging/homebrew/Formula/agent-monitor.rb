class AgentMonitor < Formula
  desc "Independent tmux monitor and session explorer for coding agents"
  homepage "https://github.com/g-battaglia/agent-monitor"
  license "MIT"
  head "https://github.com/g-battaglia/agent-monitor.git", branch: "main"

  depends_on "rust" => :build
  depends_on "tmux"

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "agent-monitor", shell_output("#{bin}/agent-monitor --help")
  end
end
