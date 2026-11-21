# Homebrew formula for Meld. The system package is named `meld-lang` to avoid a
# collision with the existing `meld` diff tool; the binaries are still meld,
# meld-lsp and meld-pack.
#
# SHA256 values are filled from the published, cosign-signed release by
# tools/generate_channel_metadata.py after the v0.2.0 artifacts exist. The guard
# below intentionally fails until then; there is no placeholder that pretends
# to be a verified checksum.
class MeldLang < Formula
  desc "Meld language-composition compiler, CLI and language server"
  homepage "https://github.com/enis1enis2/meld"
  version "0.2.0"
  license "MIT OR Apache-2.0"

  # BEGIN MANAGED CHECKSUMS (tools/generate_channel_metadata.py)
  SHA256S = {
    "aarch64-apple-darwin" => "07fb4cd20c24829395ed20cca17c6dc9fe65e4a3a2cc2d3fcd75fbc97ccdc9bb",
    "x86_64-apple-darwin" => "16937be21eec5b0813a42c479c7ca98933825f4c88e06f8ee3ab3b63ce020ae9",
  }.freeze
  # END MANAGED CHECKSUMS

  arch = Hardware::CPU.arm? ? "aarch64" : "x86_64"
  target = "#{arch}-apple-darwin"
  digest = SHA256S.fetch(target)
  if digest.include?("TODO_VERIFY")
    raise "Formula/meld-lang.rb: checksum must come from the signed release"
  end

  url "https://github.com/enis1enis2/meld/releases/download/v#{version}/meld-lang-#{version}-#{target}.tar.gz"
  sha256 digest

  def install
    payload = Dir.glob("meld-lang-*/").first
    raise "release archive has no payload directory" unless payload

    # Archives contain <payload>/bin/*; install to libexec/bin so the wrappers
    # below point at the files that were actually installed.
    (libexec / "bin").install Dir["#{payload}bin/*"]
    (share / "meld-lang").install "#{payload}packs" => "packs"
    examples = "#{payload}share/meld-lang/examples"
    (share / "meld-lang").install examples => "examples" if File.directory?(examples)
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
