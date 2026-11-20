# Debian packaging

Build from the repository root:

```sh
cp -R packaging/deb debian
dpkg-buildpackage -us -uc -b
```

`debian/rules` uses `cargo build --release --locked`, derives
`SOURCE_DATE_EPOCH` from `debian/changelog`, and installs:

- `meld`, `meld-lsp`, `meld-pack` into `usr/bin`;
- all Lang Pack data into `usr/share/meld-lang/packs`;
- licenses, changelog and release docs into `usr/share/doc/meld-lang`;
- a `MELD_PACKS` export into `etc/profile.d/meld-lang.sh`.

The source package is deliberately named `meld-lang` so it does not collide
with Debian's existing `meld` diff tool. No language runtime is a dependency.
