# Registry notes

## Mirror labels are not immutable pins

`mirrors.json` currently lists `github:enis1enis2/meld@main`. That is a
discovery label, not an immutable commit or tag. No immutable ref was added
because the repository does not provide a justified tag/commit for that label.
Remote consumers must pin their own trust roots (`trust-roots.toml`) and an
exact ref (tag or commit) when fetching a mirror; do not treat the mirror label
as a supply-chain trust root.

## Revocation drill plan

Run this drill in a scratch copy of the registry. Never exercise it against the
production `revocations.json` or production trust roots.

1. Snapshot `packs/registry/` and note the signing-key location outside the repo.
2. Generate a throwaway key with `tools/packtool.py keygen`, add its public key
   to the scratch `trust-roots.toml`, and sign a scratch pack with it.
3. Confirm `tools/packtool.py verify <pack> --trust-roots <scratch-roots>
   --require-trusted` accepts the throwaway signer before revocation.
4. Add the throwaway key id to the scratch `revocations.json` with
   `tools/packtool.py revocations revoke`, then re-attest the scratch registry.
5. Confirm both `verify` and `verify-registry` reject the revoked key id, then
   discard the scratch copy and restore from the snapshot.
6. Record the date, operator and outcome in the release log. The production
   `revocations.json` stays empty until a real revocation occurs.
