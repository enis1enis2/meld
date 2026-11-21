{ lib
, rustPlatform
, makeWrapper
}:

rustPlatform.buildRustPackage rec {
  pname = "meld-lang";
  # Matches the workspace Cargo.toml version and the v0.2.0-rc.5 release tag.
  version = "0.2.0-rc.5";

  src = builtins.path {
    path = ../.;
    name = "meld-lang-source";
    filter = path: type:
      let relative = builtins.replaceStrings [ toString ../. ] [ "" ] path;
      in !(lib.hasPrefix "/target" relative || lib.hasPrefix "/dist" relative
        || lib.hasPrefix "/.git" relative || lib.hasPrefix "/result" relative);
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p" "meld-cli" "-p" "meld-lsp" "-p" "meld-installer" ];

  nativeBuildInputs = [ makeWrapper ];

  doCheck = true;

  postInstall = ''
    mkdir -p $out/share/meld-lang $out/share/doc/meld-lang
    cp -R packs $out/share/meld-lang/packs
    cp -R examples $out/share/meld-lang/examples
    for doc in README.md CHANGELOG.md RELEASE.md SBOM.md SECURITY.md THIRD-PARTY-NOTICES.md LICENSE-APACHE LICENSE-MIT NOTICE; do
      cp "$doc" "$out/share/doc/meld-lang/$doc"
    done
    mkdir -p $out/share/doc/meld-lang/docs
    cp docs/schema-stability.md docs/release-artifacts.md $out/share/doc/meld-lang/docs/
    for tool in meld meld-lsp meld-pack; do
      wrapProgram $out/bin/$tool \
        --set MELD_PACKS $out/share/meld-lang/packs
    done
  '';

  meta = with lib; {
    description = "Meld language-composition compiler, CLI and language server";
    homepage = "https://github.com/enis1enis2/meld";
    license = with licenses; [ mit asl20 ];
    mainProgram = "meld";
  };
}
