# Homebrew formula for Meld. The system package is named `meld-lang` to avoid a
# collision with the existing `meld` diff tool; the binaries are still meld,
# meld-lsp and meld-pack.
#
# SHA256 values must be filled from the published, cosign-signed release after
# the v0.2.0-rc.4 artifacts exist. The guard below intentionally fails until
# then; there is no placeholder that pretends to be a verified checksum.
class MeldLang < Formula
  desc "Meld language-composition compiler, CLI and language server"
  homepage "https://github.com/enis1enis2/meld"
  version "0.2.0-rc.4"
  license "MIT OR Apache-2.0"

  arch = Hardware::CPU.arm? ? "aarch64" : "x86_64"
  url "https://github.com/enis1enis2/meld/releases/download/v#{version}/meld-lang-#{version}-#{arch}-apple-darwin.tar.gz"
  sha256 "TODO_VERIFY_SHA256"

  if sha256.to_s.include?("TODO_VERIFY")
    raise "Formula/meld-lang.rb: replace TODO_VERIFY_SHA256 with the checksum from the signed release"
  end

  def install
    payload = Dir.glob("meld-lang-*/").first
    raise "release archive has no payload directory" unless payload

    libexec.install Dir["#{payload}bin/*"]
    (share / "meld-lang").install Dir["#{payload}packs/*"]
    (share / "doc/meld-lang").install Dir["#{payload}share/doc/meld-lang/*"]

    %w[meld meld-lsp meld-pack].each do |tool|
      (bin / tool).write_env_script(libexec / "bin" / tool,
                                    MELD_PACKS: (share / "meld-lang/packs").to_s)
    end
  end

  test do
    assert_match "rustc:", shell_output("#{bin}/meld doctor")
    assert_predicate bin / "meld-pack", :executable?
    assert_predicate share / "meld-lang/packs/python/pack.toml", :file?
  end
end
